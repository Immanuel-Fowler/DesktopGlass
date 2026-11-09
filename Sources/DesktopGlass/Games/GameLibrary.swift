import AppKit
import Foundation

/// What a library entry runs on. Only native Mac games are listed; the type stays so the MCP JSON keeps its
/// `platform` / `platform_id` fields and other platforms can come back later as DesktopGlass apps.
enum GamePlatform: String, CaseIterable, Identifiable {
    case mac

    var id: String { rawValue }
    var name: String { "Mac" }
    var symbol: String { "macwindow" }
}

/// A game in the Games folder, as the engine's scan found it.
struct GameEntry {
    let url: URL                // what's in the Games folder (may be a Finder alias / symlink)
    var target: URL             // the real .app
    let platform: GamePlatform
    let title: String
    let detectedBy: String      // "app", "shortcut"
    var modified: Date?

    var lastPlayed: Date? { PlayHistory.lastPlayed(url) }

    init?(json d: [String: Any]) {
        guard let path = d["path"] as? String, let target = d["target"] as? String else { return nil }
        url = URL(fileURLWithPath: path)
        self.target = URL(fileURLWithPath: target)
        platform = GamePlatform(rawValue: d["platform_id"] as? String ?? "") ?? .mac
        title = d["title"] as? String ?? url.deletingPathExtension().lastPathComponent
        detectedBy = d["detected_by"] as? String ?? "app"
        modified = (d["modified"] as? Double).map { Date(timeIntervalSince1970: $0) }
    }
}

/// When each game was last played (UI state, in UserDefaults; pushed to the engine for `list_games`).
enum PlayHistory {
    private static let key = "games.lastPlayed"

    static var all: [String: Double] { (UserDefaults.standard.dictionary(forKey: key) as? [String: Double]) ?? [:] }

    static func lastPlayed(_ url: URL) -> Date? {
        guard let t = all[url.standardizedFileURL.path] else { return nil }
        return Date(timeIntervalSince1970: t)
    }

    static func record(_ url: URL) {
        var d = all
        d[url.standardizedFileURL.path] = Date().timeIntervalSince1970
        if d.count > 500 { for k in d.sorted(by: { $0.value < $1.value }).prefix(d.count - 500).map(\.key) { d[k] = nil } }
        UserDefaults.standard.set(d, forKey: key)
    }
}

/// Scans the Games folder for native Mac games (.app bundles and shortcuts to them) and launches them.
@MainActor
final class GameLibrary: ObservableObject {
    static let shared = GameLibrary()

    @Published private(set) var items: [Item] = []
    @Published private(set) var entries: [URL: GameEntry] = [:]
    @Published private(set) var scanning = false
    @Published var message = ""
    private var scannedRoot: URL?
    /// Called after a game starts, so the overlay can get out of the way.
    var onLaunched: () -> Void = {}

    enum SortOrder: String, CaseIterable, Identifiable {
        case recent, name
        var id: String { rawValue }
        var label: String { self == .recent ? "Recently Played" : "Name" }
    }

    var sortOrder: SortOrder {
        get { SortOrder(rawValue: UserDefaults.standard.string(forKey: "games.sort") ?? "") ?? .name }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: "games.sort"); applySort(); pushHistory() }
    }

    private init() {
        EngineClient.shared.on("engine/ready") { [weak self] _ in self?.pushHistory() }
    }

    private func applySort() {
        let order = sortOrder
        items.sort { a, b in
            guard let x = entries[a.url.standardizedFileURL], let y = entries[b.url.standardizedFileURL] else { return false }
            if order == .recent {
                let dx = x.lastPlayed ?? .distantPast, dy = y.lastPlayed ?? .distantPast
                if dx != dy { return dx > dy }
            }
            return x.title.localizedStandardCompare(y.title) == .orderedAscending
        }
    }

    func entry(for url: URL) -> GameEntry? { entries[url.standardizedFileURL] }

    // MARK: Scanning

    /// Scans if needed and waits for it (for the MCP tools).
    func ensureScanned(_ root: URL) async {
        scan(root)
        for _ in 0..<300 where scanning { try? await Task.sleep(nanoseconds: 100_000_000) }
    }

    /// Lists the Mac games in the Games folder (the engine's `games/scan`: .app bundles to depth 4, not inside
    /// packages, plus Finder aliases and symlinks to apps).
    func scan(_ root: URL, force: Bool = false) {
        guard force || scannedRoot != root, !scanning else { return }
        scannedRoot = root
        scanning = true
        Task {
            let found = (try? await EngineClient.shared.call("games/scan", ["root": root.path])) as? [[String: Any]]
            var entries: [URL: GameEntry] = [:]
            var items: [Item] = []
            for g in (found ?? []).compactMap(GameEntry.init(json:)) {
                entries[g.url.standardizedFileURL] = g
                items.append(Item(url: g.url, name: g.title, kind: .package, modified: g.modified, size: nil))
            }
            if found == nil { scannedRoot = nil; message = "Couldn't look for games: the DesktopGlass engine isn't running" }
            self.entries = entries
            self.items = items
            applySort()
            scanning = false
        }
    }

    /// Sends play history and the sort order to the engine, which answers the `list_games` MCP tool.
    func pushHistory() {
        Task { _ = try? await EngineClient.shared.call("games/history", ["lastPlayed": PlayHistory.all, "sort": sortOrder.rawValue]) }
    }

    // MARK: Playing

    /// Starts a game. Returns false when it couldn't (the reason is in `message`).
    @discardableResult
    func play(_ url: URL) -> Bool {
        guard let game = entry(for: url) else { NSWorkspace.shared.open(url); return true }
        NSWorkspace.shared.openApplication(at: game.target, configuration: NSWorkspace.OpenConfiguration()) { _, error in
            if let error { DispatchQueue.main.async { self.message = "\(game.title) couldn't start: \(error.localizedDescription)" } }
        }
        PlayHistory.record(game.url)
        pushHistory()
        message = "Opened \(game.title)"
        if sortOrder == .recent { applySort() } else { objectWillChange.send() }
        onLaunched()
        return true
    }
}
