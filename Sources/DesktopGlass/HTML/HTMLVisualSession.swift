import AppKit
import SwiftUI
import UniformTypeIdentifiers
import WebKit

/// Serves the page being edited (annotated with element ids) plus its CSS, JS and images from the project folder,
/// so relative links work exactly as they do in a browser.
@MainActor
final class PreviewSchemeHandler: NSObject, WKURLSchemeHandler {
    var root: URL = Layout.desktop
    var mainPath = "index.html"
    var mainHTML = Data()

    func webView(_ webView: WKWebView, start task: any WKURLSchemeTask) {
        guard let url = task.request.url else { return }
        var rel = url.path
        while rel.hasPrefix("/") { rel.removeFirst() }
        rel = rel.removingPercentEncoding ?? rel
        let data: Data?
        var mime = "application/octet-stream"
        if rel == mainPath || rel.isEmpty {
            data = mainHTML
            mime = "text/html"
        } else {
            let file = root.appendingPathComponent(rel).standardizedFileURL
            let inside = file.path.hasPrefix(root.standardizedFileURL.path + "/")
            data = inside ? try? Data(contentsOf: file) : nil
            mime = UTType(filenameExtension: file.pathExtension)?.preferredMIMEType ?? mime
        }
        guard let body = data else {
            task.didFailWithError(NSError(domain: NSURLErrorDomain, code: NSURLErrorFileDoesNotExist))
            return
        }
        let response = URLResponse(url: url, mimeType: mime, expectedContentLength: body.count,
                                   textEncodingName: mime.hasPrefix("text/") || mime.contains("javascript") ? "utf-8" : nil)
        task.didReceive(response)
        task.didReceive(body)
        task.didFinish()
    }

    func webView(_ webView: WKWebView, stop task: any WKURLSchemeTask) {}
}

@MainActor
final class VisualMessageHandler: NSObject, WKScriptMessageHandler {
    weak var session: HTMLVisualSession?
    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let body = message.body as? [String: Any] else { return }
        session?.received(body)
    }
}

/// The visual properties the inspector offers, grouped like a design tool.
enum VisualProps {
    struct Prop: Identifiable { let name: String; let options: [String]; var id: String { name }; var isColor: Bool { name.contains("color") } }
    static let groups: [(String, [Prop])] = [
        ("Layout", [Prop(name: "display", options: ["block", "inline", "inline-block", "flex", "inline-flex", "grid", "none"]),
                    Prop(name: "position", options: ["static", "relative", "absolute", "fixed", "sticky"]),
                    Prop(name: "flex-direction", options: ["row", "column", "row-reverse", "column-reverse"]),
                    Prop(name: "justify-content", options: ["flex-start", "center", "flex-end", "space-between", "space-around", "space-evenly"]),
                    Prop(name: "align-items", options: ["stretch", "flex-start", "center", "flex-end", "baseline"]),
                    Prop(name: "flex-wrap", options: ["nowrap", "wrap"]),
                    Prop(name: "gap", options: [])]),
        ("Spacing", [Prop(name: "margin", options: []), Prop(name: "padding", options: [])]),
        ("Size", [Prop(name: "width", options: ["auto", "100%"]), Prop(name: "height", options: ["auto", "100%"]),
                  Prop(name: "max-width", options: ["none", "100%"]), Prop(name: "min-height", options: [])]),
        ("Text", [Prop(name: "font-family", options: ["system-ui, -apple-system, sans-serif", "Georgia, serif", "ui-monospace, monospace"]),
                  Prop(name: "font-size", options: []),
                  Prop(name: "font-weight", options: ["300", "400", "500", "600", "700", "800"]),
                  Prop(name: "line-height", options: ["1", "1.2", "1.5", "1.75"]),
                  Prop(name: "letter-spacing", options: []),
                  Prop(name: "text-align", options: ["left", "center", "right", "justify"]),
                  Prop(name: "color", options: [])]),
        ("Fill & Border", [Prop(name: "background-color", options: []), Prop(name: "background", options: []),
                           Prop(name: "border", options: ["none"]), Prop(name: "border-radius", options: []),
                           Prop(name: "box-shadow", options: ["none", "0 1px 3px rgba(0,0,0,.2)", "0 10px 30px rgba(0,0,0,.25)"])]),
        ("Effects", [Prop(name: "opacity", options: ["1", "0.75", "0.5"]), Prop(name: "overflow", options: ["visible", "hidden", "auto"]),
                     Prop(name: "transform", options: ["none"]), Prop(name: "z-index", options: [])]),
    ]
    static var all: [String] { groups.flatMap { $0.1.map(\.name) } }
}

