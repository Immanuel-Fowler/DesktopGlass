import AppKit

enum Layout {
    static let home = FileManager.default.homeDirectoryForCurrentUser
    static let desktop = home.appendingPathComponent("Desktop", isDirectory: true)
    static let logDir = desktop.appendingPathComponent("_Organize Log", isDirectory: true)
    static let indexFile = logDir.appendingPathComponent("desktop-index.md")
    static let questionsFile = logDir.appendingPathComponent("questions.md")

    /// Names the editor's file tree dims. The overlay's own rules (groups, projects, packages, what's hidden) live
    /// in the engine: engine/crates/dg-core/src/rules.rs and desktop_model.rs.
    static let ignored: Set<String> = [".DS_Store", "desktop.ini", ".localized", "node_modules", ".venv", "venv",
                                       "__pycache__", ".git", "dist", "build", ".next", "description.md", "_Organize Log", "Icon\r"]

    static func isVisible(_ name: String) -> Bool {
        !name.hasPrefix(".") && !name.hasPrefix("~$") && !ignored.contains(name)
    }
}

/// A folder's description.md, as parsed by the engine (`desktop/load`):  "# Title", "> summary", "Tags: a, b",
/// "## Contents" bullets "- **name/** — blurb", "## Notes".
struct FolderDescription {
    var title: String?
    var summary: String?
    var tags: [String] = []
    var blurbs: [String: String] = [:]
    var notes: String?

    init(json: [String: Any]) {
        title = json["title"] as? String
        summary = json["summary"] as? String
        tags = json["tags"] as? [String] ?? []
        blurbs = json["blurbs"] as? [String: String] ?? [:]
        notes = json["notes"] as? String
    }
}

final class Item: Identifiable, Hashable {
    enum Kind: Int { case group = 0, project, folder, package, file }

    let url: URL
    let name: String
    let kind: Kind
    let modified: Date?
    let size: Int64?
    var blurb: String?
    var info: FolderDescription?
    var children: [Item] = []
    var childCount = 0
    /// Groups arrive with their children; projects and plain folders are listed the first time they're opened
    /// (`DesktopStore.loadChildren`).
    var childrenLoaded = false

    init(url: URL, name: String, kind: Kind, modified: Date?, size: Int64?) {
        self.url = url; self.name = name; self.kind = kind; self.modified = modified; self.size = size
    }

    /// One entry of the engine's `desktop/load` / `desktop/list` result.
    convenience init?(json: [String: Any]) {
        guard let path = json["path"] as? String, let name = json["name"] as? String else { return nil }
        let kind: Kind
        switch json["kind"] as? String {
        case "group": kind = .group
        case "project": kind = .project
        case "folder": kind = .folder
        case "package": kind = .package
        default: kind = .file
        }
        self.init(url: URL(fileURLWithPath: path, isDirectory: kind != .file), name: name, kind: kind,
                  modified: (json["modified"] as? Double).map { Date(timeIntervalSince1970: $0) },
                  size: (json["size"] as? NSNumber).map { $0.int64Value })
        blurb = json["blurb"] as? String
        info = (json["info"] as? [String: Any]).map(FolderDescription.init(json:))
        childCount = json["count"] as? Int ?? 0
        if let kids = json["children"] as? [[String: Any]] {
            children = kids.compactMap(Item.init(json:))
            childrenLoaded = true
        }
    }

    var id: URL { url }
    static func == (a: Item, b: Item) -> Bool { a.url == b.url }
    func hash(into h: inout Hasher) { h.combine(url) }

    var canOpenInEditor: Bool { kind == .group || kind == .project || kind == .folder }

    /// Groups, projects and plain folders all open in place in the overlay.
    var isBrowsable: Bool { kind == .group || kind == .project || kind == .folder }

    /// A project or folder whose entries haven't been listed yet.
    var needsChildren: Bool { (kind == .project || kind == .folder) && !childrenLoaded }

    /// Folders, projects and text-like files can open in the built-in editor.
    var canEditHere: Bool {
        if canOpenInEditor { return true }
        guard kind == .file else { return false }
        let ext = url.pathExtension.lowercased()
        return Item.textExtensions.contains(ext) || (ext.isEmpty && (size ?? 0) < 2_000_000)
    }

    static let textExtensions: Set<String> = ["md", "markdown", "txt", "swift", "js", "jsx", "ts", "tsx", "mjs", "cjs",
        "py", "go", "rs", "c", "h", "cpp", "hpp", "m", "mm", "java", "kt", "cs", "rb", "php", "sh", "zsh", "bash",
        "json", "yaml", "yml", "toml", "ini", "cfg", "conf", "xml", "plist", "html", "htm", "css", "scss", "sql",
        "csv", "tsv", "log", "env", "gitignore", "lua", "dart", "vue", "svelte", "svg", "tex", "r", "ipynb"]

    var parentPath: String {
        let base = Layout.desktop.standardizedFileURL.path
        let parent = url.deletingLastPathComponent().standardizedFileURL.path
        return parent == base ? "Desktop" : "Desktop/" + parent.dropFirst(base.count + 1)
    }

    var subtitle: String {
        let date = modified.map { Item.dateFormatter.string(from: $0) }
        let parts: [String?]
        switch kind {
        case .group: parts = ["\(childCount) items", date]
        case .project: parts = ["Project", date]
        case .folder: parts = ["Folder · \(childCount) items", date]
        case .package: parts = [url.pathExtension.lowercased() == "app" ? "App" : url.pathExtension.uppercased(), date]
        case .file: parts = [size.map { ByteCountFormatter.string(fromByteCount: $0, countStyle: .file) }, date]
        }
        return parts.compactMap { $0 }.joined(separator: " · ")
    }

    private static let dateFormatter: DateFormatter = {
        let f = DateFormatter(); f.dateFormat = "MMM d, yyyy"; return f
    }()
}
