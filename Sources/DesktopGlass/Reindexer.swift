import Foundation

/// Re-index schedule choices. The engine runs the schedule (methods/index in dg-engine); this is the labels and
/// the stored value the pickers bind to (`schedule` / `scheduleHour`, mirrored to the engine's settings.json).
enum Schedule: String, CaseIterable {
    case off, hourly, sixHours = "every6h", daily

    var label: String {
        switch self {
        case .off: return "Off"
        case .hourly: return "Every hour"
        case .sixHours: return "Every 6 hours"
        case .daily: return "Daily at \(Schedule.hourLabel(Schedule.hour))"
        }
    }

    /// Hour of day (0–23) for the daily run.
    static var hour: Int {
        get { min(23, max(0, UserDefaults.standard.object(forKey: "scheduleHour") as? Int ?? 3)) }
        set { UserDefaults.standard.set(min(23, max(0, newValue)), forKey: "scheduleHour") }
    }

    static func hourLabel(_ h: Int) -> String {
        let suffix = h < 12 ? "AM" : "PM"
        let twelve = h % 12 == 0 ? 12 : h % 12
        return "\(twelve) \(suffix)"
    }

    /// (The engine migrates values saved by earlier versions at launch.)
    static var current: Schedule {
        get { Schedule(rawValue: UserDefaults.standard.string(forKey: "schedule") ?? "") ?? .daily }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: "schedule") }
    }
}

/// What went wrong on the last run, if anything the user should act on (decided by the engine).
enum IndexProblem: Equatable {
    case signedOut(Harness)
    case notInstalled(Harness)
    case limited(Harness)
    case notRunning(Harness)
    case needsConsent(Harness)
    case failed(String)

    var message: String {
        switch self {
        case .signedOut(let h): return "Not signed in to \(h.label)"
        case .notInstalled(let h): return "\(h.label) isn't installed"
        case .limited(let h): return "\(h.label) hit a usage limit"
        case .notRunning(let h): return "\(h.label) isn't ready"
        case .needsConsent(let h): return "Unattended runs with \(h.label) are off"
        case .failed(let m): return m
        }
    }

    /// The engine's `{kind, harness?, message}`.
    init?(engine value: Any?) {
        guard let o = value as? [String: Any], let kind = o["kind"] as? String else { return nil }
        let message = o["message"] as? String ?? ""
        let h = (o["harness"] as? String).flatMap(Harness.init(rawValue:))
        switch (kind, h) {
        case ("signedOut", let h?): self = .signedOut(h)
        case ("notInstalled", let h?): self = .notInstalled(h)
        case ("limited", let h?): self = .limited(h)
        case ("notRunning", let h?): self = .notRunning(h)
        case ("needsConsent", let h?): self = .needsConsent(h)
        default: self = .failed(message)
        }
    }
}

/// The Rust engine: Contents/MacOS/dg next to the app's executable, else ~/.local/bin/dg (install.sh's link).
enum DGEngine {
    static var binary: URL? {
        var candidates: [URL] = []
        if let exe = Bundle.main.executableURL { candidates.append(exe.deletingLastPathComponent().appendingPathComponent("dg")) }
        candidates.append(Layout.home.appendingPathComponent(".local/bin/dg"))
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }
}

/// Re-indexing as the UI sees it. The engine runs `dg reindex`, interprets its exit code, keeps the bookkeeping
/// and runs the schedule; this mirrors its state (`index/progress`, `index/problem`, `index/finished`) for the views.
@MainActor
final class Reindexer: ObservableObject {
    @Published private(set) var running = false
    @Published private(set) var checkingAuth = false
    @Published private(set) var status = ""
    @Published private(set) var problem: IndexProblem?
    var onFinish: (() -> Void)?
    private let engine = EngineClient.shared

    init() {
        // Until the engine answers: a sign-in problem survives restarts.
        if let raw = UserDefaults.standard.string(forKey: "signedOutHarness"), let h = Harness(rawValue: raw) {
            problem = .signedOut(h)
            status = IndexProblem.signedOut(h).message
        }
        engine.on("index/progress") { [weak self] p in self?.apply(p) }
        engine.on("index/problem") { [weak self] p in self?.problem = IndexProblem(engine: p["problem"]) }
        engine.on("index/finished") { [weak self] _ in self?.onFinish?() }
        engine.on("engine/ready") { [weak self] _ in Task { await self?.refresh() } }
    }

    static var lastRun: Date? { UserDefaults.standard.object(forKey: "lastReindexRun") as? Date }

    /// Last run that finished cleanly (including "nothing changed" checks).
    static var lastSuccess: Date? { UserDefaults.standard.object(forKey: "lastReindexSuccess") as? Date }

    func run(reason: String, full: Bool = false) {
        guard !running, !checkingAuth else { return }
        var params: [String: Any] = ["reason": reason, "full": full]
        let h = Harness.current
        // Someone clicked Re-index: ask now. Schedule and MCP runs never pop up a dialog. The consent goes along with
        // the run so the engine doesn't depend on the settings mirror having caught up.
        if ["button", "menu", "settings", "settings-rewrite"].contains(reason), !h.isLocal,
           h.unattendedAllowed || Consent.ask(h) {
            params["allowUnattended"] = true
        }
        send("index/run", params)
    }

    /// Verifies the selected CLI is signed in (offline where possible, otherwise a one-word prompt).
    func checkAuth() {
        guard !running, !checkingAuth else { return }
        send("index/check_auth", [:])
    }

    func cancel() { send("index/cancel", [:]) }

    private func send(_ method: String, _ params: [String: Any]) {
        Task {
            do {
                await engine.flushSettings()     // a backend/model picked a moment ago must reach the engine first
                apply(try await engine.call(method, params) as? [String: Any] ?? [:])
            } catch {
                status = error.localizedDescription
            }
        }
    }

    private func refresh() async {
        guard let s = try? await engine.call("index/status") as? [String: Any] else { return }
        apply(s)
        problem = IndexProblem(engine: s["problem"])
    }

    private func apply(_ p: [String: Any]) {
        if let r = p["running"] as? Bool { running = r }
        if let c = p["checkingAuth"] as? Bool { checkingAuth = c }
        if let s = p["status"] as? String { status = s }
    }
}