struct SelectedElement {
    let index: Int
    let tag: String
    let elementID: String
    let classes: [String]
    let computed: [String: String]
    let text: String?
    /// Made by the page's JavaScript: no place in the source, so only CSS rules can style it.
    var isDynamic = false

    var selectors: [String] { classes.map { "." + $0 } + (elementID.isEmpty ? [] : ["#" + elementID]) }
}

/// One HTML file in visual mode: renders it with its own CSS/JS, lets you pick elements, and turns every change
/// into precise edits of the page and its stylesheets (through open documents when they're open, so ⌘Z works).
///
/// Parsing and edits are computed by the engine (`html/*`); the session sends the page text plus every open
/// document's text, then applies the returned edits to its documents. Work runs one step at a time (`enqueue`), so
/// an edit is always computed against the text the previous one left behind.
@MainActor
final class HTMLVisualSession: NSObject, ObservableObject {
    let document: EditorDocument
    let webView: WKWebView
    var openDocument: (URL) -> EditorDocument? = { _ in nil }
    /// Every open editor document (their unsaved text is what the engine reads).
    var openDocuments: () -> [EditorDocument] = { [] }
    private let scheme = PreviewSchemeHandler()
    private let messages = VisualMessageHandler()
    private(set) var map: HTMLPageMap = .empty
    /// Who to select again after the page re-renders (nil = whatever is selected now).
    private var reselect: [(path: [Int], tag: String)]?
    private var reselectTargets: [(path: [Int], tag: String)] = []
    private var lastRenderedText = ""
    private var queue: Task<Void, Never>?
    private var parsedOnce = false

    @Published var interact = false {
        didSet { webView.evaluateJavaScript("window.__dgSetMode && __dgSetMode('\(interact ? "interact" : "select")')", completionHandler: nil) }
    }
    @Published var destination: StyleDestination = .styleBlock { didSet { if destination != oldValue { refreshDeclared() } } }
    @Published var ruleSelector = "" { didSet { if ruleSelector != oldValue { refreshDeclared() } } }
    @Published private(set) var selection: [SelectedElement] = []
    @Published private(set) var message = ""
    /// The declarations of `ruleSelector` at `destination` (fetched from the engine when either changes).
    @Published private(set) var ruleDeclarations: [(String, String)] = []

    var primary: SelectedElement? { selection.first }

    init(document: EditorDocument, projectRoot: URL?) {
        self.document = document
        let config = WKWebViewConfiguration()
        config.setURLSchemeHandler(scheme, forURLScheme: "dg-preview")
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")
        webView = WKWebView(frame: .zero, configuration: config)
        super.init()
        config.userContentController.add(messages, name: "dg")
        messages.session = self
        webView.setValue(false, forKey: "drawsBackground")
        let dir = document.url.deletingLastPathComponent()
        let root = projectRoot.flatMap { document.url.path.hasPrefix($0.path + "/") ? $0 : nil } ?? dir
        scheme.root = root
        scheme.mainPath = String(document.url.standardizedFileURL.path.dropFirst(root.standardizedFileURL.path.count + 1))
        render(keepScroll: false)
    }

    var destinations: [StyleDestination] {
        map.stylesheets.map { StyleDestination.stylesheet($0) } + [.styleBlock, .inline]
    }

    /// Inline declarations of an element in the current parse.
    func inlineDeclarations(_ index: Int) -> [(String, String)] {
        map.nodes.indices.contains(index) ? map.nodes[index].inline : []
    }

    // MARK: Serial work

    /// Runs `work` after everything queued before it.
    private func enqueue(_ work: @escaping @MainActor () async -> Void) {
        let previous = queue
        queue = Task { @MainActor in
            await previous?.value
            await work()
        }
    }

    /// Waits for queued work (tests).
    func settle() async { await queue?.value }

    private var buffers: [String: String] {
        var b = HTMLEngine.buffers(openDocuments())
        b[document.url.standardizedFileURL.path] = document.text
        return b
    }

    // MARK: Rendering

    func render(keepScroll: Bool = true) {
        enqueue { await self.renderNow(keepScroll: keepScroll) }
    }

