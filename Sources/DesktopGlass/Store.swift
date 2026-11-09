import AppKit
import Combine

struct NewItemRequest: Identifiable {
    let id = UUID()
    let dir: URL
    var isFolder: Bool
}

@MainActor
final class DesktopStore: ObservableObject {
    @Published private(set) var roots: [Item] = []
    @Published var path: [Item] = [] { didSet { selected = 0 } }     // drill-down stack
    @Published var query = "" { didSet { if query != oldValue { queryChanged() } } }
    /// Keyboard highlight: index into `visibleItems`. Shown once the keyboard is used.
    @Published var selected = 0
    @Published var keyboardActive = false
    @Published var gridColumns = 3
    @Published private(set) var lastIndexed: Date?   // desktop-index.md last rebuilt
    @Published private(set) var lastChecked: Date?   // last successful re-index run
    @Published private(set) var openQuestions = 0
    @Published var focusToken = 0
    @Published var showingSettings = false
    /// The docked terminal is always there; this only switches it between the full panel and the slim rail.
    /// Starts collapsed every time the app opens.
    @Published var showTerminal = false
    /// The Apps dropdown in the sidebar (remembered).
    @Published var appsExpanded: Bool = UserDefaults.standard.object(forKey: "sidebarAppsExpanded") as? Bool ?? true {
        didSet { UserDefaults.standard.set(appsExpanded, forKey: "sidebarAppsExpanded") }
    }
    @Published var appsQuery = ""

    // MARK: Games folder

    /// Library mode for the Games folder (remembered).
    @Published var gameLibraryOn: Bool = UserDefaults.standard.object(forKey: "gameLibrary") as? Bool ?? true {
        didSet { UserDefaults.standard.set(gameLibraryOn, forKey: "gameLibrary"); selected = 0 }
    }
    /// Top-level folder chosen as the Games folder; empty = a folder named Games / Gaming.
    @Published var gamesFolderName: String = UserDefaults.standard.string(forKey: "gamesFolder") ?? "" {
        didSet { UserDefaults.standard.set(gamesFolderName, forKey: "gamesFolder") }
    }

    func isGamesFolder(_ item: Item) -> Bool {
        guard item.isBrowsable, item.url.deletingLastPathComponent().standardizedFileURL == Layout.desktop.standardizedFileURL else { return false }
        if !gamesFolderName.isEmpty { return item.name == gamesFolderName }
        return ["games", "gaming", "video games"].contains(item.name.lowercased())
    }

    var inGamesFolder: Bool { path.first.map(isGamesFolder) ?? false }

    /// The Games library replaces the folder grid (top level of the Games folder only).
    var libraryActive: Bool { gameLibraryOn && inGamesFolder && path.count == 1 && !isSearching && !showingSettings }
    /// Every item of the loaded tree by path, so search results (paths from the engine) map back to the same objects.
    private var flat: [String: Item] = [:]
    /// The query `results` belongs to ("" = none).
    private var resultsQuery = ""
    private var searchTask: Task<Void, Never>?
    private var searchSeq = 0
    private var reloadSeq = 0

    /// The Desktop model comes from the engine (`desktop/load`), which watches the Desktop and says when it
    /// changed. Until the engine is up, the overlay keeps showing what it has (empty at launch).
    init() {
        let engine = EngineClient.shared
        engine.on("engine/ready") { [weak self] _ in self?.reload() }
        engine.on("desktop/changed") { [weak self] _ in self?.reload() }
    }

    func reload() {
        let engine = EngineClient.shared
        guard engine.ready else { return }
        reloadSeq += 1
        let seq = reloadSeq
        Task {
            guard let r = try? await engine.call("desktop/load") as? [String: Any], seq == self.reloadSeq else { return }
            let roots = (r["items"] as? [[String: Any]] ?? []).compactMap(Item.init(json:))
            var flat: [String: Item] = [:]
            func walk(_ items: [Item]) { for i in items { flat[i.url.path] = i; walk(i.children) } }
            walk(roots)
            self.roots = roots
            self.flat = flat
            self.lastIndexed = (r["indexed"] as? Double).map { Date(timeIntervalSince1970: $0) }
            self.lastChecked = Reindexer.lastSuccess
            self.openQuestions = r["questions"] as? Int ?? 0
            if self.isSearching { await self.runSearch(self.query) }
            let keep = self.selected
            let open = self.path.last?.url
            let chain = open == nil ? [] : (await self.chain(to: open!) ?? [])
            guard seq == self.reloadSeq, self.path.last?.url == open else { return }   // the user moved on meanwhile
            self.path = chain
            self.selected = min(keep, max(0, self.visibleItems.count - 1))
        }
    }

    var currentItems: [Item] { libraryActive ? GameLibrary.shared.items : (path.last?.children ?? roots) }
    var isSearching: Bool { !query.trimmingCharacters(in: .whitespaces).isEmpty }

