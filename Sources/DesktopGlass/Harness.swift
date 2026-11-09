import AppKit
import Foundation

/// Everything that can write the Desktop descriptions: agent CLIs, and on-device models on Apple silicon.
enum Harness: String, CaseIterable, Identifiable {
    // agent CLIs (run tools themselves, follow SKILL.md)
    case claude, codex, gemini, antigravity, cursor, copilot, opencode
    // on-device (`dg reindex` drives them)
    case apple, mlx, ollama, lmstudio, llamacpp

    var id: String { rawValue }

    static var agents: [Harness] { [.claude, .codex, .gemini, .antigravity, .cursor, .copilot, .opencode] }
    static var locals: [Harness] { [.apple, .mlx, .ollama, .lmstudio, .llamacpp] }

    var isLocal: Bool { Harness.locals.contains(self) }

    var label: String {
        switch self {
        case .claude: return "Claude Code"
        case .codex: return "Codex CLI"
        case .gemini: return "Gemini CLI"
        case .antigravity: return "Antigravity CLI"
        case .cursor: return "Cursor CLI"
        case .copilot: return "GitHub Copilot CLI"
        case .opencode: return "OpenCode"
        case .apple: return "Apple Foundation Models"
        case .mlx: return "MLX"
        case .ollama: return "Ollama"
        case .lmstudio: return "LM Studio"
        case .llamacpp: return "llama.cpp server"
        }
    }

    /// Binary names, in lookup order.
    var binaries: [String] {
        switch self {
        case .claude: return ["claude"]
        case .codex: return ["codex"]
        case .gemini: return ["gemini"]
        case .antigravity: return ["agy"]
        case .cursor: return ["cursor-agent", "agent"]
        case .copilot: return ["copilot"]
        case .opencode: return ["opencode"]
        case .ollama: return ["ollama"]
        case .lmstudio: return ["lms"]
        case .llamacpp: return ["llama-server"]
        case .apple, .mlx: return []
        }
    }

    /// Desktop app bundles that count as "installed".
    var appBundles: [String] {
        switch self {
        case .ollama: return ["/Applications/Ollama.app"]
        case .lmstudio: return ["/Applications/LM Studio.app"]
        default: return []
        }
    }

    var defaultEndpoint: String? {
        switch self {
        case .ollama: return "http://localhost:11434"
        case .lmstudio: return "http://localhost:1234"
        case .llamacpp: return "http://localhost:8080"
        default: return nil
        }
    }

    var usesServer: Bool { defaultEndpoint != nil }

    var defaultModel: String { self == .claude ? "haiku" : "" }

    var modelPlaceholder: String {
        switch self {
        case .claude: return "haiku, sonnet, opus"
        case .codex: return "Default (e.g. gpt-5-codex)"
        case .gemini: return "Default (e.g. gemini-2.5-flash)"
        case .antigravity: return "Default (from agy models)"
        case .cursor: return "Default (e.g. sonnet-4.5)"
        case .copilot: return "Default (e.g. claude-sonnet-4.5)"
        case .opencode: return "Default (provider/model)"
        case .apple: return "On-device model"
        case .mlx: return "mlx-community/…-4bit"
        case .ollama: return "e.g. llama3.2, qwen2.5:7b"
        case .lmstudio, .llamacpp: return "Loaded model"
        }
    }

    var usesSkill: Bool { self == .claude }

    var detail: String {
        switch self {
        case .claude: return "Uses the desktopglass-index skill"
        case .apple: return "Runs on this Mac with Apple Intelligence · no account, no tokens"
        case .mlx: return "Runs on this Mac's GPU with mlx-lm · models download from Hugging Face once"
        case .ollama, .lmstudio, .llamacpp: return "Runs on this Mac through a local server · no account, no tokens"
        default: return "Gets SKILL.md as its prompt"
        }
    }