    private func renderNow(keepScroll: Bool) async {
        // Remember the selection by position in the old tree, then find it again after the reload.
        reselectTargets = reselect ?? selection.filter { !$0.isDynamic && map.nodes.indices.contains($0.index) }
            .map { (path: map.path(of: $0.index), tag: $0.tag) }
        reselect = nil
        selection = []
        guard await reparse() else { return }
        let path = scheme.mainPath.split(separator: "/").map { $0.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? String($0) }
        guard let url = URL(string: "dg-preview://project/" + path.joined(separator: "/")) else { return }
        if keepScroll, webView.url != nil {
            let y = (try? await webView.evaluateJavaScript("window.scrollY")) as? Double
            load(url, scrollTo: y ?? 0)
        } else {
            load(url, scrollTo: 0)
        }
    }

    /// Parses the document's current text in the engine. False (with a message) if the engine couldn't.
    @discardableResult
    private func reparse() async -> Bool {
        let text = document.text
        do {
            map = try await HTMLEngine.parse(path: document.url, source: text)
        } catch {
            message = "Couldn't read the page: \(error.localizedDescription)"
            return false
        }
        lastRenderedText = text
        scheme.mainHTML = Data(map.annotated.utf8)
        if !parsedOnce {
            parsedOnce = true
            destination = map.defaultDestination
        }
        return true
    }

    private func load(_ url: URL, scrollTo y: Double) {
        let ucc = webView.configuration.userContentController
        ucc.removeAllUserScripts()
        ucc.addUserScript(WKUserScript(source: "addEventListener('load', () => scrollTo(0, \(y)));",
                                       injectionTime: .atDocumentStart, forMainFrameOnly: true))
        ucc.addUserScript(WKUserScript(source: Self.selectionScript, injectionTime: .atDocumentEnd, forMainFrameOnly: true))
        webView.load(URLRequest(url: url))
    }

    /// Re-renders if the page was edited in Source mode (or by another app) since the last render.
    func refreshIfSourceChanged() { enqueue { if self.document.text != self.lastRenderedText { await self.renderNow(keepScroll: true) } } }

    // MARK: Selection

    func received(_ body: [String: Any]) {
        enqueue { await self.handle(body) }
    }

    private func handle(_ body: [String: Any]) async {
        switch body["type"] as? String {
        case "loaded":
            if interact { webView.evaluateJavaScript("__dgSetMode('interact')", completionHandler: nil) }
            let targets = reselectTargets
            reselectTargets = []
            var added = false
            for t in targets {
                guard let i = map.node(atPath: t.path), map.nodes[i].selectable, map.nodes[i].tag == t.tag else { continue }
                select(index: i, add: added)
                added = true
            }
            if !added { highlight() }
        case "select":
            handleSelect(body)
        case "editText":
            guard let raw = body["id"] as? String, let index = Int(raw), map.nodes.indices.contains(index),
                  let html = body["html"] as? String else { return }
            if document.text != lastRenderedText {
                message = "The file changed while you were typing — reloaded it. Try again."
                await renderNow(keepScroll: true)
                return
            }
            if !(await commit("set_html", ["element": index, "html": html])) { await renderNow(keepScroll: true) }
        case "editRefused":
            message = "Only text with inline formatting (bold, italic, links, spans…) can be edited in place — edit this one in Source."
        default:
            break
        }
    }

    private func handleSelect(_ body: [String: Any]) {
        guard let raw = body["id"] as? String, let index = Int(raw), map.nodes.indices.contains(index) else { return }
        let dynamic = (body["dynamic"] as? Bool) == true
        let pageTag = (body["tag"] as? String ?? "").lowercased()
        let refresh = (body["refresh"] as? Bool) == true
        // The page and the source must agree, or the inspector would describe (and edit) the wrong element.
        if document.text != lastRenderedText || (!dynamic && !pageTag.isEmpty && map.nodes[index].tag != pageTag) {
            if !refresh {
                message = "The page was out of date with the file — reloaded it. Click the element again."
                render()
            }
            return
        }
        let computed = body["computed"] as? [String: String] ?? [:]
        let el: SelectedElement
        if dynamic {
            el = SelectedElement(index: index, tag: pageTag, elementID: body["elId"] as? String ?? "",
                                 classes: body["classes"] as? [String] ?? [], computed: computed, text: nil, isDynamic: true)
        } else {
            let node = map.nodes[index]
            el = SelectedElement(index: index, tag: node.tag, elementID: node.id ?? "", classes: node.classes,
                                 computed: computed, text: map.innerText(index))
        }
        if refresh {
            if let k = selection.firstIndex(where: { $0.index == index && $0.isDynamic == dynamic }) { selection[k] = el }
            return
        }
        let shift = (body["shift"] as? Bool) == true
        if shift, !selection.isEmpty, !dynamic, !(primary?.isDynamic ?? false) {
            if let k = selection.firstIndex(where: { $0.index == index }) { selection.remove(at: k) } else { selection.append(el) }
        } else {
            selection = [el]
            if !el.selectors.contains(ruleSelector) { ruleSelector = el.selectors.first ?? "" }
            if dynamic && destination == .inline { destination = map.defaultDestination }
        }
        highlight()
    }

