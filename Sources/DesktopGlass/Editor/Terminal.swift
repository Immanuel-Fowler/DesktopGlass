import AppKit
import SwiftTerm
import SwiftUI

/// What the IDE terminal runs: a plain shell, or an interactive CLI/model that drops back to the shell on exit.
enum TerminalProgram: Hashable {
    case shell
    case harness(Harness)

    var label: String {
        switch self {
        case .shell: return "Shell"
        case .harness(let h): return h.label
        }
    }

    var symbol: String {
        switch self {
        case .shell: return "terminal"
        case .harness(let h): return h.isLocal ? "cpu" : "sparkles"
        }
    }

    var storageValue: String {
        switch self {
        case .shell: return "shell"
        case .harness(let h): return h.rawValue
        }
    }

    /// Settings → "IDE terminal opens": "auto" (the re-index CLI), "shell", or a backend's raw value.
    static var preference: String {
        get { UserDefaults.standard.string(forKey: "terminalProgram") ?? "auto" }
        set { UserDefaults.standard.set(newValue, forKey: "terminalProgram") }
    }
}

/// SwiftTerm's view, adjusted for glass windows: its transparent background would otherwise let a click drag the
/// window instead of reaching the terminal, so it never got keyboard focus.
final class GlassTerminalView: LocalProcessTerminalView {
    override var mouseDownCanMoveWindow: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        super.mouseDown(with: event)
    }

    /// Set by TerminalSession.start; the view takes focus the moment it lands in a window.
    var wantsFocus = false

    func focus() {
        guard let window else { wantsFocus = true; return }
        wantsFocus = false
        window.makeKey()
        window.makeFirstResponder(self)
        needsDisplay = true
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil, wantsFocus { DispatchQueue.main.async { self.focus() } }
    }
}

@MainActor
final class TerminalSession: NSObject, ObservableObject, LocalProcessTerminalViewDelegate {
    @Published private(set) var view: GlassTerminalView
    @Published private(set) var program: TerminalProgram = .shell
    @Published private(set) var title = "Terminal"
    @Published private(set) var model = ""
    @Published private(set) var running = false
    /// Set while the terminal is on an earlier conversation (so Restart picks it up again rather than starting over).
    @Published private(set) var resumed: ConversationResume?
    private(set) var directory: URL?

    override init() {
        view = TerminalSession.makeView()
        super.init()
    }

    private static func makeView() -> GlassTerminalView {
        let v = GlassTerminalView(frame: NSRect(x: 0, y: 0, width: 480, height: 400))
        v.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
        v.nativeBackgroundColor = NSColor.textBackgroundColor.withAlphaComponent(0.25)
        v.nativeForegroundColor = NSColor.labelColor
        return v
    }

    /// The program the panel should open with, given what's installed.
    static func defaultProgram(installed: [Harness: String]) -> TerminalProgram {
        let pref = TerminalProgram.preference
        if pref == "shell" { return .shell }
        if let h = Harness(rawValue: pref), let bin = installed[h], h.canRunInTerminal(binary: bin) { return .harness(h) }
        let current = Harness.current
        if !current.isLocal, installed[current] != nil { return .harness(current) }
        if let agent = Harness.agents.first(where: { installed[$0] != nil }) { return .harness(agent) }
        return .shell
    }

    /// Starts (or restarts) the terminal in `dir`. Any running program is ended first.
    func start(_ program: TerminalProgram, in dir: URL, installed: [Harness: String], model chosen: String = "",
               resume: ConversationResume? = nil) {
        stop()
        model = resume == nil ? chosen : ""
        resumed = resume
        let fresh = TerminalSession.makeView()
        fresh.processDelegate = self
        view = fresh
        self.program = program
        directory = dir

        let shell = ProcessInfo.processInfo.environment["SHELL"].flatMap { $0.isEmpty ? nil : $0 } ?? "/bin/zsh"
        let q = { (s: String) in "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'" }
        var script = "cd \(q(dir.path)) || exit 1; "
        if case .harness(let h) = program, let bin = installed[h],
           let command = h.terminalCommand(binary: bin, model: chosen, resume: resume) {
            let banner = h.label + (resume != nil ? " · earlier conversation" : chosen.isEmpty ? "" : " · " + chosen)
            script += "printf '\\033[2m%s · %s\\033[0m\\n\\n' \(q(banner)) \(q(dir.lastPathComponent)); \(command); "
        }
        script += "exec \(q(shell)) -l"

        var env = ProcessInfo.processInfo.environment
        env["TERM"] = "xterm-256color"
        env["COLORTERM"] = "truecolor"
        env["TERM_PROGRAM"] = "DesktopGlass"
        env["LANG"] = env["LANG"] ?? "en_US.UTF-8"
        let envList = env.map { "\($0.key)=\($0.value)" }

        fresh.startProcess(executable: shell, args: ["-l", "-c", script], environment: envList, execName: nil)
        running = true
        title = program.label
        focusSoon()
    }

