import Foundation

// The visual HTML editor's and the html_* tools' client side. Parsing and every edit are computed by the engine
// (engine/crates/dg-core/src/html.rs, served by dg-engine's `html/*` methods); this file holds what the UI keeps:
// a snapshot of the parse for selection and labels, where styles go, and applying returned edits to open documents.

/// Where styles are written. Stylesheets are the default; the HTML file is used only when the user picks it.
enum StyleDestination: Hashable {
    case stylesheet(URL)   // a local .css file linked from the page
    case styleBlock        // <style> inside the HTML file
    case inline            // style="…" on the element (properties only)

    var label: String {
        switch self {
        case .stylesheet(let u): return u.lastPathComponent
        case .styleBlock: return "<style> in this HTML"
        case .inline: return "Inline (style attribute)"
        }
    }

    var isInHTML: Bool { !isStylesheet }
    var isStylesheet: Bool { if case .stylesheet = self { return true }; return false }

    /// The engine's `destination` parameter.
    var engineValue: String {
        switch self {
        case .stylesheet(let u): return u.path
        case .styleBlock: return "styleBlock"
        case .inline: return "inline"
        }
    }
}

/// The engine's parse of a page (`html/parse`): every element with its exact source ranges (UTF-16, like NSRange).
struct HTMLPageMap {
    struct Node {
        let index: Int
        let tag: String
        let label: String
        let path: [Int]
        let parent: Int?
        let children: [Int]
        let selectable: Bool
        let startTag: NSRange
        let endTag: NSRange?
        let id: String?
        let classes: [String]
        let inline: [(String, String)]
        let textOnly: Bool

        var innerRange: NSRange? {
            guard let e = endTag else { return nil }
            return NSRange(location: NSMaxRange(startTag), length: e.location - NSMaxRange(startTag))
        }
    }

    /// The text this map describes.
    let text: NSString
    let nodes: [Node]
    let roots: [Int]
    let stylesheets: [URL]
    /// The source with data-dg-id="N" on every visual element, for the renderer.
    let annotated: String

    static let empty = HTMLPageMap(text: "", nodes: [], roots: [], stylesheets: [], annotated: "")

    init(text: NSString, nodes: [Node], roots: [Int], stylesheets: [URL], annotated: String) {
        self.text = text
        self.nodes = nodes
        self.roots = roots
        self.stylesheets = stylesheets
        self.annotated = annotated
    }

    init(json: [String: Any], text: String) {
        func range(_ v: Any?) -> NSRange? {
            guard let d = v as? [String: Any], let l = d["location"] as? Int, let n = d["length"] as? Int else { return nil }
            return NSRange(location: l, length: n)
        }
        let nodes = (json["nodes"] as? [[String: Any]] ?? []).map { n in
            Node(index: n["index"] as? Int ?? 0, tag: n["tag"] as? String ?? "", label: n["label"] as? String ?? "",
                 path: n["path"] as? [Int] ?? [], parent: n["parent"] as? Int, children: n["children"] as? [Int] ?? [],
                 selectable: n["selectable"] as? Bool ?? false, startTag: range(n["startTag"]) ?? NSRange(),
                 endTag: range(n["endTag"]), id: n["id"] as? String, classes: n["classes"] as? [String] ?? [],
                 inline: HTMLEngine.pairs(n["inline"]), textOnly: n["textOnly"] as? Bool ?? false)
        }
        self.init(text: text as NSString, nodes: nodes, roots: nodes.filter { $0.parent == nil }.map(\.index),
                  stylesheets: (json["stylesheets"] as? [String] ?? []).map { URL(fileURLWithPath: $0) },
                  annotated: json["annotated"] as? String ?? text)
    }

    func label(_ i: Int) -> String { nodes.indices.contains(i) ? nodes[i].label : "" }
    func path(of i: Int) -> [Int] { nodes[i].path }

    /// The node at a tree position (the nearest ancestor when the path goes deeper than the tree).
    func node(atPath path: [Int]) -> Int? {
        guard let first = path.first, first < roots.count else { return nil }
        var cur = roots[first]
        for k in path.dropFirst() {
            let ch = nodes[cur].children
            guard k < ch.count else { return cur }
            cur = ch[k]
        }
        return cur
    }