    func select(index: Int, add: Bool = false) {
        webView.evaluateJavaScript("window.__dgSelect && __dgSelect('\(index)', \(add))", completionHandler: nil)
    }

    func selectParent() {
        guard let p = primary else { return }
        if p.isDynamic { select(index: p.index); return }      // up to the nearest element that's in the file
        guard let parent = map.nodes[p.index].parent, map.nodes[parent].selectable else { return }
        select(index: parent)
    }

    private func highlight() {
        let ids = selection.map { $0.isDynamic ? "'dyn'" : "'\($0.index)'" }.joined(separator: ",")
        webView.evaluateJavaScript("window.__dgHighlight && __dgHighlight([\(ids)])", completionHandler: nil)
    }

    /// The value currently declared at the chosen destination (inline style or the chosen rule), if any.
    func declaredValue(_ prop: String) -> String? {
        guard let p = primary else { return nil }
        if destination == .inline {
            return p.isDynamic ? nil : inlineDeclarations(p.index).first { $0.0 == prop }?.1
        }
        guard !ruleSelector.isEmpty else { return nil }
        return ruleDeclarations.first { $0.0 == prop }?.1
    }

    /// Fetches the declarations of the chosen rule (after the selector, destination or a file changed).
    private func refreshDeclared() {
        enqueue { await self.fetchDeclared() }
    }

    private func fetchDeclared() async {
        guard destination != .inline, !ruleSelector.isEmpty else { ruleDeclarations = []; return }
        ruleDeclarations = (try? await HTMLEngine.declarations(path: document.url, source: document.text, buffers: buffers,
                                                               destination: destination, selector: ruleSelector)) ?? []
    }

    // MARK: Editing

    enum NeedsName: Equatable { case newClass, newID }

    /// Elements made by JavaScript have no place in the HTML file, so only CSS rules can change them.
    private func inSource(_ p: SelectedElement, _ action: String) -> Bool {
        if p.isDynamic {
            message = "This <\(p.tag)> is created by the page's JavaScript, so there's nothing in the HTML to \(action). Style it with a CSS rule instead."
        }
        return !p.isDynamic
    }

    /// Applies a CSS property at the chosen destination. Returns a name request when a rule needs a class/id first.
    @discardableResult
    func setProperty(_ prop: String, _ value: String) -> NeedsName? {
        guard let p = primary else { return nil }
        let v: String? = value.trimmingCharacters(in: .whitespaces).isEmpty ? nil : value
        if destination == .inline && !inSource(p, "write an inline style on") { return nil }
        if destination != .inline && ruleSelector.isEmpty {
            if p.isDynamic { message = "This element has no class or id — give it one in your script, then style that here."; return nil }
            return .newClass
        }
        let dest = destination
        enqueue {
            // The selector is read now: createClass/createID (queued before this) may have just set it.
            let selector = self.ruleSelector
            let args: [String: Any] = ["element": p.index, "destination": dest.engineValue, "selector": selector,
                                       "properties": [prop: v.map { $0 as Any } ?? NSNull()]]
            guard await self.commit("style", args) else { return }
            if dest == .inline {
                self.js("__dgPreview('\(p.index)', \(self.jsString(prop)), \(self.jsString(v ?? "")))")
            } else {
                let sheet: String
                if case .stylesheet(let u) = dest { sheet = u.lastPathComponent } else { sheet = "" }
                self.js("__dgSetRule(\(self.jsString(selector)), \(self.jsString(prop)), \(self.jsString(v ?? "")), \(self.jsString(sheet)))")
            }
        }
        return nil
    }

    func createClass(_ name: String) {
        guard let p = primary, inSource(p, "add a class to") else { return }
        ruleSelector = "." + name
        enqueue {
            guard await self.commit("add_class", ["element": p.index, "name": name]) else { return }
            self.js("document.querySelector('[data-dg-id=\"\(p.index)\"]').classList.add(\(self.jsString(name)))")
            self.select(index: p.index)
        }
    }