    static var current: Harness {
        get { Harness(rawValue: UserDefaults.standard.string(forKey: "harness") ?? "") ?? .claude }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: "harness") }
    }

    var modelKey: String { "model.\(rawValue)" }

    /// Model for this backend; empty means its own default.
    var model: String {
        get {
            if let saved = UserDefaults.standard.string(forKey: modelKey) { return saved }
            if self == .claude, let legacy = UserDefaults.standard.string(forKey: "model") { return legacy }
            return defaultModel
        }
        nonmutating set { UserDefaults.standard.set(newValue, forKey: modelKey) }
    }

    /// Server address for Ollama / LM Studio / llama.cpp.
    var endpoint: String {
        get { UserDefaults.standard.string(forKey: "endpoint.\(rawValue)") ?? defaultEndpoint ?? "" }
        nonmutating set { UserDefaults.standard.set(newValue, forKey: "endpoint.\(rawValue)") }
    }

    /// Only letters, digits and . _ / : - (it's passed through a shell).
    static func sanitizeModel(_ s: String) -> String {
        String(s.filter { $0.isLetter || $0.isNumber || "._/:-".contains($0) })
    }

    static func sanitizeEndpoint(_ s: String) -> String {
        let t = s.trimmingCharacters(in: .whitespaces)
        guard let u = URL(string: t), let scheme = u.scheme, scheme == "http" || scheme == "https", u.host != nil else { return "" }
        return t
    }

    /// Interactive command for the IDE terminal (nil = not usable interactively).
    /// Model used when this runs in the IDE terminal (chosen in the launch pane; separate from re-indexing).
    var terminalModel: String {
        get { UserDefaults.standard.string(forKey: "terminalModel.\(rawValue)") ?? (isLocalChat ? "" : model) }
        nonmutating set { UserDefaults.standard.set(newValue, forKey: "terminalModel.\(rawValue)") }
    }

    /// Plain chat with a local model (no file access), as opposed to a coding agent.
    var isLocalChat: Bool { [.ollama, .mlx, .lmstudio].contains(self) }

    /// Shows the model picker before the terminal starts. Agent CLIs start right away and use their own
    /// model setting; on-device models (and API backends, when they're added) need a model chosen first.
    var needsLaunchPane: Bool { isLocalChat }

    /// The executable to run in a terminal, or nil if this backend has no interactive mode.
    func terminalBinary(_ binary: String) -> String? {
        switch self {
        case .apple, .llamacpp: return nil
        case .ollama:   // the Ollama app bundles its CLI
            let cli = binary.hasSuffix(".app") ? binary + "/Contents/Resources/ollama" : binary
            return FileManager.default.isExecutableFile(atPath: cli) ? cli : nil
        case .lmstudio: return binary.hasSuffix(".app") ? nil : binary
        default: return binary
        }
    }

    func canRunInTerminal(binary: String) -> Bool { terminalBinary(binary) != nil }

    /// Interactive command for the IDE terminal, with the chosen model — or picking up an earlier conversation.
    func terminalCommand(binary: String, model chosen: String? = nil, resume: ConversationResume? = nil) -> String? {
        guard let bin = terminalBinary(binary) else { return nil }
        let q = { (s: String) in "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'" }
        if let resume {
            guard let args = resumeArguments(resume) else { return nil }
            return ([q(bin)] + args.map(q)).joined(separator: " ")
        }
        let m = Harness.sanitizeModel(chosen ?? terminalModel)
        let flag = { (name: String) in m.isEmpty ? "" : " \(name) " + q(m) }
        switch self {
        case .claude, .copilot, .antigravity: return q(bin) + flag("--model")
        case .codex, .gemini, .cursor, .opencode: return q(bin) + flag("-m")
        case .ollama: return q(bin) + " run " + q(m.isEmpty ? "llama3.2" : m)
        case .mlx: return q(bin) + " -m mlx_lm.chat" + flag("--model")
        case .lmstudio: return q(bin) + " chat" + (m.isEmpty ? "" : " " + q(m))
        case .apple, .llamacpp: return nil
        }
    }
}

// MARK: - Detection

/// What's installed, as the engine finds it (`backends/detect`: the login-shell PATH, app bundles, mlx-lm).
@MainActor
final class HarnessDetector: ObservableObject {
    @Published private(set) var installed: [Harness: String] = [:]
    @Published private(set) var detecting = false

    func detect() {
        guard !detecting, EngineClient.binary != nil else { return }
        detecting = true
        let params: [String: Any] = ["appleAvailable": FoundationModelsCLI.isAvailable,
                                     "glassBin": Bundle.main.executablePath ?? ""]
        Task {
            let engine = EngineClient.shared
            await engine.waitUntilReady()
            let result = try? await engine.call("backends/detect", params) as? [String: Any]
            var found: [Harness: String] = [:]
            for (k, v) in result?["installed"] as? [String: Any] ?? [:] {
                if let h = Harness(rawValue: k), let path = v as? String { found[h] = path }
            }
            installed = found
            detecting = false
        }
    }
}

// MARK: - Models

extension Harness {
    /// Suggested models when the live list can't be read (from the engine).
    @MainActor var presetModels: [String] { ModelCatalog.shared.presets[self] ?? [] }

    /// Whether the backend can list its own models, and whether that needs its CLI.
    var canListModels: Bool { listsWithCLI || [.mlx, .ollama, .lmstudio, .llamacpp].contains(self) }
    var listsWithCLI: Bool { [.cursor, .antigravity, .opencode, .copilot].contains(self) }

    var listDescription: String {
        switch self {
        case .cursor: return "from `cursor-agent models`"
        case .antigravity: return "from `agy models`"
        case .opencode: return "from `opencode models`"
        case .copilot: return "from `copilot --help`"
        case .ollama: return "pulled in Ollama"
        case .lmstudio: return "loaded in LM Studio"
        case .llamacpp: return "served by llama.cpp"
        case .mlx: return "downloaded to the Hugging Face cache"
        default: return ""
        }
    }
}

