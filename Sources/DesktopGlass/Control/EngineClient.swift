import Foundation

/// Runs `dg engine` (the Rust engine bundled next to the app) and talks to it: newline-delimited JSON-RPC 2.0 over
/// its stdin/stdout. See engine/crates/dg-proto for the messages.
///
/// - The engine owns the control channel (control.json); control calls it can't answer (browser, open editor
///   buffers, windows) arrive here as `ui/control` requests and go to `AppControl`.
/// - Shared settings (backend, models, schedule, `mcp.allow.*`, …) live in the engine's settings.json. This class
///   mirrors them both ways with UserDefaults, so SwiftUI's `@AppStorage` bindings keep working unchanged.
/// - The engine is restarted with backoff if it exits while the app is running.
@MainActor
final class EngineClient {
    static let protocolVersion = 1
    /// The app's one engine (started by AppDelegate).
    static let shared = EngineClient()

    /// Whether the engine is up and has answered `engine/hello`.
    private(set) var ready = false
    private var subscribers: [String: [([String: Any]) -> Void]] = [:]
    private var readyWaiters: [CheckedContinuation<Void, Never>] = []

    /// Calls `handler` (on the main actor) for every engine notification named `method`, e.g. "index/progress".
    func on(_ method: String, _ handler: @escaping ([String: Any]) -> Void) {
        subscribers[method, default: []].append(handler)
    }

    /// Waits until the engine has completed its handshake (returns at once if it already has).
    func waitUntilReady() async {
        if ready { return }
        await withCheckedContinuation { readyWaiters.append($0) }
    }

    /// Answers `ui/control` requests (AppControl.handle).
    var controlHandler: ((String, [String: Any]) async throws -> Any)?
    /// `desktop/changed` with the changed paths.
    var onDesktopChanged: (([String]) -> Void)?
    /// The control channel's port, once the engine is up (the artifact viewer hides it from "local servers").
    var onReady: ((Int) -> Void)?

    private var process: Process?
    private var stdin: FileHandle?
    private var nextID = 1
    private var pending: [Int: CheckedContinuation<Any, Error>] = [:]
    private var buffer = Data()
    private var stopping = false
    private var restarts = 0
    /// Last values exchanged with the engine, so echoes of our own writes aren't sent back.
    private var mirrored: [String: String] = [:]
    private var defaultsObserver: NSObjectProtocol?

