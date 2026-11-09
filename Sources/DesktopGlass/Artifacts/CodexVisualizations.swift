import Foundation

/// Codex's Visualize skill saves each visualization as an HTML fragment in
/// `~/.codex/visualizations/YYYY/MM/DD/<thread>/<name>.html` (or `<project>/.codex/visualizations/…`) and only the
/// Codex app draws it — the CLI and other clients show "Visualization unavailable on this device". This watches
/// those folders and opens each new or updated visualization in the viewer, as a full page.
@MainActor
final class CodexVisualizations {
    static let shared = CodexVisualizations()
    nonisolated static let enabledKey = "codex.visualizations.autoshow"

    /// Extra places to look (the editor's open folder), set by the app.
    var projectRoots: () -> [URL] = { [] }
    private var seen: [String: Date] = [:]
    private var timer: Timer?
    private let started = Date()

    nonisolated static var codexHome: URL {
        ProcessInfo.processInfo.environment["CODEX_HOME"].map { URL(fileURLWithPath: $0) }
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".codex")
    }

    /// Where the full-page copies go (Codex's own folder is left alone).
    nonisolated static var pagesDir: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/DesktopGlass/Codex Visualizations", isDirectory: true)
    }

    var enabled: Bool { UserDefaults.standard.object(forKey: Self.enabledKey) as? Bool ?? true }

    func start() {
        guard timer == nil else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
            MainActor.assumeIsolated { CodexVisualizations.shared.tick() }
        }
    }

    private func tick() {
        guard enabled else { return }
        let roots = [Self.codexHome.appendingPathComponent("visualizations")]
            + projectRoots().map { $0.appendingPathComponent(".codex/visualizations") }
        for file in Self.recentFiles(in: roots, now: Date()) {
            let modified = Self.modified(file) ?? .distantPast
            guard modified > started else { continue }              // only what Codex makes while DesktopGlass runs
            let isNew = seen[file.path] == nil
            guard seen[file.path] != modified else { continue }
            seen[file.path] = modified
            guard let page = try? Self.writePage(for: file) else { continue }
            if isNew {
                ArtifactBrowserController.shared.open(page, title: "Codex · " + file.deletingPathExtension().lastPathComponent)
            }                                                          // updates: the open tab reloads the page itself
        }
    }

    // MARK: Finding and converting (no app state; tested)

    /// .html files in today's and yesterday's YYYY/MM/DD folders under each root, at any depth below that
    /// (normally <thread>/<name>.html).
    nonisolated static func recentFiles(in roots: [URL], now: Date) -> [URL] {
        let fm = FileManager.default
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = .current
        let days = [now, now.addingTimeInterval(-86_400)].map { d -> String in
            let c = cal.dateComponents([.year, .month, .day], from: d)
            return String(format: "%04d/%02d/%02d", c.year ?? 0, c.month ?? 0, c.day ?? 0)
        }
        var out: [URL] = []
        for root in roots {
            for day in Set(days) {
                let dir = root.appendingPathComponent(day, isDirectory: true)
                guard let e = fm.enumerator(at: dir, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]) else { continue }
                while let u = e.nextObject() as? URL {
                    if e.level > 3 { e.skipDescendants(); continue }
                    if ["html", "htm"].contains(u.pathExtension.lowercased()) { out.append(u) }
                }
            }
        }
        return out.sorted { $0.path < $1.path }
    }

    /// A complete page for a fragment (a file that's already a full document is used as it is).
    nonisolated static func page(for fragment: String, title: String) -> String {
        let head = fragment.prefix(1000).lowercased()
        if head.contains("<!doctype") || head.contains("<html") { return fragment }
        let safeTitle = title.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
        return """
        <!doctype html>
        <html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
        <meta name="color-scheme" content="light dark"><title>\(safeTitle)</title>
        <style>html,body{margin:0;background:transparent}body{padding:20px;font:14px -apple-system,system-ui,sans-serif;color:CanvasText}</style>
        </head><body>
        \(fragment)
        </body></html>

        """
    }

    /// Where the page for a visualization file goes: `<pagesDir>/<thread folder>/<name>.html`.
    nonisolated static func pageURL(for file: URL, in dir: URL) -> URL {
        let thread = file.deletingLastPathComponent().lastPathComponent
        return dir.appendingPathComponent(thread, isDirectory: true).appendingPathComponent(file.lastPathComponent)
    }

    nonisolated static func writePage(for file: URL, in dir: URL = pagesDir) throws -> URL {
        let text = try String(contentsOf: file, encoding: .utf8)
        let out = pageURL(for: file, in: dir)
        try FileManager.default.createDirectory(at: out.deletingLastPathComponent(), withIntermediateDirectories: true)
        try page(for: text, title: file.deletingPathExtension().lastPathComponent).write(to: out, atomically: true, encoding: .utf8)
        return out
    }

    nonisolated private static func modified(_ url: URL) -> Date? {
        (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
    }
}