/// Model lists per backend, read by the engine (`models/list`): live where possible, otherwise its presets.
@MainActor
final class ModelCatalog: ObservableObject {
    static let shared = ModelCatalog()

    @Published private(set) var fetched: [Harness: [String]] = [:]
    @Published private(set) var loading: Set<Harness> = []
    @Published private(set) var presets: [Harness: [String]] = [:]

    init() {
        EngineClient.shared.on("engine/ready") { [weak self] _ in Task { await self?.loadPresets() } }
    }

    func models(for h: Harness) -> [String] { fetched[h] ?? presets[h] ?? [] }
    func isLive(_ h: Harness) -> Bool { fetched[h] != nil }

    func load(_ h: Harness, binary: String?, force: Bool = false) {
        guard force || fetched[h] == nil, !loading.contains(h), EngineClient.binary != nil else { return }
        loading.insert(h)
        var params: [String: Any] = ["harness": h.rawValue, "force": force]
        if let binary { params["binary"] = binary }
        Task {
            let engine = EngineClient.shared
            await engine.waitUntilReady()
            if presets.isEmpty { await loadPresets() }
            let r = try? await engine.call("models/list", params) as? [String: Any]
            loading.remove(h)
            if r?["live"] as? Bool == true, let list = r?["models"] as? [String], !list.isEmpty { fetched[h] = list }
        }
    }

    private func loadPresets() async {
        guard let r = try? await EngineClient.shared.call("models/presets") as? [String: Any] else { return }
        var out: [Harness: [String]] = [:]
        for (k, v) in r { if let h = Harness(rawValue: k), let list = v as? [String] { out[h] = list } }
        presets = out
    }
}


// MARK: - Unattended runs (opt-in)

extension Harness {
    /// Agent CLIs approve their own actions in headless runs, so each one needs the user's opt-in.
    /// On-device backends never run tools (`dg reindex` writes the files), so they need none.
    var unattendedAllowed: Bool {
        get { isLocal || UserDefaults.standard.bool(forKey: "unattended.\(rawValue)") }
        nonmutating set { UserDefaults.standard.set(newValue, forKey: "unattended.\(rawValue)") }
    }

    /// Mirrors unattended_permissions in dg-index (harness.rs).
    var unattendedPermissions: String {
        switch self {
        case .claude: return "Tool allow-list: Read, Write, Edit and dg only; edits are auto-accepted."
        case .codex: return "Codex's own sandbox (--full-auto): writes only inside your Desktop; its commands have no network."
        case .gemini: return "Auto-approves every action (--approval-mode yolo)."
        case .antigravity: return "Auto-approves every tool call (--dangerously-skip-permissions)."
        case .cursor: return "Auto-approves commands (--force)."
        case .copilot: return "Allowed tools: read, write and dg."
        case .opencode: return "Auto-approves tool calls (opencode run)."
        default: return "Runs on this Mac; DesktopGlass writes the files itself."
        }
    }

    static var sandboxUnattended: Bool {
        get { UserDefaults.standard.object(forKey: "sandboxUnattended") as? Bool ?? false }   // optional extra layer
        set { UserDefaults.standard.set(newValue, forKey: "sandboxUnattended") }
    }
}

enum Consent {
    static let sandboxSummary = "DesktopGlass's sandbox lets it write only description.md files and _Organize Log on your Desktop (plus the CLI's own session and cache folders), blocks changes to CLI settings, hooks and skills, and blocks reading SSH, cloud and GPG keys. It can still use the network to reach its model."

    /// Asks before letting an agent CLI run unattended. Returns true if allowed (and saves it).
    @MainActor
    static func ask(_ h: Harness) -> Bool {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = "Let \(h.label) re-index without asking each time?"
        var info = "Re-indexing runs \(h.label) headless — nobody approves its individual steps.\n\n\(h.unattendedPermissions)\n\n"
        if h == .codex { info += "Codex applies its own sandbox." }
        else if Harness.sandboxUnattended { info += sandboxSummary }
        else { info += "\(h.label)'s own permission controls apply. For an extra layer, turn on DesktopGlass's sandbox in Settings → Re-indexing." }
        info += "\n\nYou can turn this off again in Settings → Re-indexing."
        alert.informativeText = info
        alert.addButton(withTitle: "Allow")
        alert.addButton(withTitle: "Cancel")
        alert.layout()
        alert.window.level = .popUpMenu                 // above the floating overlay
        NSApp.activate(ignoringOtherApps: true)
        let ok = alert.runModal() == .alertFirstButtonReturn
        if ok { h.unattendedAllowed = true }
        return ok
    }
}