    /// The engine binary: Contents/MacOS/dg next to the app, else ~/.local/bin/dg.
    static var binary: URL? {
        let candidates = [Bundle.main.executableURL?.deletingLastPathComponent().appendingPathComponent("dg"),
                          FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".local/bin/dg")]
        return candidates.compactMap { $0 }.first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    func start() {
        stopping = false
        launch()
        if defaultsObserver == nil {
            defaultsObserver = NotificationCenter.default.addObserver(forName: UserDefaults.didChangeNotification,
                                                                      object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.pushLocalSettings() }
            }
        }
    }

    func stop() {
        stopping = true
        if process?.isRunning == true {
            send(["jsonrpc": "2.0", "id": 0, "method": "engine/shutdown", "params": [:]])
            try? stdin?.close()          // EOF also stops it
            let p = process
            DispatchQueue.global().asyncAfter(deadline: .now() + 1) { if p?.isRunning == true { p?.terminate() } }
        }
    }

    // MARK: Requests

    func call(_ method: String, _ params: [String: Any] = [:]) async throws -> Any {
        guard process?.isRunning == true else { throw ControlError("The DesktopGlass engine isn't running") }
        let id = nextID
        nextID += 1
        return try await withCheckedThrowingContinuation { cont in
            pending[id] = cont
            send(["jsonrpc": "2.0", "id": id, "method": method, "params": params])
        }
    }

    // MARK: Process

    private func launch() {
        guard let bin = Self.binary else {
            NSLog("DesktopGlass: engine binary (dg) not found; MCP app tools are unavailable")
            return
        }
        let p = Process()
        p.executableURL = bin
        p.arguments = ["engine"]
        let input = Pipe(), output = Pipe()
        p.standardInput = input
        p.standardOutput = output
        p.standardError = FileHandle.standardError
        let box = WeakEngine(self)
        output.fileHandleForReading.readabilityHandler = { h in
            let data = h.availableData
            DispatchQueue.main.async { MainActor.assumeIsolated { box.value?.receive(data) } }
        }
        p.terminationHandler = { proc in
            DispatchQueue.main.async { MainActor.assumeIsolated { box.value?.exited(proc) } }
        }
        do {
            try p.run()
        } catch {
            NSLog("DesktopGlass: couldn't start the engine: \(error.localizedDescription)")
            return
        }
        process = p
        stdin = input.fileHandleForWriting
        buffer = Data()
        Task { await self.handshake() }
    }

    private func handshake() async {
        do {
            let hello = try await call("engine/hello") as? [String: Any] ?? [:]
            if let v = hello["protocol"] as? Int, v != Self.protocolVersion {
                NSLog("DesktopGlass: engine protocol \(v), app expects \(Self.protocolVersion)")
            }
            restarts = 0
            ready = true
            readyWaiters.forEach { $0.resume() }
            readyWaiters = []
            for h in subscribers["engine/ready"] ?? [] { h([:]) }
            if let port = (hello["control"] as? [String: Any])?["port"] as? Int, port > 0 { onReady?(port) }
            try await pullEngineSettings(keys: nil)
            pushLocalSettings()
        } catch {
            NSLog("DesktopGlass: engine handshake failed: \(error.localizedDescription)")
        }
    }

    private func exited(_ proc: Process) {
        guard proc === process else { return }
        ready = false
        output(nil)
        process = nil
        stdin = nil
        for (_, c) in pending { c.resume(throwing: ControlError("The DesktopGlass engine stopped")) }
        pending = [:]
        guard !stopping else { return }
        restarts += 1
        let delay = min(30.0, pow(2.0, Double(min(restarts, 5))) * 0.5)    // 1s, 2s, 4s … 30s
        NSLog("DesktopGlass: engine exited (\(proc.terminationStatus)); restarting in \(delay)s")
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let self, !self.stopping, self.process == nil else { return }
            self.launch()
        }
    }

    private func output(_ h: FileHandle?) {
        (process?.standardOutput as? Pipe)?.fileHandleForReading.readabilityHandler = nil
    }

    // MARK: Wire

    private func send(_ obj: [String: Any]) {
        guard let stdin, var data = try? JSONSerialization.data(withJSONObject: obj) else { return }
        data.append(10)
        try? stdin.write(contentsOf: data)
    }

    private func receive(_ data: Data) {
        guard !data.isEmpty else { return }
        buffer.append(data)
        while let nl = buffer.firstIndex(of: 10) {
            let line = buffer.prefix(upTo: nl)
            buffer.removeSubrange(...nl)
            guard let msg = try? JSONSerialization.jsonObject(with: line) as? [String: Any] else { continue }
            dispatch(msg)
        }
    }

    private func dispatch(_ msg: [String: Any]) {
        guard let method = msg["method"] as? String else {
            // a response to one of our calls
            guard let id = msg["id"] as? Int, let cont = pending.removeValue(forKey: id) else { return }
            if let err = msg["error"] as? [String: Any] {
                cont.resume(throwing: ControlError(err["message"] as? String ?? "Engine error"))
            } else {
                cont.resume(returning: msg["result"] ?? NSNull())
            }
            return
        }
        let params = msg["params"] as? [String: Any] ?? [:]
        switch method {
        case "ui/control":
            let id = msg["id"] ?? NSNull()
            let m = params["method"] as? String ?? ""
            let p = params["params"] as? [String: Any] ?? [:]
            Task {
                do {
                    guard let handler = self.controlHandler else { throw ControlError("DesktopGlass isn't ready") }
                    let result = try await handler(m, p)
                    self.send(["jsonrpc": "2.0", "id": id, "result": Self.jsonSafe(result)])
                } catch {
                    self.send(["jsonrpc": "2.0", "id": id,
                               "error": ["code": 1, "message": error.localizedDescription]])
                }
            }
        case "desktop/changed":
            onDesktopChanged?(params["paths"] as? [String] ?? [])
            for h in subscribers[method] ?? [] { h(params) }
        case "settings/changed":
            let keys = params["keys"] as? [String]
            Task { try? await self.pullEngineSettings(keys: keys) }
        case "engine/log":
            NSLog("dg engine: \(params["message"] as? String ?? "")")
        default:
            if msg["id"] == nil, let handlers = subscribers[method] {
                handlers.forEach { $0(params) }
                return
            }
            if let id = msg["id"] {
                send(["jsonrpc": "2.0", "id": id, "error": ["code": -32601, "message": "Unknown method \(method)"]])
            }
        }
    }

    /// JSONSerialization only takes plist-ish types; anything else becomes its description.
    private static func jsonSafe(_ v: Any) -> Any {
        if JSONSerialization.isValidJSONObject([v]) { return v }
        return String(describing: v)
    }

    // MARK: Settings mirror

    private static let prefixes = ["model.", "endpoint.", "mcp.allow.", "unattended."]
    private static let exact: Set<String> = ["harness", "schedule", "scheduleHour", "sandboxUnattended",
                                             "signedOutHarness", "lastReindexRun", "lastReindexSuccess", "gamesFolder"]

    static func isShared(_ key: String) -> Bool {
        exact.contains(key) || prefixes.contains { key.hasPrefix($0) }
    }

    private static let iso = ISO8601DateFormatter()

    /// A UserDefaults value as JSON + a comparable fingerprint.
    private static func encode(_ v: Any) -> (json: Any, key: String)? {
        switch v {
        case let d as Date: let s = iso.string(from: d); return (s, "d:" + s)
        case let n as NSNumber where CFGetTypeID(n) == CFBooleanGetTypeID(): return (n.boolValue, "b:\(n.boolValue)")
        case let n as NSNumber: return (n, "n:\(n)")
        case let s as String: return (s, "s:" + s)
        default: return nil
        }
    }

    /// Our UserDefaults → the engine, for shared keys that changed since the last exchange.
    private func pushLocalSettings() {
        guard let values = pendingLocalSettings() else { return }
        Task { _ = try? await self.call("settings/set", ["values": values]) }
    }

    /// Sends any not-yet-mirrored setting changes and waits until the engine has them. Call before a request that
    /// depends on settings the user may have just changed (e.g. starting a re-index right after picking a backend).
    func flushSettings() async {
        guard let values = pendingLocalSettings() else { return }
        _ = try? await call("settings/set", ["values": values])
    }

    private func pendingLocalSettings() -> [String: Any]? {
        guard process?.isRunning == true else { return nil }
        var values: [String: Any] = [:]
        let local = UserDefaults.standard.dictionaryRepresentation().filter { Self.isShared($0.key) }
        for (k, v) in local {
            guard let (json, fp) = Self.encode(v), mirrored[k] != fp else { continue }
            mirrored[k] = fp
            values[k] = json
        }
        return values.isEmpty ? nil : values
    }

    /// The engine's settings → UserDefaults (it's the source of truth for shared keys).
    private func pullEngineSettings(keys: [String]?) async throws {
        var params: [String: Any] = [:]
        if let keys { params["keys"] = keys }
        guard let values = try await call("settings/get", params) as? [String: Any] else { return }
        let d = UserDefaults.standard
        for (k, v) in values where Self.isShared(k) {
            var stored: Any = v
            if k.hasPrefix("lastReindex"), let s = v as? String, let date = Self.iso.date(from: s) ?? Self.legacyDate(s) {
                stored = date
            }
            guard let (_, fp) = Self.encode(stored) else { continue }
            mirrored[k] = fp
            if let cur = d.object(forKey: k), let (_, curFp) = Self.encode(cur), curFp == fp { continue }
            d.set(stored, forKey: k)
        }
        // keys the engine dropped
        for k in keys ?? [] where values[k] == nil && d.object(forKey: k) != nil {
            mirrored[k] = nil
            d.removeObject(forKey: k)
        }
    }

    /// `defaults read` prints dates as "2026-11-06 16:00:00 +0000" (what the one-time migration copied).
    private static func legacyDate(_ s: String) -> Date? {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd HH:mm:ss Z"
        return f.date(from: s)
    }
}

/// Lets the pipe/termination handlers (which run off the main actor) reach the client without capturing `self`.
private final class WeakEngine: @unchecked Sendable {
    weak var value: EngineClient?
    init(_ v: EngineClient) { value = v }
}
