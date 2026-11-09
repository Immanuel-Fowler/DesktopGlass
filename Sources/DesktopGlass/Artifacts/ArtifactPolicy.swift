import Foundation

/// What the artifact viewer is allowed to show. It is deliberately not a general browser: anything outside the
/// allow-list opens in the user's default browser instead.
///
/// The rules live in the engine (engine/crates/dg-core/src/artifact_policy.rs). WebKit asks synchronously, so the
/// engine's tables (`policy/tables`: extensions, artifact and sign-in hosts, path rules, …) are fetched once when it
/// starts and evaluated here by a small matcher — dg-engine's methods/tools/policy.rs documents it and tests it
/// against the Rust predicates. Until the tables arrive nothing is allowed or handed off (callers that open things
/// at start-up wait with `whenLoaded`). Resolving paths and address-bar text needs the file system: the engine
/// answers that (`policy/resolve`).
@MainActor
enum ArtifactPolicy {
    struct Tables {
        var fileExtensions: Set<String> = []
        var artifactHosts: [String] = []
        var signInHosts: [String] = []
        var localHosts: Set<String> = []
        var localSuffixes: [String] = []
        var pathRules: [String: [String]] = [:]
        var inertSchemes: Set<String> = []
        var handOffSchemes: Set<String> = []
        var safariUserAgent: String?

        init() {}

        init(json d: [String: Any]) {
            func list(_ k: String) -> [String] { d[k] as? [String] ?? [] }
            fileExtensions = Set(list("fileExtensions"))
            artifactHosts = list("artifactHosts")
            signInHosts = list("signInHosts")
            localHosts = Set(list("localHosts"))
            localSuffixes = list("localSuffixes")
            pathRules = d["pathRules"] as? [String: [String]] ?? [:]
            inertSchemes = Set(list("inertSchemes"))
            handOffSchemes = Set(list("handOffSchemes"))
            safariUserAgent = d["safariUserAgent"] as? String
        }
    }

    private(set) static var tables: Tables?
    private static var waiters: [() -> Void] = []

    /// Fetches the tables from the engine (at start-up and after each engine restart).
    static func load() async {
        guard let d = try? await EngineClient.shared.call("policy/tables") as? [String: Any] else { return }
        install(Tables(json: d))
    }

    static func install(_ t: Tables) {
        tables = t
        let w = waiters
        waiters = []
        w.forEach { $0() }
    }

    /// Runs `work` now if the rules are loaded, else as soon as they are.
    static func whenLoaded(_ work: @escaping () -> Void) {
        if tables != nil { work() } else { waiters.append(work) }
    }

    /// Waits for the rules (returns at once if they're loaded).
    static func ready() async {
        if tables != nil { return }
        await withCheckedContinuation { cont in whenLoaded { cont.resume() } }
    }

    /// Local files of these types open in the viewer (and get a View button in the overlay).
    static var fileExtensions: Set<String> { tables?.fileExtensions ?? [] }

    /// Viewer pages present as Safari: some sign-in pages (Google's especially) refuse anything that looks embedded.
    static var safariUserAgent: String? { tables?.safariUserAgent }

    private static func hostIn(_ host: String, _ list: [String]) -> Bool {
        list.contains { host == $0 || host.hasSuffix("." + $0) }
    }

    static func isAllowed(_ url: URL) -> Bool {
        guard let t = tables else { return false }
        if url.isFileURL { return t.fileExtensions.contains(url.pathExtension.lowercased()) || url.hasDirectoryPath }
        guard let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
              let host = url.host?.lowercased() else {
            return url.scheme.map(t.inertSchemes.contains) ?? false
        }
        if t.localHosts.contains(host) || t.localSuffixes.contains(where: { host.hasSuffix($0) }) { return true }
        if let prefixes = t.pathRules[host] { return prefixes.contains { url.path.hasPrefix($0) } }
        return hostIn(host, t.artifactHosts)
    }

    /// Sign-in pages an artifact may send you to. Allowed only as a navigation inside a tab that's already showing
    /// an artifact, so signing in happens in the viewer (and its cookies stay there). Never opened on their own.
    static func isSignIn(_ url: URL) -> Bool {
        guard let t = tables, url.scheme?.lowercased() == "https", let host = url.host?.lowercased() else { return false }
        return hostIn(host, t.signInHosts)
    }

    /// A web page (http/https with a host).
    static func isWebPage(_ url: URL) -> Bool {
        guard let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https" else { return false }
        return !(url.host ?? "").isEmpty
    }

    /// What opens when someone asks for it by name — `dgview`, a desktopglass:// link, the show_artifact tool or the
    /// address bar: everything the viewer shows on its own, plus any web page.
    static func canOpenOnRequest(_ url: URL) -> Bool { isAllowed(url) || isWebPage(url) }

    /// What may be passed on to another app when the viewer won't show it: web pages and mail links only. Never
    /// files or other schemes — NSWorkspace would launch an .app, run a .command or open any handler.
    static func canHandOff(_ url: URL) -> Bool {
        guard let t = tables, let scheme = url.scheme?.lowercased() else { return false }
        return t.handOffSchemes.contains(scheme) || isWebPage(url)
    }

    /// Turns what an AI or a user hands us (a path, ~/path, Desktop-relative path, or URL) into a URL (engine).
    static func resolve(_ raw: String) async -> URL? { await target(raw, address: false) }

    /// Address-bar text → URL: "localhost:5173" → http://localhost:5173, "example.com/x" → https://…, paths as resolve.
    static func address(_ raw: String) async -> URL? { await target(raw, address: true) }

    private static func target(_ raw: String, address: Bool) async -> URL? {
        guard let r = try? await EngineClient.shared.call("policy/resolve", ["raw": raw, "address": address]) else { return nil }
        if tables == nil { await load() }      // the engine answered, so its rules are there too
        guard let d = r as? [String: Any] else { return nil }
        if let path = d["path"] as? String { return URL(fileURLWithPath: path) }
        return (d["url"] as? String).flatMap { URL(string: $0) }
    }
}