    /// Gives the terminal keyboard focus once it's on screen.
    func focusSoon() {
        let v = view
        v.wantsFocus = true
        for delay in [0.05, 0.25, 0.6] {                 // the panel may still be sliding in
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                if v.window?.firstResponder !== v { v.focus() }
            }
        }
    }

    /// Types `cd <dir>` into the running shell (doesn't interrupt anything already typed beyond a line kill).
    func changeDirectory(to dir: URL) {
        guard running else { return }
        let q = "'" + dir.path.replacingOccurrences(of: "'", with: "'\\''") + "'"
        view.send(txt: "\u{15}cd \(q)\r")   // ⌃U clears the current line first
        directory = dir
    }

    func stop() {
        if running { view.process?.terminate() }
        running = false
    }

    // MARK: LocalProcessTerminalViewDelegate (called from SwiftTerm; hop to the main actor)

    nonisolated func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) {}

    nonisolated func setTerminalTitle(source: LocalProcessTerminalView, title: String) {
        DispatchQueue.main.async { if !title.isEmpty { self.title = title } }
    }

    nonisolated func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}

    nonisolated func processTerminated(source: TerminalView, exitCode: Int32?) {
        DispatchQueue.main.async {
            self.running = false
            self.title = "\(self.program.label) — exited"
        }
    }
}

@MainActor
struct TerminalHost: NSViewRepresentable {
    let view: GlassTerminalView

    func makeNSView(context: Context) -> NSView {
        let container = NSView()
        attach(to: container)
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        if container.subviews.first !== view { attach(to: container) }
    }

    @MainActor
    private func attach(to container: NSView) {
        container.subviews.forEach { $0.removeFromSuperview() }
        view.removeFromSuperview()
        view.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(view)
        NSLayoutConstraint.activate([
            view.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 6),
            view.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -6),
            view.topAnchor.constraint(equalTo: container.topAnchor, constant: 4),
            view.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -4),
        ])
    }
}

/// Where a docked terminal lives: the IDE (its open folder) or the desktop overlay (the Desktop).
@MainActor
protocol TerminalDock: ObservableObject {
    /// The folder agents and shells start in; nil shows "Open a folder" instead of a terminal.
    var terminalFolder: URL? { get }
    var terminalFolderName: String { get }
    /// Expanded panel vs. the slim collapsed rail.
    var showTerminal: Bool { get set }
    /// The folder on screen, when it can differ from `terminalFolder` (offers a `cd` there).
    var browsingFolder: URL? { get }
}

extension TerminalDock {
    var browsingFolder: URL? { nil }
}

struct TerminalPanel<Dock: TerminalDock>: View {
    @ObservedObject var terminal: TerminalSession
    @ObservedObject var model: Dock
    @ObservedObject var detector: HarnessDetector
    @ObservedObject private var catalog = ModelCatalog.shared
    /// A CLI or model waiting for the user to pick a model and press Start.
    @State private var pending: Harness?
    @State private var pendingModel = ""
    @State private var showHistory = false

    /// Installed agent CLIs that can pick up an earlier conversation.
    private var resumable: [Harness] { Harness.agents.filter { detector.installed[$0] != nil && $0.canResume } }

    private var historyDefault: Harness? {
        if case .harness(let h) = terminal.program, resumable.contains(h) { return h }
        return resumable.first
    }

