import AppKit

final class FileNode {
    let url: URL
    let name: String
    let isDirectory: Bool
    private var cached: [FileNode]?

    init(url: URL, isDirectory: Bool) {
        self.url = url
        self.name = url.lastPathComponent
        self.isDirectory = isDirectory
    }

    var children: [FileNode] {
        if let c = cached { return c }
        let keys: [URLResourceKey] = [.isDirectoryKey, .isPackageKey]
        let urls = (try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: keys, options: [])) ?? []
        let nodes = urls
            .filter { $0.lastPathComponent != ".DS_Store" && $0.lastPathComponent != ".git" }
            .map { u -> FileNode in
                let v = try? u.resourceValues(forKeys: Set(keys))
                return FileNode(url: u, isDirectory: (v?.isDirectory ?? false) && !(v?.isPackage ?? false))
            }
            .sorted { a, b in
                a.isDirectory != b.isDirectory ? a.isDirectory : a.name.localizedStandardCompare(b.name) == .orderedAscending
            }
        cached = nodes
        return nodes
    }

    var isDimmed: Bool { name.hasPrefix(".") || Layout.ignored.contains(name) }
}

struct TreeRow: Identifiable {
    let node: FileNode
    let depth: Int
    var id: URL { node.url }
}

@MainActor
final class EditorModel: ObservableObject {
    @Published private(set) var root: FileNode?
    @Published private(set) var rows: [TreeRow] = []
    @Published private(set) var documents: [EditorDocument] = []
    @Published var active: EditorDocument?
    /// A page from the artifact viewer shown in the code area (its tabs live in ArtifactBrowserController).
    @Published var activeWeb: ArtifactTab?
    @Published var showSidebar = true
    @Published var showTerminal = UserDefaults.standard.object(forKey: "editorShowTerminal") as? Bool ?? true {
        didSet { UserDefaults.standard.set(showTerminal, forKey: "editorShowTerminal") }
    }
    @Published var showQuickOpen = false
    @Published private(set) var fileIndex: [URL] = []
    @Published private(set) var fontSize: CGFloat = {
        let saved = UserDefaults.standard.double(forKey: "editorFontSize")
        return saved > 0 ? CGFloat(saved) : 13
    }()
    private var expanded: Set<URL> = []
    private var watcher: DirectoryWatcher?
    private var visualSessions: [ObjectIdentifier: HTMLVisualSession] = [:]

    func document(at url: URL) -> EditorDocument? {
        documents.first { $0.url.standardizedFileURL == url.standardizedFileURL }
    }

    /// The visual editor for an HTML tab (kept while the tab is open, so selection and scroll survive switching).
    func visualSession(for doc: EditorDocument) -> HTMLVisualSession {
        if let s = visualSessions[ObjectIdentifier(doc)] { return s }
        let s = HTMLVisualSession(document: doc, projectRoot: root?.url)
        s.openDocument = { [weak self] url in self?.document(at: url) }
        s.openDocuments = { [weak self] in self?.documents ?? [] }
        visualSessions[ObjectIdentifier(doc)] = s
        return s
    }

    var hasUnsavedChanges: Bool { documents.contains { $0.isDirty } }
    var rootName: String { root?.name ?? "No Folder" }

    // MARK: Opening

