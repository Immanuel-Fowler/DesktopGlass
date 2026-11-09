import AppKit
import Carbon
import ServiceManagement
import SwiftUI

enum HotKeyChoice: String, CaseIterable {
    case optCmdD = "⌥⌘D", ctrlOptD = "⌃⌥D", ctrlSpace = "⌃Space", optSpace = "⌥Space", shiftCmdSpace = "⇧⌘Space"

    var keyCode: Int {
        switch self {
        case .optCmdD, .ctrlOptD: return kVK_ANSI_D
        default: return kVK_Space
        }
    }

    var modifiers: Int {
        switch self {
        case .optCmdD: return cmdKey | optionKey
        case .ctrlOptD: return controlKey | optionKey
        case .ctrlSpace: return controlKey
        case .optSpace: return optionKey
        case .shiftCmdSpace: return cmdKey | shiftKey
        }
    }

    static var current: HotKeyChoice {
        get { HotKeyChoice(rawValue: UserDefaults.standard.string(forKey: "hotkey") ?? "") ?? .optCmdD }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: "hotkey") }
    }
}

struct SettingsView: View {
    @ObservedObject var store: DesktopStore
    @ObservedObject var reindexer: Reindexer
    @ObservedObject var detector: HarnessDetector
    @ObservedObject private var catalog = ModelCatalog.shared
    @State private var customModel = false
    private static let customTag = "__custom__"

