import AppKit
import XCTest
@testable import DesktopGlass

/// The UI side of the engine-backed HTML editor and artifact policy: applying the engine's edits to open documents
/// (one undoable change), the Swift matcher over the engine's policy tables, and the shared web scripts.
///
/// The end-to-end tests run the real `dg engine` (engine/target/{release,debug}/dg, skipped if it isn't built) with
/// HOME pointed at a temp folder, so its settings, control.json and Desktop are all scratch.
@MainActor
final class HTMLEngineTests: XCTestCase {
    private var windows: [NSWindow] = []

    override func tearDown() {
        windows.forEach { $0.close() }
        windows = []
        super.tearDown()
    }

    /// An open editor document in a window (NSTextView needs one for its undo manager).
    private func openDocument(_ url: URL) throws -> EditorDocument {
        let doc = try XCTUnwrap(EditorDocument(url: url, fontSize: 12))
        let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 400, height: 300), styleMask: [.titled],
                         backing: .buffered, defer: true)
        w.isReleasedWhenClosed = false
        w.contentView = doc.scrollView
        w.makeFirstResponder(doc.textView)
        windows.append(w)
        return doc
    }

    // MARK: Applying edits + undo (no engine needed)

    func testEngineEditsApplyAsOneUndoableChange() throws {
        let dir = try TempDir()
        let page = try dir.file("index.html", "<h1 class=\"t\">Hi</h1>\n<p>One</p>\n")
        let css = try dir.file("site.css", ".t {\n  color: blue;\n}\n")
        let doc = try openDocument(page)
        // What `html/edit` returns (UTF-16 ranges): two edits in the page, one in a closed stylesheet.
        let json: [String: Any] = [
            "message": "Updated", "structural": true,
            "files": [
                ["path": page.path, "written": false, "edits": [
                    ["location": 14, "length": 2, "text": "Hello ✨"],
                    ["location": 25, "length": 3, "text": "Two"]]],
                ["path": css.path, "written": false, "edits": [["location": 14, "length": 4, "text": "red"]]],
            ],
        ]
        let edits = HTMLEdits(json: json)
        XCTAssertEqual(edits.message, "Updated")
        XCTAssertTrue(edits.structural)
        let changed = try HTMLEdits.apply(edits.files) { $0.standardizedFileURL == page.standardizedFileURL ? doc : nil }
        XCTAssertEqual(changed.count, 1)
        XCTAssertEqual(doc.text, "<h1 class=\"t\">Hello ✨</h1>\n<p>Two</p>\n")
        XCTAssertEqual(try String(contentsOf: css, encoding: .utf8), ".t {\n  color: red;\n}\n", "closed file patched")

        let undo = try XCTUnwrap(doc.textView.undoManager)
        XCTAssertTrue(undo.canUndo)
        undo.undo()
        XCTAssertEqual(doc.text, "<h1 class=\"t\">Hi</h1>\n<p>One</p>\n", "one undo reverts every edit in the page")
        undo.redo()
        XCTAssertEqual(doc.text, "<h1 class=\"t\">Hello ✨</h1>\n<p>Two</p>\n")
    }

    func testWrittenFilesAreLeftAlone() throws {
        let dir = try TempDir()
        let css = try dir.file("site.css", "a {}\n")
        let edits = HTMLEdits(json: ["files": [["path": css.path, "written": true,
                                                "edits": [["location": 0, "length": 1, "text": "b"]]]]])
        XCTAssertEqual(try HTMLEdits.apply(edits.files) { _ in nil }.count, 0)
        XCTAssertEqual(try String(contentsOf: css, encoding: .utf8), "a {}\n")
    }

    func testPageMapNavigation() {
        let json: [String: Any] = [
            "annotated": "x", "stylesheets": ["/tmp/a.css"],
            "nodes": [
                ["index": 0, "tag": "div", "label": "div.box", "path": [0], "children": [1, 2], "selectable": true,
                 "startTag": ["location": 0, "length": 17], "endTag": ["location": 33, "length": 6],
                 "classes": ["box"], "inline": [["color", "red"]], "textOnly": false],
                ["index": 1, "tag": "b", "label": "b", "path": [0, 0], "parent": 0, "children": [], "selectable": true,
                 "startTag": ["location": 17, "length": 3], "endTag": ["location": 22, "length": 4], "textOnly": true],
                ["index": 2, "tag": "i", "label": "i", "path": [0, 1], "parent": 0, "children": [], "selectable": true,
                 "startTag": ["location": 26, "length": 3], "endTag": ["location": 29, "length": 4], "textOnly": false],
            ],
        ]
        let map = HTMLPageMap(json: json, text: "<div class=\"box\"><b>hi</b><i></i></div>")
        XCTAssertEqual(map.roots, [0])
        XCTAssertEqual(map.node(atPath: [0, 1]), 2)
        XCTAssertEqual(map.node(atPath: [0, 5]), 0, "deeper than the tree: the nearest ancestor")
        XCTAssertNil(map.node(atPath: [3]))
        XCTAssertEqual(map.innerText(1), "hi")
        XCTAssertEqual(map.nodes[0].inline.first?.1, "red")
        XCTAssertEqual(map.defaultDestination, .stylesheet(URL(fileURLWithPath: "/tmp/a.css")))
        XCTAssertEqual(StyleDestination.styleBlock.engineValue, "styleBlock")
    }

    // MARK: Shared web scripts

    func testWebScriptsLoadFromSharedWeb() throws {
        for name in ["visual-editor", "browser-read", "browser-click", "browser-type", "browser-eval",
                     "markdown-report-size", "mermaid-render"] {
            XCTAssertFalse(WebScripts.source(name).isEmpty, name)
        }
        XCTAssertTrue(HTMLVisualSession.selectionScript.hasPrefix("window.__dgProps = [\"display\""))
        XCTAssertTrue(HTMLVisualSession.selectionScript.contains("window.__dgSetRule"))
        let call = try WebScripts.call("browser-type", ["#q", "a \"b\"", true])
        XCTAssertTrue(call.hasSuffix(")(\"#q\", \"a \\\"b\\\"\", true)"), call)
        let page = VisualView.mermaidPage("graph TD; A-->B", dark: true)
        XCTAssertTrue(page.contains("window.__dgMermaidTheme = 'dark'"))
        XCTAssertTrue(page.contains("mermaid.render(") && page.contains("const report"))
    }

    // MARK: End to end against dg engine

    func testVisualEditPathThroughTheEngine() throws {
        let engine = try TestEngine()
        let dir = try TempDir()
        let source = "<html><head><link rel=\"stylesheet\" href=\"site.css\"></head>\n<body>\n  <h1 class=\"title\">Hello</h1>\n</body></html>\n"
        let page = try dir.file("index.html", source)
        let css = try dir.file("site.css", ".title {\n  color: blue;\n}\n")
        let doc = try openDocument(page)

        let parsed = HTMLPageMap(json: try engine.call("html/parse", ["path": page.path, "source": doc.text]) as? [String: Any] ?? [:],
                                 text: doc.text)
        let h1 = try XCTUnwrap(parsed.nodes.firstIndex { $0.tag == "h1" })
        XCTAssertEqual(parsed.label(h1), "h1.title")
        XCTAssertEqual(parsed.innerText(h1), "Hello")
        XCTAssertEqual(parsed.defaultDestination, .stylesheet(css))
        XCTAssertTrue(parsed.annotated.contains("data-dg-id=\"\(h1)\""))

        // Text edit in the open page: applied to the document, undoable.
        var edits = HTMLEdits(json: try engine.call("html/edit", [
            "path": page.path, "source": doc.text, "buffers": HTMLEngine.buffers([doc]), "op": "set_text",
            "element": h1, "text": "Hi & bye", "write": true]))
        try HTMLEdits.apply(edits.files) { _ in doc }
        XCTAssertEqual(doc.text, source.replacingOccurrences(of: ">Hello<", with: ">Hi &amp; bye<"))
        XCTAssertEqual(try String(contentsOf: page, encoding: .utf8), source, "the open page isn't written behind the editor's back")
        doc.textView.undoManager?.undo()
        XCTAssertEqual(doc.text, source)

        // A rule in the closed stylesheet: the engine writes it.
        edits = HTMLEdits(json: try engine.call("html/edit", [
            "path": page.path, "source": doc.text, "buffers": HTMLEngine.buffers([doc]), "op": "style", "element": h1,
            "destination": css.path, "selector": ".title", "properties": ["color": "green"], "write": true]))
        XCTAssertEqual(edits.message, ".title in site.css")
        XCTAssertTrue(edits.files.allSatisfy(\.written))
        XCTAssertEqual(try String(contentsOf: css, encoding: .utf8), ".title {\n  color: green;\n}\n")
    }

    func testPolicyTablesMatcher() throws {
        let engine = try TestEngine()
        let saved = ArtifactPolicy.tables
        defer { if let saved { ArtifactPolicy.install(saved) } }
        ArtifactPolicy.install(.init(json: try engine.call("policy/tables") as? [String: Any] ?? [:]))
        let u = { (s: String) in URL(string: s)! }
        for s in ["http://localhost:5173/", "https://myapp.vercel.app/", "https://claude.ai/artifact/abc", "about:blank",
                  "https://chatgpt.com/canvas/shared/1"] {
            XCTAssertTrue(ArtifactPolicy.isAllowed(u(s)), s)
        }
        XCTAssertTrue(ArtifactPolicy.isAllowed(URL(fileURLWithPath: "/tmp/a.SVG")))
        for s in ["https://claude.ai/chat/1", "https://claude.ai.evil.com/artifact/x", "https://example.com/", "javascript:alert(1)"] {
            XCTAssertFalse(ArtifactPolicy.isAllowed(u(s)), s)
        }
        XCTAssertFalse(ArtifactPolicy.isAllowed(URL(fileURLWithPath: "/tmp/run.command")))
        XCTAssertTrue(ArtifactPolicy.isSignIn(u("https://accounts.google.com/o/oauth2/auth")))
        XCTAssertFalse(ArtifactPolicy.isSignIn(u("http://claude.ai/login")))
        for a in ArtifactAccount.all { XCTAssertTrue(ArtifactPolicy.isSignIn(a.loginURL), a.name) }
        XCTAssertTrue(ArtifactPolicy.canHandOff(u("mailto:a@b.c")))
        XCTAssertFalse(ArtifactPolicy.canHandOff(u("file:///Applications/Calculator.app")))
        XCTAssertTrue(ArtifactPolicy.canOpenOnRequest(u("https://example.com/")))
        XCTAssertNotNil(ArtifactPolicy.safariUserAgent)
    }
}