    /// Folder → becomes the workspace. File → opens in a tab (its folder becomes the workspace if it's outside the current one).
    func open(_ url: URL) {
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir) else { return }
        if isDir.boolValue && !NSWorkspace.shared.isFilePackage(atPath: url.path) {
            if root?.url.standardizedFileURL != url.standardizedFileURL { setRoot(url) }
            return
        }
        if !isInsideRoot(url) { setRoot(url.deletingLastPathComponent()) }
        openFile(url)
    }

    /// Asks about unsaved changes before switching to a different workspace. Returns false if the user cancels.
    func confirmSwitch(to url: URL) -> Bool {
        let target = isDirectory(url) ? url : url.deletingLastPathComponent()
        let leaving = documents.filter { $0.isDirty && !isInside($0.url, target) }
        guard !leaving.isEmpty, !(root.map { isInside(url, $0.url) } ?? false) else { return true }
        return confirmUnsaved(leaving, verb: "opening another folder")
    }

    func setRoot(_ url: URL) {
        root = FileNode(url: url, isDirectory: true)
        // Files created outside the editor (an agent in the terminal, Finder) appear without pressing Refresh.
        watcher = DirectoryWatcher(url: url) { [weak self] in
            Task { @MainActor in self?.refresh() }
        }
        expanded = [url]
        documents.removeAll { !$0.isDirty && !isInside($0.url, url) }
        if let a = active, !documents.contains(where: { $0 === a }) { active = documents.last }
        rebuildRows()
        buildIndex()
    }

    func openFile(_ url: URL) {
        activeWeb = nil
        if let existing = documents.first(where: { $0.url.standardizedFileURL == url.standardizedFileURL }) {
            active = existing
        } else if let doc = EditorDocument(url: url, fontSize: fontSize) {
            documents.append(doc)
            active = doc
        } else {
            NSWorkspace.shared.open(url)   // binary or huge: hand it to its default app
            return
        }
        reveal(url)
    }

    func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = true
        panel.allowsMultipleSelection = false
        panel.directoryURL = root?.url ?? Layout.desktop
        panel.prompt = "Open"
        if panel.runModal() == .OK, let url = panel.url, confirmSwitch(to: url) { open(url) }
    }

    // MARK: Tree

    func isExpanded(_ node: FileNode) -> Bool { expanded.contains(node.url) }

    func toggle(_ node: FileNode) {
        if expanded.contains(node.url) { expanded.remove(node.url) } else { expanded.insert(node.url) }
        rebuildRows()
    }

    func collapseAll() {
        if let r = root { expanded = [r.url] }
        rebuildRows()
    }

    func refresh() {
        guard let r = root else { return }
        root = FileNode(url: r.url, isDirectory: true)
        rebuildRows()
        buildIndex()
        for doc in documents { doc.reloadIfChangedOnDisk() }
    }

    private func rebuildRows() {
        var out: [TreeRow] = []
        func add(_ node: FileNode, depth: Int) {
            for child in node.children {
                out.append(TreeRow(node: child, depth: depth))
                if child.isDirectory && expanded.contains(child.url) && out.count < 20_000 { add(child, depth: depth + 1) }
            }
        }
        if let r = root { add(r, depth: 0) }
        rows = out
    }

    private func reveal(_ url: URL) {
        guard let r = root else { return }
        var dir = url.deletingLastPathComponent().standardizedFileURL
        let rootURL = r.url.standardizedFileURL
        var changed = false
        while dir.path.hasPrefix(rootURL.path) && dir != rootURL {
            if !expanded.contains(dir) { expanded.insert(dir); changed = true }
            dir = dir.deletingLastPathComponent()
        }
        // expanded holds the URLs the tree hands out; match them by path
        if changed {
            let paths = Set(expanded.map { $0.standardizedFileURL.path })
            var matched: Set<URL> = [r.url]
            func walk(_ node: FileNode) {
                for child in node.children where child.isDirectory && paths.contains(child.url.standardizedFileURL.path) {
                    matched.insert(child.url); walk(child)
                }
            }
            walk(r)
            expanded = matched.union(expanded)
            rebuildRows()
        }
    }

    // MARK: Tabs & saving

    func close(_ doc: EditorDocument) {
        if doc.isDirty && !confirmUnsaved([doc], verb: "closing") { return }
        guard let index = documents.firstIndex(where: { $0 === doc }) else { return }
        documents.remove(at: index)
        visualSessions[ObjectIdentifier(doc)] = nil
        if active === doc { active = documents.isEmpty ? nil : documents[min(index, documents.count - 1)] }
    }

    func closeActive() { if let a = active { close(a) } }
    func saveActive() { active?.save() }
    func saveAll() { for d in documents where d.isDirty { d.save() } }

    /// Save / Don't Save / Cancel. Returns false on Cancel.
    func confirmUnsaved(_ docs: [EditorDocument], verb: String) -> Bool {
        let alert = NSAlert()
        alert.messageText = docs.count == 1 ? "Save changes to \(docs[0].name) before \(verb)?" : "Save changes to \(docs.count) files before \(verb)?"
        alert.informativeText = "Your changes will be lost if you don't save them."
        alert.addButton(withTitle: docs.count == 1 ? "Save" : "Save All")
        alert.addButton(withTitle: "Don't Save")
        alert.addButton(withTitle: "Cancel")
        switch alert.runModal() {
        case .alertFirstButtonReturn: return docs.allSatisfy { $0.save() }
        case .alertSecondButtonReturn: return true
        default: return false
        }
    }

    func confirmCloseAll() -> Bool {
        let dirty = documents.filter(\.isDirty)
        return dirty.isEmpty || confirmUnsaved(dirty, verb: "closing the editor")
    }

    func reloadChangedFiles() { for d in documents { d.reloadIfChangedOnDisk() } }

    func changeFontSize(by delta: CGFloat?) {
        let size = delta.map { min(28, max(9, fontSize + $0)) } ?? 13
        fontSize = size
        UserDefaults.standard.set(Double(size), forKey: "editorFontSize")
        for d in documents { d.setFontSize(size) }
    }

    // MARK: File operations

    func newFile(in dir: URL) {
        guard let name = promptName(title: "New File", placeholder: "untitled.txt") else { return }
        let url = dir.appendingPathComponent(name)
        guard !FileManager.default.fileExists(atPath: url.path) else { NSSound.beep(); return }
        FileManager.default.createFile(atPath: url.path, contents: Data())
        refresh()
        openFile(url)
    }

    func newFolder(in dir: URL) {
        guard let name = promptName(title: "New Folder", placeholder: "folder") else { return }
        let url = dir.appendingPathComponent(name, isDirectory: true)
        do { try FileManager.default.createDirectory(at: url, withIntermediateDirectories: false) }
        catch { NSAlert(error: error).runModal(); return }
        expanded.insert(dir)
        refresh()
    }

    func moveToTrash(_ node: FileNode) {
        let alert = NSAlert()
        alert.messageText = "Move “\(node.name)” to the Trash?"
        alert.addButton(withTitle: "Move to Trash")
        alert.addButton(withTitle: "Cancel")
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        NSWorkspace.shared.recycle([node.url]) { _, error in
            DispatchQueue.main.async {
                if let error { NSAlert(error: error).runModal(); return }
                self.documents.removeAll { self.isInside($0.url, node.url) }
                if let a = self.active, !self.documents.contains(where: { $0 === a }) { self.active = self.documents.last }
                self.refresh()
            }
        }
    }

    private func promptName(title: String, placeholder: String) -> String? {
        let alert = NSAlert()
        alert.messageText = title
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 260, height: 24))
        field.placeholderString = placeholder
        alert.accessoryView = field
        alert.addButton(withTitle: "Create")
        alert.addButton(withTitle: "Cancel")
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return nil }
        let name = field.stringValue.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty, !name.contains("/") else { return nil }
        return name
    }

    // MARK: Quick Open

    private func buildIndex() {
        guard let rootURL = root?.url else { fileIndex = []; return }
        let skip: Set<String> = ["node_modules", ".git", ".venv", "venv", "__pycache__", "build", "dist", ".next",
                                 ".build", "DerivedData", "Pods", ".gradle", "target", ".turbo", ".cache"]
        DispatchQueue.global(qos: .utility).async {
            var out: [URL] = []
            let e = FileManager.default.enumerator(at: rootURL, includingPropertiesForKeys: [.isDirectoryKey],
                                                   options: [.skipsPackageDescendants])
            while let u = e?.nextObject() as? URL {
                let name = u.lastPathComponent
                if skip.contains(name) { e?.skipDescendants(); continue }
                if name == ".DS_Store" { continue }
                if (try? u.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true { continue }
                out.append(u)
                if out.count >= 40_000 { break }
            }
            let result = out
            DispatchQueue.main.async {
                if self.root?.url == rootURL { self.fileIndex = result }
            }
        }
    }

    func relativePath(_ url: URL) -> String {
        guard let r = root?.url.standardizedFileURL.path else { return url.path }
        let p = url.standardizedFileURL.path
        return p.hasPrefix(r + "/") ? String(p.dropFirst(r.count + 1)) : p
    }

    func quickOpenResults(_ query: String) -> [URL] {
        let q = query.lowercased().filter { !$0.isWhitespace }
        if q.isEmpty {
            let open = documents.map(\.url)
            return open + fileIndex.filter { !open.contains($0) }.prefix(40)
        }
        var scored: [(Int, URL)] = []
        for u in fileIndex {
            if let s = Self.fuzzyScore(q, relativePath(u).lowercased(), name: u.lastPathComponent.lowercased()) {
                scored.append((s, u))
            }
        }
        return scored.sorted { $0.0 > $1.0 }.prefix(60).map(\.1)
    }

    private static func fuzzyScore(_ q: String, _ path: String, name: String) -> Int? {
        var score = 0
        var index = path.startIndex
        var previous: String.Index?
        for ch in q {
            guard let found = path[index...].firstIndex(of: ch) else { return nil }
            if let p = previous, path.index(after: p) == found { score += 8 }
            score += 1
            previous = found
            index = path.index(after: found)
        }
        if name.hasPrefix(q) { score += 100 } else if name.contains(q) { score += 50 }
        return score - path.count / 8
    }

    // MARK: Helpers

    private func isDirectory(_ url: URL) -> Bool {
        var d: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &d) && d.boolValue
    }

    private func isInsideRoot(_ url: URL) -> Bool { root.map { isInside(url, $0.url) } ?? false }

    func isInside(_ url: URL, _ dir: URL) -> Bool {
        let p = url.standardizedFileURL.path, d = dir.standardizedFileURL.path
        return p == d || p.hasPrefix(d.hasSuffix("/") ? d : d + "/")
    }
}

extension EditorModel: TerminalDock {
    var terminalFolder: URL? { root?.url }
    var terminalFolderName: String { rootName }
}