    @AppStorage("hotkey") private var hotkey = HotKeyChoice.optCmdD.rawValue
    @AppStorage("editor") private var editor = Editor.vscode.rawValue
    @AppStorage("schedule") private var schedule = Schedule.daily.rawValue
    @AppStorage("scheduleHour") private var scheduleHour = 3
    @AppStorage("harness") private var harness = Harness.claude.rawValue
    @AppStorage("terminalProgram") private var terminalProgram = "auto"
    @State private var modelText = Harness.current.model
    @State private var endpointText = Harness.current.endpoint
    @State private var consentTick = 0
    @State private var launchAtLogin = SMAppService.mainApp.status == .enabled
    @AppStorage("island.show") private var showIsland = true
    @AppStorage("island.commandD") private var islandCommandD = true
    @AppStorage("island.swipe") private var islandSwipe = true
    @AppStorage("island.hideDesktopIcons") private var hideDesktopIcons = true
    @ObservedObject private var wallpaper = WallpaperStore.shared
    @State private var accessibilityOK = DesktopInput.isTrusted

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text("Settings").font(.system(size: 26, weight: .semibold))
                    Spacer()
                    Button("Done") { store.showingSettings = false }
                        .buttonStyle(.plain)
                        .padding(.horizontal, 14).padding(.vertical, 6)
                        .glassPill()
                }

                section("Island", symbol: "capsule.portrait") {
                    row("Show the island", "A glass pill at the top of your desktop that opens DesktopGlass") {
                        Toggle("", isOn: $showIsland).labelsHidden().toggleStyle(.switch)
                    }
                    row("⌘D opens the island", "Works from any app; while this is on, other apps don't get ⌘D") {
                        Toggle("", isOn: $islandCommandD).labelsHidden().toggleStyle(.switch)
                    }
                    row("Swipe down from the top", "Two fingers down on the trackpad at the menu bar or over the island") {
                        Toggle("", isOn: $islandSwipe).labelsHidden().toggleStyle(.switch)
                    }
                    row("DesktopGlass mode", "Hides Finder's desktop icons (files stay on the Desktop and show here), shows your wallpaper and auto-hides the Dock") {
                        Toggle("", isOn: $hideDesktopIcons).labelsHidden().toggleStyle(.switch)
                    }
                    row("Wallpaper", wallpaper.current?.lastPathComponent ?? "None — an image or looping video, from a file or a link") {
                        HStack(spacing: 6) {
                            Button("Choose…") { wallpaper.chooseFile() }
                            if wallpaper.current != nil { Button("Remove") { wallpaper.clear() } }
                        }
                    }
                    row("Fallback wallpaper", wallpaper.fallback?.lastPathComponent
                        ?? "None — an image or video shown when a YouTube wallpaper can't play (offline, loading, blocked)") {
                        HStack(spacing: 6) {
                            Button("Choose…") { wallpaper.chooseFallback() }
                            if wallpaper.fallback != nil { Button("Remove") { wallpaper.clearFallback() } }
                        }
                    }
                    if islandCommandD || islandSwipe {
                        row("Accessibility", accessibilityOK
                            ? "Allowed — ⌘D and the top-edge swipe are on"
                            : "Needed for ⌘D and the top-edge swipe (clicking the island always works)") {
                            if accessibilityOK {
                                Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                            } else {
                                Button("Allow…") { DesktopInput.requestAccess(); DesktopInput.openAccessibilitySettings() }
                            }
                        }
                        .onReceive(Timer.publish(every: 2, on: .main, in: .common).autoconnect()) { _ in
                            accessibilityOK = DesktopInput.isTrusted
                        }
                    }
                    row("Size", "⌘= and ⌘- while the island is open: Standard, Medium, Large, Full Screen") {
                        Text(IslandPrefs.size.label).foregroundStyle(.secondary)
                    }
                }

                section("General", symbol: "gearshape") {
                    row("Shortcut", "Also opens and closes the island, from any app") {
                        Picker("", selection: $hotkey) {
                            ForEach(HotKeyChoice.allCases, id: \.rawValue) { Text($0.rawValue).tag($0.rawValue) }
                        }
                        .labelsHidden().frame(width: 140)
                    }
                    row("Launch at login", "Start DesktopGlass when you log in") {
                        Toggle("", isOn: $launchAtLogin).labelsHidden().toggleStyle(.switch)
                            .onChange(of: launchAtLogin) { on in
                                do {
                                    if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
                                } catch {
                                    NSSound.beep()
                                    launchAtLogin = SMAppService.mainApp.status == .enabled
                                }
                            }
                    }
                    row("IDE terminal opens", "What runs in the editor's terminal panel when you open a folder") {
                        Picker("", selection: $terminalProgram) {
                            Text("Default CLI (\(Harness.current.isLocal ? "first agent CLI" : Harness.current.label))").tag("auto")
                            Text("Shell").tag("shell")
                            Divider()
                            ForEach(Harness.allCases.filter { h in
                                detector.installed[h].map { h.canRunInTerminal(binary: $0) } ?? false
                            }) { h in Text(h.label).tag(h.rawValue) }
                        }
                        .labelsHidden().frame(width: 220)
                    }
                    row("Open projects in", editorDetail) {
                        Picker("", selection: $editor) {
                            ForEach(Editor.allCases, id: \.rawValue) { Text($0.rawValue).tag($0.rawValue) }
                        }
                        .labelsHidden().frame(width: 200)
                    }
                }

                section("Re-indexing", symbol: "sparkles") {
                    row("Written by", harnessDetail) {
                        HStack(spacing: 8) {
                            Picker("", selection: $harness) {
                                Section("Agent CLIs") {
                                    ForEach(Harness.agents) { h in
                                        Text(h.label + (detector.installed[h] == nil ? " (not found)" : "")).tag(h.rawValue)
                                    }
                                }
                                Section("On this Mac (Apple silicon)") {
                                    ForEach(Harness.locals) { h in
                                        Text(h.label + (detector.installed[h] == nil ? " (not found)" : "")).tag(h.rawValue)
                                    }
                                }
                            }
                            .labelsHidden().frame(width: 220)
                            .onChange(of: harness) { raw in
                                let h = Harness(rawValue: raw) ?? .claude
                                if case .signedOut(let old)? = reindexer.problem, old != h { reindexer.checkAuth() }
                                modelText = h.model
                                endpointText = h.endpoint
                                customModel = false
                                catalog.load(h, binary: detector.installed[h])
                            }
                            Button { detector.detect() } label: {
                                Image(systemName: "arrow.clockwise").frame(width: 22, height: 22)
                            }
                            .buttonStyle(.plain).glassPill().help("Detect installed CLIs again")
                        }
                    }
                    if !selectedHarness.isLocal {
                        row("Unattended runs", unattendedDetail) {
                            Toggle("", isOn: Binding(
                                get: { _ = consentTick; return selectedHarness.unattendedAllowed },
                                set: { on in
                                    if on { _ = Consent.ask(selectedHarness) } else { selectedHarness.unattendedAllowed = false }
                                    consentTick += 1
                                }))
                            .labelsHidden().toggleStyle(.switch)
                        }
                        row("Extra sandbox", Harness.sandboxUnattended
                            ? "On — writes limited to description.md files and _Organize Log; credential folders unreadable"
                            : "Off — the CLI's own permission controls apply (optional extra layer)") {
                            Toggle("", isOn: Binding(
                                get: { _ = consentTick; return Harness.sandboxUnattended },
                                set: { Harness.sandboxUnattended = $0; consentTick += 1 }))
                            .labelsHidden().toggleStyle(.switch)
                            .disabled(selectedHarness == .codex)
                        }
                    }
                    row("Schedule", "Skipped automatically when nothing changed") {
                        Picker("", selection: $schedule) {
                            ForEach(Schedule.allCases, id: \.rawValue) { Text($0.label).tag($0.rawValue) }
                        }
                        .labelsHidden().frame(width: 200)
                        if schedule == Schedule.daily.rawValue {
                            Picker("", selection: $scheduleHour) {
                                ForEach(0..<24, id: \.self) { Text(Schedule.hourLabel($0)).tag($0) }
                            }
                            .labelsHidden().frame(width: 90)
                        }
                    }
                    row("Model", modelDetail) {
                        HStack(spacing: 8) {
                            Picker("", selection: modelSelection) {
                                Text("Default").tag("")
                                Divider()
                                ForEach(catalog.models(for: selectedHarness), id: \.self) { Text($0).tag($0) }
                                Divider()
                                Text("Custom…").tag(Self.customTag)
                            }
                            .labelsHidden().frame(width: 220)
                            .onChange(of: modelText) { text in selectedHarness.model = Harness.sanitizeModel(text) }
                            Button {
                                catalog.load(selectedHarness, binary: detector.installed[selectedHarness], force: true)
                            } label: {
                                Group {
                                    if catalog.loading.contains(selectedHarness) { ProgressView().controlSize(.small) }
                                    else { Image(systemName: "arrow.clockwise") }
                                }
                                .frame(width: 22, height: 22)
                            }
                            .buttonStyle(.plain).glassPill()
                            .disabled(!selectedHarness.canListModels || (selectedHarness.listsWithCLI && detector.installed[selectedHarness] == nil))
                            .help(!selectedHarness.canListModels ? "\(selectedHarness.label) can't list its models — use Custom… for others"
                                                                  : "Reload models \(selectedHarness.listDescription)")
                        }
                        .onAppear { catalog.load(selectedHarness, binary: detector.installed[selectedHarness]) }
                        .onChange(of: detector.installed[selectedHarness]) { path in catalog.load(selectedHarness, binary: path) }
                    }
                    if showCustomModel {
                        row("Custom model", "Exact model ID passed to \(selectedHarness.label)") {
                            TextField(selectedHarness.modelPlaceholder, text: $modelText)
                                .textFieldStyle(.roundedBorder)
                                .frame(width: 220)
                        }
                    }
                    if selectedHarness.usesServer {
                        row("Server", "Where \(selectedHarness.label) is listening") {
                            TextField(selectedHarness.defaultEndpoint ?? "", text: $endpointText)
                                .textFieldStyle(.roundedBorder).frame(width: 220)
                                .onSubmit { saveEndpoint() }
                                .onChange(of: endpointText) { _ in saveEndpoint() }
                        }
                    }
                    row(selectedHarness.isLocal ? "Status" : "Sign-in", signInDetail) {
                        HStack(spacing: 8) {
                            if case .signedOut? = reindexer.problem {
                                Button("Sign In…") { Actions.signIn(selectedHarness) }
                                    .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 5).glassPill()
                            }
                            Button {
                                reindexer.checkAuth()
                            } label: {
                                HStack(spacing: 6) {
                                    if reindexer.checkingAuth { ProgressView().controlSize(.small) }
                                    Text(reindexer.checkingAuth ? "Checking…" : (selectedHarness.isLocal ? "Check model" : "Check sign-in"))
                                }
                                .padding(.horizontal, 12).padding(.vertical, 5)
                            }
                            .buttonStyle(.plain).glassPill()
                            .disabled(reindexer.checkingAuth || reindexer.running
                                      || (detector.installed[selectedHarness] == nil && !selectedHarness.usesServer))
                        }
                    }
                    row("Last run", lastRunText) {
                        Button {
                            reindexer.run(reason: "settings")
                        } label: {
                            HStack(spacing: 6) {
                                if reindexer.running { ProgressView().controlSize(.small) }
                                Text(reindexer.running ? "Indexing…" : "Re-index Now")
                            }
                            .padding(.horizontal, 12).padding(.vertical, 5)
                        }
                        .buttonStyle(.plain).glassPill().disabled(reindexer.running)
                    }
                }

                section("MCP", symbol: "point.3.connected.trianglepath.dotted") {
                    MCPSection()
                }

                section("Accounts", symbol: "person.crop.circle.badge.checkmark") {
                    AccountsSection()
                }

                section("Indexing Preferences", symbol: "slider.horizontal.3") {
                    IndexPreferencesSection(store: store, reindexer: reindexer)
                }

                section(selectedHarness.label, symbol: "terminal") {
                    row("Questions", store.openQuestions > 0 ? "\(store.openQuestions) waiting for you" : "None waiting") {
                        Button("Answer in Terminal") { Actions.answerQuestions() }
                            .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 5).glassPill()
                    }
                }

                section("Files", symbol: "folder") {
                    row("Desktop index", "_Organize Log/desktop-index.md") {
                        Button("Open") { Actions.openIndex() }
                            .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 5).glassPill()
                    }
                    row("Preferences file", "_Organize Log/preferences.json — shared by every CLI") {
                        Button("Show") { NSWorkspace.shared.activateFileViewerSelecting([IndexPreferences.fileURL]) }
                            .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 5).glassPill()
                    }
                    row("Re-index log", "Output from headless runs") {
                        Button("Open") { open(Layout.logDir.appendingPathComponent("reindex.log")) }
                            .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 5).glassPill()
                    }
                    row("Organize Log folder", Layout.logDir.path) {
                        Button("Show") { NSWorkspace.shared.open(Layout.logDir) }
                            .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 5).glassPill()
                    }
                }

                section("DesktopGlass", symbol: "info.circle") {
                    row("Version", AppInfo.versionString) {
                        Button {
                            NSApp.terminate(nil)
                        } label: {
                            Label("Quit DesktopGlass", systemImage: "power")
                                .padding(.horizontal, 12).padding(.vertical, 5)
                        }
                        .buttonStyle(.plain).foregroundStyle(.red).glassPill()
                    }
                }
            }
            .padding(20)
        }
    }

    private var selectedHarness: Harness { Harness(rawValue: harness) ?? .claude }

    private var editorDetail: String {
        guard let e = Editor(rawValue: editor) else { return "" }
        if e == .builtin { return "The built-in glass editor (the ✏️ button always uses it)" }
        return e.isInstalled ? "Used by the </> buttons" : "Not installed"
    }

    private var unattendedDetail: String {
        _ = consentTick
        let h = selectedHarness
        if h.unattendedAllowed { return "Allowed · " + h.unattendedPermissions }
        return "Off — Re-index asks first, scheduled runs are skipped"
    }

    private func saveEndpoint() {
        let clean = Harness.sanitizeEndpoint(endpointText)
        if !clean.isEmpty { selectedHarness.endpoint = clean }
    }

    private var signInDetail: String {
        if selectedHarness.isLocal {
            if case .notRunning(let h)? = reindexer.problem, h == selectedHarness { return reindexer.status }
            if reindexer.checkingAuth { return "Checking \(selectedHarness.label)…" }
            if reindexer.status.contains("is ready") { return reindexer.status }
            return "Checks that the model is available — no tokens, nothing leaves this Mac"
        }
        switch reindexer.problem {
        case .signedOut(let h)? where h == selectedHarness:
            return "Not signed in — scheduled runs are paused until you sign in"
        case .limited(let h)? where h == selectedHarness:
            return "Signed in, but the last run hit a usage limit"
        default:
            if reindexer.checkingAuth { return "Checking \(selectedHarness.label)…" }
            if reindexer.status.hasPrefix("Signed in") { return reindexer.status }
            return "Uses no tokens when the CLI has an offline check"
        }
    }

    private var showCustomModel: Bool {
        customModel || (!modelText.isEmpty && !catalog.models(for: selectedHarness).contains(modelText))
    }

    private var modelSelection: Binding<String> {
        Binding(
            get: { showCustomModel ? Self.customTag : modelText },
            set: { value in
                if value == Self.customTag { customModel = true }
                else { customModel = false; modelText = value }
            })
    }

    private var modelDetail: String {
        let h = selectedHarness
        if catalog.loading.contains(h) { return "Loading models \(h.listDescription)…" }
        if catalog.isLive(h) { return "\(catalog.models(for: h).count) models \(h.listDescription)" }
        if modelText.isEmpty { return "Uses \(h.label)'s own default" }
        if h == .apple { return "Apple's on-device model (there's only one)" }
        return h.canListModels ? "Suggested models (couldn't read the live list)" : "Suggested models — Custom… for any other ID"
    }

    private var harnessDetail: String {
        if detector.detecting { return "Looking for installed CLIs…" }
        if let path = detector.installed[selectedHarness] {
            return selectedHarness.detail + " · " + path
        }
        switch selectedHarness {
        case .apple: return "Needs macOS 26 with Apple Intelligence turned on"
        case .mlx: return "Install with: pip install mlx-lm"
        case .ollama, .lmstudio, .llamacpp: return selectedHarness.detail
        default: return "Not found on your PATH — install it or pick another"
        }
    }

    private var lastRunText: String {
        if !reindexer.status.isEmpty { return reindexer.status }
        guard let last = Reindexer.lastSuccess ?? store.lastIndexed else { return "Never" }
        let f = DateFormatter(); f.dateStyle = .medium; f.timeStyle = .short
        return f.string(from: last)
    }

    private func open(_ url: URL) {
        if FileManager.default.fileExists(atPath: url.path) { NSWorkspace.shared.open(url) }
        else { NSWorkspace.shared.open(Layout.logDir) }
    }

    private func section<Content: View>(_ title: String, symbol: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Label(title, systemImage: symbol)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(.secondary)
                .padding(.bottom, 8)
            VStack(spacing: 0) { content() }
                .padding(.horizontal, 14).padding(.vertical, 4)
                .glassCard()
        }
    }

    private func row<Control: View>(_ title: String, _ detail: String, @ViewBuilder control: () -> Control) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 13, weight: .medium))
                Text(detail).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer()
            control()
        }
        .padding(.vertical, 9)
    }
}