    func createID(_ name: String) {
        guard let p = primary, inSource(p, "set an id on") else { return }
        ruleSelector = "#" + name
        enqueue {
            guard await self.commit("set_id", ["element": p.index, "id": name]) else { return }
            self.js("document.querySelector('[data-dg-id=\"\(p.index)\"]').id = \(self.jsString(name))")
            self.select(index: p.index)
        }
    }

    func setText(_ text: String) {
        guard let p = primary, inSource(p, "edit") else { return }
        enqueue {
            guard await self.commit("set_text", ["element": p.index, "text": text]) else { return }
            self.js("document.querySelector('[data-dg-id=\"\(p.index)\"]').textContent = \(self.jsString(text))")
        }
    }

    /// Split style: this element gets its own class (copied from a shared one, or as a variant on top of it).
    func splitStyle(baseClass: String, newClass: String, copy: Bool, moveInline: Bool, into dest: StyleDestination) {
        guard let p = primary, inSource(p, "change the class of") else { return }
        enqueue {
            guard await self.commit("split_style", ["element": p.index, "baseClass": baseClass, "newClass": newClass,
                                                    "copy": copy, "moveInline": moveInline,
                                                    "destination": dest.engineValue]) else { return }
            self.destination = dest
            self.ruleSelector = "." + newClass
        }
    }

    /// Group: one new class on every selected element, holding the chosen shared properties.
    func groupStyle(className: String, properties: [String: String], moveInline: Bool, into dest: StyleDestination) {
        guard primary != nil, selection.allSatisfy({ inSource($0, "add a class to") }) else { return }
        let indices = selection.map(\.index)
        enqueue {
            guard await self.commit("group_style", ["elements": indices, "className": className, "properties": properties,
                                                    "moveInline": moveInline, "destination": dest.engineValue]) else { return }
            self.destination = dest
            self.ruleSelector = "." + className
        }
    }

    func duplicate() {
        guard let p = primary, inSource(p, "duplicate") else { return }
        enqueue { await self.commit("duplicate", ["element": p.index]) }
    }

    func remove() {
        guard let p = primary, inSource(p, "remove") else { return }
        enqueue { await self.commit("remove", ["element": p.index], reselect: []) }
    }

    func undo() {
        enqueue {
            self.document.textView.undoManager?.undo()
            await self.renderNow(keepScroll: true)
        }
    }

    /// Computes an edit in the engine and writes it: the page through its document, stylesheets through their open
    /// documents (saved, so the renderer sees them); the engine writes files that aren't open. Structural edits
    /// re-render and select again by position. False (with the reason in `message`) if nothing was written.
    @discardableResult
    private func commit(_ op: String, _ args: [String: Any], reselect targets: [(path: [Int], tag: String)]? = nil) async -> Bool {
        let edits: HTMLEdits
        do {
            edits = try await HTMLEngine.edit(path: document.url, source: document.text, buffers: buffers, op: op, args)
            try apply(edits)
        } catch {
            message = error.localizedDescription
            return false
        }
        message = edits.message
        if edits.structural {
            reselect = targets
            await renderNow(keepScroll: true)
        } else {
            await reparse()        // same elements, new offsets
        }
        await fetchDeclared()
        return true
    }

    /// Applies returned edits: the page to this document, other open files to theirs (then saved).
    func apply(_ edits: HTMLEdits) throws {
        let docURL = document.url.standardizedFileURL
        let changed = try HTMLEdits.apply(edits.files) { url in
            url.standardizedFileURL == docURL ? self.document : self.openDocument(url)
        }
        for doc in changed where doc !== document { doc.save() }
    }

    private func js(_ code: String) { webView.evaluateJavaScript("try { \(code) } catch (e) {}", completionHandler: nil) }

    private func jsString(_ s: String) -> String {
        (try? JSONSerialization.data(withJSONObject: s, options: .fragmentsAllowed)).flatMap { String(data: $0, encoding: .utf8) } ?? "\"\""
    }

    // MARK: In-page script

    /// shared/web/visual-editor.js, after the inspector's property list (`window.__dgProps`).
    static let selectionScript: String = {
        let props = (try? JSONSerialization.data(withJSONObject: VisualProps.all)).flatMap { String(data: $0, encoding: .utf8) } ?? "[]"
        return "window.__dgProps = \(props);\n" + WebScripts.source("visual-editor")
    }()
}
