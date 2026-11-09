import AppKit
import Foundation

/// Everything the MCP server can ask the running app to do. Runs on the main actor.
@MainActor
final class AppControl {
    let artifacts: ArtifactBrowserController
    let reindexer: Reindexer
    let store: DesktopStore
    let openEditor: (URL) -> Void
    let showOverlay: () -> Void
    var controlPort: Int = 0
    var documents: () -> [EditorDocument] = { [] }

    init(artifacts: ArtifactBrowserController, reindexer: Reindexer, store: DesktopStore,
         openEditor: @escaping (URL) -> Void, showOverlay: @escaping () -> Void) {
        self.artifacts = artifacts
        self.reindexer = reindexer
        self.store = store
        self.openEditor = openEditor
        self.showOverlay = showOverlay
    }

    /// MCP permission groups (Settings → MCP) for the UI-bound methods that reach the app. The engine checks the same
    /// gates first (dg_proto::gate); this is a second, defence-in-depth check.
    /// Settings, schedule, re-index and preferences methods are answered by the engine and never get here.
    private static let gates: [String: (key: String, label: String)] = {
        var g: [String: (key: String, label: String)] = [:]
        for m in ["html.style", "html.split_style", "html.group_style", "html.set_text", "html.set_html", "html.duplicate", "html.remove"] {
            g[m] = (key: "edit", label: "Edit files")
        }
        for m in ["browser.eval", "browser.click", "browser.type"] { g[m] = (key: "script", label: "Click, type and run JavaScript") }
        g["games.play"] = (key: "play", label: "Launch games")
        return g
    }()