    /// Search results from the engine (`desktop/search`). While a newer query is in flight the previous results stay.
    @Published private(set) var results: [Item] = []

    private func queryChanged() {
        selected = 0
        searchTask?.cancel()
        guard isSearching else {
            searchSeq += 1
            results = []
            resultsQuery = ""
            return
        }
        let q = query
        let immediate = resultsQuery.isEmpty       // the first keystroke answers at once; typing on is debounced
        searchTask = Task { [weak self] in
            if !immediate { try? await Task.sleep(nanoseconds: 120_000_000) }
            guard !Task.isCancelled else { return }
            await self?.runSearch(q)
        }
    }

    /// Asks the engine; applies the answer only if `q` is still the query.
    private func runSearch(_ q: String) async {
        searchSeq += 1
        let seq = searchSeq
        guard let r = try? await EngineClient.shared.call("desktop/search", ["query": q]) as? [String: Any],
              seq == searchSeq, q == query else { return }
        results = (r["paths"] as? [String] ?? []).compactMap { flat[$0] }
        resultsQuery = q
        selected = min(selected, max(0, results.count - 1))
    }

    // MARK: New folders and files

    /// The new-folder / new-file form, when open.
    @Published var newItem: NewItemRequest?

    /// Where new items go: the folder being browsed, else the Desktop itself.
    var currentFolderURL: URL {
        if let last = path.last, last.isBrowsable { return last.url }
        return Layout.desktop
    }

    func startNewItem(folder: Bool) { newItem = NewItemRequest(dir: currentFolderURL, isFolder: folder) }

    /// Creates it; returns an error message to show, or nil. A new file opens in the DesktopGlass editor.
    func createItem(named raw: String, folder: Bool, in dir: URL) -> String? {
        let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return "Type a name" }
        guard !name.contains("/"), !name.contains(":"), name != ".", name != ".." else { return "Names can't contain / or :" }
        let url = dir.appendingPathComponent(name, isDirectory: folder)
        guard !FileManager.default.fileExists(atPath: url.path) else { return "“\(name)” already exists here" }
        do {
            if folder {
                try FileManager.default.createDirectory(at: url, withIntermediateDirectories: false)
            } else if !FileManager.default.createFile(atPath: url.path, contents: Data()) {
                return "Couldn't create “\(name)”"
            }
        } catch {
            return error.localizedDescription
        }
        newItem = nil
        reload()                       // the engine's watcher also notices; this just makes it immediate
        if !folder { EditorRouter.open(url) }
        return nil
    }

    func select(root: Item?) {
        path = root.map { [$0] } ?? []
        query = ""
        showingSettings = false
    }

    /// Lists a project or folder the first time it's opened.
    func loadChildren(_ item: Item) async {
        guard item.needsChildren,
              let r = try? await EngineClient.shared.call("desktop/list", ["path": item.url.path]) as? [String: Any],
              item.needsChildren else { return }
        item.children = (r["items"] as? [[String: Any]] ?? []).compactMap(Item.init(json:))
        item.childCount = item.children.count
        item.childrenLoaded = true
        objectWillChange.send()
    }

    /// Items from the Desktop down to `url`, opening (and loading) folders and projects on the way.
    func chain(to url: URL) async -> [Item]? {
        let target = url.standardizedFileURL.path
        var items = roots
        var out: [Item] = []
        while true {
            guard let next = items.first(where: { i in
                let p = i.url.standardizedFileURL.path
                return i.isBrowsable && (p == target || target.hasPrefix(p + "/"))
            }) else { return out.isEmpty ? nil : out }
            await loadChildren(next)
            out.append(next)
            if next.url.standardizedFileURL.path == target { return out }
            items = next.children
        }
    }

    func navigate(to target: Item) {
        Task {
            guard let c = await chain(to: target.url) else { return }
            query = ""
            showingSettings = false
            path = c
        }
    }

    /// Folders and projects open in place; files and apps open in their default app.
    func activate(_ item: Item) {
        if libraryActive, GameLibrary.shared.entry(for: item.url) != nil {
            GameLibrary.shared.play(item.url)
            return
        }
        if item.isBrowsable {
            if !isSearching, currentItems.contains(where: { $0 === item }) {
                let from = path.last?.url
                Task {
                    await loadChildren(item)
                    guard path.last?.url == from else { return }
                    path.append(item)
                }
            } else {
                navigate(to: item)
            }
        } else {
            Actions.open(item)
        }
    }

    /// Return in the search field: the first result of the current query (waiting for it if it's still coming).
    func activateFirstResult() {
        guard isSearching else { return }
        if resultsQuery == query {
            if let first = results.first { activate(first) }
            return
        }
        searchTask?.cancel()
        let q = query
        Task {
            await runSearch(q)
            if resultsQuery == q, q == query, let first = results.first { activate(first) }
        }
    }

    // MARK: Keyboard

    var visibleItems: [Item] { isSearching ? results : currentItems }

    func moveSelection(dx: Int, dy: Int) {
        let n = visibleItems.count
        guard n > 0 else { return }
        if !keyboardActive { keyboardActive = true; selected = min(selected, n - 1); return }
        let step = isSearching ? dy + dx : dy * max(1, gridColumns) + dx
        selected = min(max(0, selected + step), n - 1)
    }

    var selectedItem: Item? {
        let items = visibleItems
        return items.indices.contains(selected) ? items[selected] : nil
    }

    func activateSelection() { if let i = selectedItem { keyboardActive = true; activate(i) } }

    /// Back one level (or out of a search), keeping the highlight on the folder you came from.
    func goBack() {
        keyboardActive = true
        if isSearching { query = ""; return }
        guard let left = path.popLast() else { return }
        selected = currentItems.firstIndex(where: { $0 === left }) ?? 0
    }
}