/// `dg engine` with HOME in a temp folder, spoken to directly over stdio (EngineClient.shared would look for the
/// installed binary and the real home).
final class TestEngine {
    private let process = Process()
    private let input = Pipe(), output = Pipe()
    private var buffer = Data()
    private var nextID = 1
    private let home: TempDir

    init() throws {
        let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        guard let bin = ["release", "debug"].map({ repo.appendingPathComponent("engine/target/\($0)/dg") })
            .first(where: { FileManager.default.isExecutableFile(atPath: $0.path) }) else {
            throw XCTSkip("dg isn't built (cd engine && cargo build --release)")
        }
        home = try TempDir()
        // An existing settings.json skips the one-time copy from UserDefaults.
        try home.file("Library/Application Support/DesktopGlass/settings.json", "{}")
        try home.dir("Desktop")
        process.executableURL = bin
        process.arguments = ["engine"]
        var env = ProcessInfo.processInfo.environment
        env["HOME"] = home.root.path
        process.environment = env
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        try process.run()
    }

    deinit {
        try? input.fileHandleForWriting.close()
        if process.isRunning { process.terminate() }
    }

    func call(_ method: String, _ params: [String: Any] = [:]) throws -> Any {
        let id = nextID
        nextID += 1
        var line = try JSONSerialization.data(withJSONObject: ["jsonrpc": "2.0", "id": id, "method": method, "params": params])
        line.append(10)
        try input.fileHandleForWriting.write(contentsOf: line)
        let deadline = Date().addingTimeInterval(20)
        while Date() < deadline {
            while let nl = buffer.firstIndex(of: 10) {
                let msg = buffer.prefix(upTo: nl)
                buffer.removeSubrange(...nl)
                guard let obj = try? JSONSerialization.jsonObject(with: msg) as? [String: Any], obj["id"] as? Int == id,
                      obj["method"] == nil else { continue }
                if let e = obj["error"] as? [String: Any] { throw ControlError(e["message"] as? String ?? "error") }
                return obj["result"] ?? NSNull()
            }
            let data = output.fileHandleForReading.availableData
            if data.isEmpty { break }
            buffer.append(data)
        }
        throw ControlError("dg engine didn't answer \(method)")
    }
}