    /// The source text inside a text-only element.
    func innerText(_ i: Int) -> String? {
        guard nodes[i].textOnly, let r = nodes[i].innerRange, NSMaxRange(r) <= text.length else { return nil }
        return text.substring(with: r)
    }

    /// The default for new rules: the page's first local stylesheet, else its <style> block.
    var defaultDestination: StyleDestination { stylesheets.first.map { .stylesheet($0) } ?? .styleBlock }
}

/// Edits returned by the engine: `{message, structural, files: [{path, written, edits: [{location, length, text}]}]}`.
/// Files marked `written` were written by the engine (they weren't open); the rest are applied to open documents.
struct HTMLEdits {
    struct File {
        let url: URL
        let written: Bool
        let edits: [(NSRange, String)]
    }
    var files: [File] = []
    var message = ""
    var structural = false

    init(files: [File], message: String, structural: Bool) {
        self.files = files
        self.message = message
        self.structural = structural
    }

    init(json: Any) {
        let d = json as? [String: Any] ?? [:]
        message = d["message"] as? String ?? ""
        structural = d["structural"] as? Bool ?? false
        files = HTMLEdits.files(d["files"])
    }

    static func files(_ raw: Any?) -> [File] {
        (raw as? [[String: Any]] ?? []).map { f in
            File(url: URL(fileURLWithPath: f["path"] as? String ?? ""), written: f["written"] as? Bool ?? false,
                 edits: (f["edits"] as? [[String: Any]] ?? []).map { e in
                     (NSRange(location: e["location"] as? Int ?? 0, length: e["length"] as? Int ?? 0), e["text"] as? String ?? "")
                 })
        }
    }

    /// Applies the unwritten edits to open documents (each one undoable change). Files that aren't open anymore are
    /// patched on disk. Returns the documents that changed.
    @MainActor
    @discardableResult
    static func apply(_ files: [File], open: (URL) -> EditorDocument?) throws -> [EditorDocument] {
        var changed: [EditorDocument] = []
        for f in files where !f.written {
            if let doc = open(f.url) {
                doc.applyEdits(f.edits)
                changed.append(doc)
            } else {
                // Closed between the request and the reply: patch the file itself, from the end backwards.
                let out = NSMutableString(string: try String(contentsOf: f.url, encoding: .utf8))
                for (r, s) in f.edits.sorted(by: { $0.0.location > $1.0.location }) where NSMaxRange(r) <= out.length {
                    out.replaceCharacters(in: r, with: s)
                }
                try (out as String).write(to: f.url, atomically: true, encoding: .utf8)
            }
        }
        return changed
    }
}

/// Calls to the engine's `html/*` methods.
@MainActor
enum HTMLEngine {
    nonisolated static func pairs(_ raw: Any?) -> [(String, String)] {
        (raw as? [[String]] ?? []).compactMap { $0.count == 2 ? ($0[0], $0[1]) : nil }
    }

    /// `{path: text}` of every open document (unsaved text included), for the engine to read instead of the disk.
    static func buffers(_ docs: [EditorDocument]) -> [String: String] {
        Dictionary(docs.map { ($0.url.standardizedFileURL.path, $0.text) }, uniquingKeysWith: { a, _ in a })
    }

    static func parse(path: URL, source: String) async throws -> HTMLPageMap {
        let r = try await EngineClient.shared.call("html/parse", ["path": path.path, "source": source])
        return HTMLPageMap(json: r as? [String: Any] ?? [:], text: source)
    }

    static func declarations(path: URL, source: String, buffers: [String: String], destination: StyleDestination,
                             selector: String) async throws -> [(String, String)] {
        pairs(try await EngineClient.shared.call("html/declarations", [
            "path": path.path, "source": source, "buffers": buffers, "destination": destination.engineValue,
            "selector": selector]))
    }

    /// One `html/edit` op (see dg-engine methods/tools/html.rs). Closed files are written by the engine.
    static func edit(path: URL, source: String, buffers: [String: String], op: String,
                     _ args: [String: Any]) async throws -> HTMLEdits {
        var p: [String: Any] = ["path": path.path, "source": source, "buffers": buffers, "op": op, "write": true]
        p.merge(args) { _, new in new }
        return HTMLEdits(json: try await EngineClient.shared.call("html/edit", p))
    }
}