    private var choices: [TerminalProgram] {
        [.shell] + Harness.allCases.compactMap { h in
            guard let bin = detector.installed[h], h.canRunInTerminal(binary: bin) else { return nil }
            return .harness(h)
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Menu {
                    ForEach(choices, id: \.self) { p in
                        Button { choose(p) } label: { Label(p.label, systemImage: p.symbol) }
                    }
                } label: {
                    Label(headerTitle, systemImage: (pending.map { TerminalProgram.harness($0) } ?? terminal.program).symbol)
                        .font(.system(size: 12, weight: .semibold))
                }
                .menuStyle(.borderlessButton)
                .fixedSize()
                .help("Choose what runs in this terminal")
                Circle().fill(terminal.running && pending == nil ? Color.green : Color.secondary.opacity(0.4)).frame(width: 6, height: 6)
                Spacer()
                if let browsing = model.browsingFolder, terminal.running, pending == nil, terminal.program == .shell,
                   terminal.directory?.standardizedFileURL != browsing.standardizedFileURL {
                    Button { terminal.changeDirectory(to: browsing) } label: {
                        Label("cd \(browsing.lastPathComponent)", systemImage: "arrow.turn.down.right")
                            .font(.system(size: 11)).lineLimit(1)
                    }
                    .buttonStyle(.plain).foregroundStyle(Color.accentColor)
                    .help("Move the shell to the folder you're browsing")
                }
                if let first = historyDefault, let dir = model.terminalFolder {
                    Button { showHistory.toggle() } label: { Image(systemName: "clock.arrow.circlepath") }
                        .buttonStyle(.plain).foregroundStyle(.secondary).help("Past conversations in this folder")
                        .popover(isPresented: $showHistory, arrowEdge: .bottom) {
                            ConversationsPopover(folder: dir, harnesses: resumable, installed: detector.installed,
                                                 initial: first) { h, r in resume(h, r) }
                        }
                }
                Button { restart() } label: { Image(systemName: "arrow.counterclockwise") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Restart")
                Button { model.showTerminal = false } label: { Image(systemName: "chevron.right.2") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Collapse terminal (⌃`)")
            }
            .padding(.horizontal, 12).frame(height: 36)
            Divider().opacity(0.2)
            if model.terminalFolder == nil {
                Text("Open a folder to start a terminal there").foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let h = pending {
                launchPane(h)
            } else {
                TerminalHost(view: terminal.view)
                    .background(Color.black.opacity(0.12))
            }
        }
        .onAppear { ensureStarted() }
        .onChange(of: model.terminalFolder) { _ in ensureStarted(restartIfMoved: true) }
        .onChange(of: detector.detecting) { stillDetecting in if !stillDetecting { ensureStarted() } }
    }

    private var headerTitle: String {
        if let h = pending { return h.label }
        return terminal.program.label + (terminal.model.isEmpty ? "" : " · " + terminal.model)
    }

    // MARK: Launch pane

    /// Models already on this Mac (local backends), so nothing downloads unless you pick a "downloads" one.
    private func installedModels(_ h: Harness) -> [String] {
        h.isLocalChat ? (catalog.isLive(h) ? catalog.models(for: h) : []) : []
    }

    private func modelOptions(_ h: Harness) -> (installed: [String], other: [String]) {
        if h.isLocalChat {
            let have = installedModels(h)
            return (have, h.presetModels.filter { !have.contains($0) })
        }
        return ([], catalog.models(for: h))
    }

    private func needsDownload(_ h: Harness) -> Bool {
        guard h.isLocalChat else { return false }
        let effective = (h == .ollama && pendingModel.isEmpty) ? "llama3.2" : pendingModel
        if effective.isEmpty { return !catalog.isLive(h) }          // mlx default
        return !installedModels(h).contains(where: { $0 == effective || $0 == effective + ":latest" })
    }

    private func launchPane(_ h: Harness) -> some View {
        let options = modelOptions(h)
        return VStack(alignment: .leading, spacing: 12) {
            Label(h.label, systemImage: h.isLocalChat ? "cpu" : "sparkles").font(.system(size: 15, weight: .semibold))
            Text(h.isLocalChat
                 ? "Chat with a model running on this Mac. It can't read or change your files — paste in what it should see."
                 : "Coding agent, working in \(model.terminalFolderName). It can read and edit files there.")
                .font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)

            HStack(spacing: 8) {
                Text("Model").font(.system(size: 12, weight: .medium)).frame(width: 46, alignment: .leading)
                Picker("", selection: $pendingModel) {
                    if !h.isLocalChat { Text("Default").tag("") }
                    if h == .mlx && !options.installed.contains("") { Text("mlx-lm default (downloads)").tag("") }
                    if !options.installed.isEmpty {
                        Section("On this Mac") { ForEach(options.installed, id: \.self) { Text($0).tag($0) } }
                    }
                    if !options.other.isEmpty {
                        Section(h.isLocalChat ? "Downloads first" : "Models") {
                            ForEach(options.other, id: \.self) { Text($0).tag($0) }
                        }
                    }
                    if !pendingModel.isEmpty, !options.installed.contains(pendingModel), !options.other.contains(pendingModel) {
                        Text(pendingModel).tag(pendingModel)
                    }
                }
                .labelsHidden()
                Button {
                    catalog.load(h, binary: detector.installed[h], force: true)
                } label: {
                    Group {
                        if catalog.loading.contains(h) { ProgressView().controlSize(.small) } else { Image(systemName: "arrow.clockwise") }
                    }
                    .frame(width: 20, height: 20)
                }
                .buttonStyle(.plain).foregroundStyle(.secondary).help("Reload the model list")
            }
            TextField("Or type a model ID", text: $pendingModel)
                .textFieldStyle(.roundedBorder).font(.system(size: 11.5, design: .monospaced))

            if h == .ollama && !catalog.isLive(.ollama) && !catalog.loading.contains(.ollama) {
                warning("Couldn't list Ollama's models — make sure the Ollama app is running.")
            }
            if needsDownload(h) {
                warning("This model isn't on your Mac yet. Starting will download it first (often a few GB).")
            }

            HStack {
                Button("Open a Shell Instead") { choose(.shell) }.buttonStyle(.plain).foregroundStyle(.secondary)
                    .font(.system(size: 12))
                Spacer()
                Button {
                    start(h)
                } label: {
                    Label("Start", systemImage: "play.fill").font(.system(size: 12.5, weight: .semibold))
                        .padding(.horizontal, 14).padding(.vertical, 6)
                }
                .buttonStyle(.plain).glassPill()
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(18)
        .frame(maxWidth: 380)
        .glassCard(cornerRadius: 16)
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear { catalog.load(h, binary: detector.installed[h]) }
        .onChange(of: installedModels(h)) { list in
            // the list arrived after the pane opened: prefer a model that's already here (unless one was saved before)
            let saved = UserDefaults.standard.string(forKey: "terminalModel.\(h.rawValue)") ?? ""
            if saved.isEmpty, let first = list.first, !list.contains(pendingModel) { pendingModel = first }
        }
    }

    private func warning(_ text: String) -> some View {
        Label(text, systemImage: "exclamationmark.triangle.fill")
            .font(.system(size: 11.5)).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true)
    }

    // MARK: Starting

    /// Shells start right away; CLIs and models show the launch pane so a model can be picked first.
    private func choose(_ p: TerminalProgram) {
        guard let dir = model.terminalFolder else { return }
        switch p {
        case .shell:
            pending = nil
            terminal.start(.shell, in: dir, installed: detector.installed)
        case .harness(let h) where !h.needsLaunchPane:
            pending = nil
            terminal.start(.harness(h), in: dir, installed: detector.installed)   // the CLI's own default model
        case .harness(let h):
            terminal.stop()
            catalog.load(h, binary: detector.installed[h])
            var m = h.terminalModel
            if h.isLocalChat, m.isEmpty, let first = installedModels(h).first { m = first }
            if h == .ollama, m.isEmpty { m = "llama3.2" }               // `ollama run` needs a model name
            pendingModel = m
            pending = h
        }
    }

    private func start(_ h: Harness) {
        guard let dir = model.terminalFolder else { return }
        let m = Harness.sanitizeModel(pendingModel)
        h.terminalModel = m
        pending = nil
        terminal.start(.harness(h), in: dir, installed: detector.installed, model: m)
    }

    private func restart() {
        guard let dir = model.terminalFolder else { return }
        if pending != nil { return }
        terminal.start(terminal.program, in: dir, installed: detector.installed, model: terminal.model, resume: terminal.resumed)
    }

    private func resume(_ h: Harness, _ r: ConversationResume) {
        guard let dir = model.terminalFolder else { return }
        showHistory = false
        pending = nil
        terminal.start(.harness(h), in: dir, installed: detector.installed, resume: r)
    }

    private func ensureStarted(restartIfMoved: Bool = false) {
        guard let dir = model.terminalFolder, !detector.detecting, pending == nil else { return }
        let moved = terminal.directory.map { $0.standardizedFileURL != dir.standardizedFileURL } ?? true
        guard !terminal.running || (restartIfMoved && moved) else { return }
        if terminal.directory != nil, case .harness(let h) = terminal.program {
            terminal.start(.harness(h), in: dir, installed: detector.installed, model: terminal.model)   // same choice, new folder
        } else {
            choose(terminal.directory == nil ? TerminalSession.defaultProgram(installed: detector.installed) : terminal.program)
        }
    }
}

/// Earlier conversations an agent CLI saved for this folder; choosing one resumes it in the terminal.
struct ConversationsPopover: View {
    let folder: URL
    let harnesses: [Harness]
    let installed: [Harness: String]
    let resume: (Harness, ConversationResume) -> Void
    @State private var harness: Harness
    @State private var items: [PastConversation] = []
    @State private var loading = false
    @State private var generation = 0

    init(folder: URL, harnesses: [Harness], installed: [Harness: String], initial: Harness,
         resume: @escaping (Harness, ConversationResume) -> Void) {
        self.folder = folder
        self.harnesses = harnesses
        self.installed = installed
        self.resume = resume
        _harness = State(initialValue: initial)
    }

    private static let relative: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter(); f.unitsStyle = .short; return f
    }()

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Past Conversations").font(.system(size: 13, weight: .semibold))
                    Text(folder.lastPathComponent).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer()
                if harnesses.count > 1 {
                    Picker("", selection: $harness) {
                        ForEach(harnesses, id: \.self) { Text($0.label).tag($0) }
                    }
                    .labelsHidden().fixedSize()
                }
            }
            HStack(spacing: 8) {
                Button("Continue Last") { resume(harness, .last) }
                if harness.hasResumePicker {
                    Button("Choose in \(harness.label)…") { resume(harness, .picker) }
                }
            }
            .controlSize(.small)
            Divider()
            if loading {
                HStack { ProgressView().controlSize(.small); Text("Reading history…").foregroundStyle(.secondary) }
                    .font(.system(size: 12))
            } else if items.isEmpty {
                Text(emptyText).font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(items) { c in
                            Button { resume(harness, .session(c.id)) } label: { row(c) }.buttonStyle(.plain)
                        }
                    }
                }
                .frame(maxHeight: 340)
            }
        }
        .padding(14)
        .frame(width: 400)
        .onAppear(perform: load)
        .onChange(of: harness) { _ in load() }
    }

    private func row(_ c: PastConversation) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(c.title).font(.system(size: 12.5)).lineLimit(2).frame(maxWidth: .infinity, alignment: .leading)
            Text(c.updated == .distantPast ? "" : Self.relative.localizedString(for: c.updated, relativeTo: Date()))
                .font(.system(size: 11)).foregroundStyle(.tertiary)
        }
        .padding(.vertical, 5).padding(.horizontal, 6)
        .contentShape(Rectangle())
        .help("Resume in \(harness.label)")
    }

    private var emptyText: String {
        if harness == .cursor {
            return "Cursor keeps its chats in its own store. Use Continue Last, or Choose in Cursor CLI… to pick one."
        }
        return "No saved \(harness.label) conversations for this folder yet."
            + (harness.hasResumePicker ? " Choose in \(harness.label)… shows its full history." : "")
    }

    private func load() {
        generation += 1
        let ticket = generation, h = harness, dir = folder, binary = installed[h]
        loading = true
        DispatchQueue.global(qos: .userInitiated).async {
            let found = ConversationHistory.list(h, folder: dir, binary: binary)
            DispatchQueue.main.async {
                guard ticket == generation else { return }      // the user switched CLIs meanwhile
                items = found
                loading = false
            }
        }
    }
}

/// What's left of the IDE terminal when it's collapsed: a slim strip that expands it again.
struct CollapsedTerminalRail<Dock: TerminalDock>: View {
    @ObservedObject var terminal: TerminalSession
    @ObservedObject var model: Dock
    @State private var hover = false

    var body: some View {
        Button { model.showTerminal = true } label: {
            VStack(spacing: 10) {
                Image(systemName: "chevron.left.2").font(.system(size: 10, weight: .bold))
                Image(systemName: "terminal").font(.system(size: 13))
                Circle().fill(terminal.running ? Color.green : Color.secondary.opacity(0.35)).frame(width: 5, height: 5)
                Spacer()
            }
            .padding(.top, 12)
            .frame(width: 28)
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
            .background(Color.white.opacity(hover ? 0.08 : 0.03))
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
        .onHover { hover = $0 }
        .help("Show terminal (⌃`)")
    }
}