    func handle(_ method: String, _ p: [String: Any]) async throws -> Any {
        if let gate = Self.gates[method], !UserDefaults.standard.bool(forKey: "mcp.allow.\(gate.key)") {
            throw ControlError("“\(gate.label)” is turned off for AI apps — the user can allow it in DesktopGlass → Settings → MCP")
        }
        switch method {
        case "ping":
            return ["ok": true, "version": AppInfo.version]

        // MARK: Browser
        case "browser.open":
            guard let raw = p["target"] as? String ?? p["url"] as? String, let url = await ArtifactPolicy.resolve(raw) else {
                throw ControlError("Give a file path, a Desktop-relative path, or a URL")
            }
            guard artifacts.open(url, title: p["title"] as? String, explicit: true) else {
                throw ControlError("\(url.absoluteString) can't be shown: web pages, local servers, and HTML, SVG, PDF, image, Markdown, text and JSON files open here")
            }
            if let doc = artifacts.lastDocument {
                return ["title": doc.lastPathComponent, "url": doc.path, "isFile": true, "loading": false,
                        "shownIn": "editor", "kind": "document"]
            }
            if let tab = artifacts.active { await tab.waitUntilLoaded(timeout: 10); return tab.info }
            return [:]
        case "browser.tabs":
            return artifacts.tabs.map { t -> [String: Any] in
                var i = t.info; i["active"] = artifacts.active === t; return i
            }
        case "browser.read":
            let tab = try requireTab(p)
            await tab.waitUntilLoaded()
            let js = WebScripts.source("browser-read")
            let raw = try await tab.evaluate(js) as? String ?? "{}"
            var obj = (try? JSONSerialization.jsonObject(with: Data(raw.utf8)) as? [String: Any]) ?? [:]
            let max = p["max_chars"] as? Int ?? 20_000
            if let text = obj["text"] as? String, text.count > max { obj["text"] = String(text.prefix(max)) + "\n…(truncated)" }
            obj["tab"] = tab.id.uuidString
            return obj
        case "browser.click":
            let tab = try requireTab(p)
            let js = try WebScripts.call("browser-click", [p["selector"] as? String ?? ""])
            let result = try await tab.evaluate(js) as? String ?? ""
            await tab.waitUntilLoaded(timeout: 5)
            return result
        case "browser.type":
            let tab = try requireTab(p)
            let submit = p["submit"] as? Bool ?? false
            let js = try WebScripts.call("browser-type", [p["selector"] as? String ?? "", p["text"] as? String ?? "", submit])
            let result = try await tab.evaluate(js) as? String ?? ""
            if submit { await tab.waitUntilLoaded(timeout: 8) }
            return result
        case "browser.eval":
            let tab = try requireTab(p)
            let js = try WebScripts.call("browser-eval", [p["script"] as? String ?? ""])
            return try await tab.evaluate(js) as? String ?? "undefined"
        case "browser.screenshot":
            let tab = try requireTab(p)
            await tab.waitUntilLoaded()
            guard let png = await tab.snapshotPNG() else { throw ControlError("Couldn't capture the page") }
            return ["png": png.base64EncodedString(), "title": tab.title, "url": tab.info["url"] ?? ""]
        case "browser.navigate":
            let tab = try requireTab(p)
            switch p["action"] as? String ?? "" {
            case "back": tab.goBack()
            case "forward": tab.goForward()
            case "reload": tab.reload()
            case "close": artifacts.close(tab); return "Closed"
            default:
                guard let raw = p["url"] as? String, let url = await ArtifactPolicy.address(raw) else { throw ControlError("Unknown action") }
                guard tab.navigate(url) else { throw ControlError("Only web pages, local servers and viewable files can open here") }
            }
            await tab.waitUntilLoaded()
            return tab.info
        // MARK: HTML styling (the engine computes the edits, like the visual editor's)
        case let m where m.hasPrefix("html."):
            // Open documents may hold unsaved text: the engine reads it from here, writes the files that aren't
            // open, and returns the edits for the ones that are, which are applied (undoably) and saved.
            let open = documents()
            let r = try await EngineClient.shared.call("html/control", ["method": m, "params": p,
                                                                        "buffers": HTMLEngine.buffers(open)]) as? [String: Any] ?? [:]
            let changed = try HTMLEdits.apply(HTMLEdits.files(r["files"])) { url in
                open.first { $0.url.standardizedFileURL == url.standardizedFileURL }
            }
            changed.forEach { $0.save() }
            return r["result"] as? String ?? ""

        // MARK: Windows
        case "editor.open":
            guard let raw = p["path"] as? String, let url = await ArtifactPolicy.resolve(raw), url.isFileURL else {
                throw ControlError("Give a file or folder path")
            }
            openEditor(url)
            return "Opened \(url.lastPathComponent) in the DesktopGlass editor"
        case "overlay.show":
            showOverlay()
            return "Shown"

        // MARK: Games
        case "games.play":
            let library = try await gamesLibrary()
            guard let raw = p["game"] as? String, !raw.isEmpty else { throw ControlError("Give a game title or path from list_games") }
            let all = library.items.compactMap { library.entry(for: $0.url) }
            let match = all.first { $0.url.path == raw || $0.target.path == raw }
                ?? all.first { $0.title.caseInsensitiveCompare(raw) == .orderedSame }
                ?? all.first { $0.title.lowercased().contains(raw.lowercased()) }
            guard let game = match else { throw ControlError("No game matching “\(raw)” in the Games folder") }
            let ok = library.play(game.url)
            if !ok { throw ControlError(library.message) }
            return library.message
        default:
            throw ControlError("Unknown method \(method)")
        }
    }

    private func gamesLibrary() async throws -> GameLibrary {
        if store.roots.isEmpty { store.reload() }
        guard let root = store.roots.first(where: store.isGamesFolder) else {
            throw ControlError("There's no Games folder on the Desktop (a folder named Games, or one chosen with “Use as Games Folder”)")
        }
        let library = GameLibrary.shared
        await library.ensureScanned(root.url)
        return library
    }

    private func requireTab(_ p: [String: Any]) throws -> ArtifactTab {
        guard let tab = artifacts.tab(p["tab"] as? String) else {
            throw ControlError("No artifact is open — use show_artifact first")
        }
        return tab
    }
}