enum Editor: String, CaseIterable {
    case builtin = "DesktopGlass Editor"
    case vscode = "Visual Studio Code", cursor = "Cursor", xcode = "Xcode", zed = "Zed"

    var bundleID: String {
        switch self {
        case .builtin: return Bundle.main.bundleIdentifier ?? ""
        case .vscode: return "com.microsoft.VSCode"
        case .cursor: return "com.todesktop.230313mxl4wiwg"
        case .xcode: return "com.apple.dt.Xcode"
        case .zed: return "dev.zed.Zed"
        }
    }

    var isInstalled: Bool { self == .builtin || appURL != nil }

    /// The editor used for "Open in …" from inside the built-in editor (never the built-in one itself).
    static var external: Editor {
        let c = current
        if c != .builtin, c.appURL != nil { return c }
        return allCases.first { $0 != .builtin && $0.appURL != nil } ?? .vscode
    }

    func open(_ url: URL) {
        if self == .builtin { EditorRouter.open(url); return }
        guard let app = appURL else { NSSound.beep(); return }
        NSWorkspace.shared.open([url], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
    }

    var appURL: URL? {
        if self == .builtin { return nil }
        if let u = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) { return u }
        for base in ["/Applications", Layout.home.appendingPathComponent("Applications").path] {
            let u = URL(fileURLWithPath: "\(base)/\(rawValue).app")
            if FileManager.default.fileExists(atPath: u.path) { return u }
        }
        return nil
    }

    static var current: Editor {
        get { Editor(rawValue: UserDefaults.standard.string(forKey: "editor") ?? "") ?? .vscode }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: "editor") }
    }
}

enum Actions {
    static func open(_ item: Item) { NSWorkspace.shared.open(item.url) }

    static func reveal(_ item: Item) { NSWorkspace.shared.activateFileViewerSelecting([item.url]) }

    static func openInEditor(_ item: Item) { Editor.current.open(item.url) }

    static func editHere(_ item: Item) { EditorRouter.open(item.url) }

    static func copyPath(_ item: Item) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(item.url.path, forType: .string)
    }

    static func openIndex() {
        if FileManager.default.fileExists(atPath: Layout.indexFile.path) { NSWorkspace.shared.open(Layout.indexFile) }
        else { NSWorkspace.shared.open(Layout.desktop) }
    }

    /// Opens the selected CLI's sign-in flow in Terminal.
    static func signIn(_ harness: Harness = .current) {
        let dg = (DGEngine.binary ?? Layout.home.appendingPathComponent(".local/bin/dg")).path
        let source = """
        tell application "Terminal"
            activate
            do script "'\(dg)' login \(harness.rawValue)"
        end tell
        """
        var error: NSDictionary?
        NSAppleScript(source: source)?.executeAndReturnError(&error)
    }

    /// Opens an interactive session in the selected CLI so the task can ask its pending questions.
    static func answerQuestions() {
        let harness = Harness.current
        let model = Harness.sanitizeModel(harness.model)
        let dg = (DGEngine.binary ?? Layout.home.appendingPathComponent(".local/bin/dg")).path
        let command = "'\(dg)' ask \(harness.rawValue)" + (model.isEmpty ? "" : " '\(model)'")
        let source = """
        tell application "Terminal"
            activate
            do script "\(command)"
        end tell
        """
        var error: NSDictionary?
        NSAppleScript(source: source)?.executeAndReturnError(&error)
        if error != nil { NSWorkspace.shared.open(Layout.questionsFile) }
    }
}

extension DesktopStore: TerminalDock {
    /// Agents in the overlay work from the Desktop itself, so browsing around never restarts them.
    var terminalFolder: URL? { Layout.desktop }
    var terminalFolderName: String { "Desktop" }
    var browsingFolder: URL? { path.last?.url ?? Layout.desktop }
}

