import AppKit
import WebKit

/// A diagram or image in a Markdown file, shown under its markup in Preview mode.
struct MarkdownVisual: Hashable {
    enum Kind: Hashable {
        case mermaid(String)          // ```mermaid … ```
        case svgCode(String)          // ```svg … ```
        case image(src: String, alt: String)
    }
    let kind: Kind
    let range: NSRange                // the markup (whole fenced block, or the image syntax)
    let paragraph: NSRange            // the paragraph the visual is shown under
    let standalone: Bool              // markup is alone in its paragraph → hide it when the caret is elsewhere

    var key: String {
        switch kind {
        case .mermaid(let c): return "m:" + c
        case .svgCode(let c): return "s:" + c
        case .image(let src, _): return "i:" + src
        }
    }

    static func == (a: MarkdownVisual, b: MarkdownVisual) -> Bool { a.key == b.key && a.range == b.range }
    func hash(into h: inout Hasher) { h.combine(key); h.combine(range.location) }

    // MARK: Detection

    private static let fence = try! NSRegularExpression(
        pattern: "^```[ \\t]*([A-Za-z0-9_+-]*)[^\\n]*\\n((?:.*\\n)*?)```[ \\t]*$", options: [.anchorsMatchLines])
    private static let mdImage = try! NSRegularExpression(pattern: "!\\[([^\\]\\n]*)\\]\\(\\s*<?([^)\\s>]+)>?(?:\\s+\"[^\"]*\")?\\s*\\)")
    private static let htmlImage = try! NSRegularExpression(
        pattern: "<img\\b[^>]*?\\bsrc\\s*=\\s*[\"']([^\"']+)[\"'][^>]*>", options: [.caseInsensitive])
    private static let htmlAlt = try! NSRegularExpression(pattern: "\\balt\\s*=\\s*[\"']([^\"']*)[\"']", options: [.caseInsensitive])

    static func find(in text: String) -> [MarkdownVisual] {
        let ns = text as NSString
        let full = NSRange(location: 0, length: ns.length)
        var out: [MarkdownVisual] = []
        var codeRanges: [NSRange] = []

        fence.enumerateMatches(in: text, range: full) { m, _, _ in
            guard let m else { return }
            codeRanges.append(m.range)
            let lang = ns.substring(with: m.range(at: 1)).lowercased()
            let body = ns.substring(with: m.range(at: 2))
            let kind: Kind
            switch lang {
            case "mermaid": kind = .mermaid(body)
            case "svg": kind = .svgCode(body)
            default: return
            }
            out.append(MarkdownVisual(kind: kind, range: m.range, paragraph: m.range, standalone: true))
        }

        func addImage(_ r: NSRange, src: String, alt: String) {
            guard !codeRanges.contains(where: { NSIntersectionRange($0, r).length > 0 }) else { return }
            let para = ns.paragraphRange(for: r)
            let rest = ns.substring(with: para).replacingOccurrences(of: ns.substring(with: r), with: "")
            let alone = rest.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            out.append(MarkdownVisual(kind: .image(src: src, alt: alt), range: r, paragraph: para, standalone: alone))
        }
        mdImage.enumerateMatches(in: text, range: full) { m, _, _ in
            guard let m else { return }
            addImage(m.range, src: ns.substring(with: m.range(at: 2)), alt: ns.substring(with: m.range(at: 1)))
        }
        htmlImage.enumerateMatches(in: text, range: full) { m, _, _ in
            guard let m else { return }
            let tag = ns.substring(with: m.range)
            let alt = htmlAlt.firstMatch(in: tag, range: NSRange(location: 0, length: (tag as NSString).length))
                .map { (tag as NSString).substring(with: $0.range(at: 1)) } ?? ""
            addImage(m.range, src: ns.substring(with: m.range(at: 1)), alt: alt)
        }
        return out.sorted { $0.range.location < $1.range.location }
    }
}

/// Renders the visuals as subviews of the text view, in space the styler reserves under each paragraph.
/// Nothing is inserted into the text, so the file on disk is untouched.
@MainActor
final class MarkdownVisualsController {
    private weak var textView: NSTextView?
    private let baseURL: URL                       // folder of the Markdown file, for relative image paths
    private var views: [String: VisualView] = [:]
    private var visuals: [MarkdownVisual] = []
    /// Height reserved under each paragraph (keyed by paragraph start), measured from the rendered views.
    private(set) var reserved: [Int: CGFloat] = [:]
    var onSizeChange: (() -> Void)?

    init(textView: NSTextView, baseURL: URL) {
        self.textView = textView
        self.baseURL = baseURL
    }

    /// Visuals hidden because the caret is inside their markup (so the source can be edited).
    func expanded(_ visuals: [MarkdownVisual], caret: NSRange) -> Set<String> {
        Set(visuals.filter { v in
            NSLocationInRange(caret.location, NSRange(location: v.range.location, length: v.range.length + 1))
                || NSIntersectionRange(caret, v.range).length > 0
        }.map { "\($0.key)@\($0.range.location)" })
    }

    /// Creates, reuses and removes views to match the current visuals, then lays them out.
    func sync(_ new: [MarkdownVisual], expanded: Set<String>) {
        guard let tv = textView else { return }
        visuals = new.filter { !expanded.contains("\($0.key)@\($0.range.location)") }
        let wanted = Set(visuals.map(viewID))
        for (id, v) in views where !wanted.contains(id) { v.removeFromSuperview(); views[id] = nil }
        for v in visuals {
            let id = viewID(v)
            if views[id] == nil {
                let view = VisualView(kind: v.kind, baseURL: baseURL, dark: isDark(tv))
                view.onSize = { [weak self] in self?.measureAndNotify() }
                let location = v.range.location
                view.onClick = { [weak tv] in                     // click a diagram → edit its source
                    guard let tv else { return }
                    tv.window?.makeFirstResponder(tv)
                    tv.setSelectedRange(NSRange(location: min(location + 1, (tv.string as NSString).length), length: 0))
                }
                view.toolTip = "Click to edit the source"
                tv.addSubview(view)
                views[id] = view
            }
        }
        layout()
    }

    func removeAll() {
        for v in views.values { v.removeFromSuperview() }
        views = [:]
        visuals = []
        reserved = [:]
    }

    /// Flow layout: visuals under the same paragraph sit side by side (badges) and wrap.
    func computeReserved(width: CGFloat) -> [Int: CGFloat] {
        var result: [Int: CGFloat] = [:]
        for (para, items) in Dictionary(grouping: visuals, by: { $0.paragraph.location }) {
            var x: CGFloat = 0, rowH: CGFloat = 0, total: CGFloat = 0
            for v in items.sorted(by: { $0.range.location < $1.range.location }) {
                let s = views[viewID(v)]?.displaySize(maxWidth: width) ?? NSSize(width: width, height: 60)
                if x > 0 && x + s.width > width { total += rowH + 8; x = 0; rowH = 0 }
                x += s.width + 8
                rowH = max(rowH, s.height)
            }
            result[para] = total + rowH + 14
        }
        return result
    }

    func layout() {
        guard let tv = textView, let lm = tv.layoutManager, let tc = tv.textContainer else { return }
        let width = max(80, tc.containerSize.width - tc.lineFragmentPadding * 2)
        let length = (tv.string as NSString).length
        guard length > 0 else { return }
        lm.ensureLayout(for: tc)
        let origin = tv.textContainerOrigin
        for (_, items) in Dictionary(grouping: visuals, by: { $0.paragraph.location }) {
            let sorted = items.sorted { $0.range.location < $1.range.location }
            guard let first = sorted.first else { continue }
            // bottom of the paragraph's last line (its used rect excludes the reserved spacing)
            var endChar = NSMaxRange(first.paragraph) - 1
            while endChar > first.paragraph.location,
                  let c = (tv.string as NSString).substring(with: NSRange(location: endChar, length: 1)).first, c.isNewline {
                endChar -= 1
            }
            let glyph = lm.glyphIndexForCharacter(at: min(max(0, endChar), length - 1))
            let used = lm.lineFragmentUsedRect(forGlyphAt: glyph, effectiveRange: nil)
            var x: CGFloat = 0, y = origin.y + used.maxY + 8, rowH: CGFloat = 0
            for v in sorted {
                guard let view = views[viewID(v)] else { continue }
                let s = view.displaySize(maxWidth: width)
                if x > 0 && x + s.width > width { y += rowH + 8; x = 0; rowH = 0 }
                view.frame = NSRect(x: origin.x + tc.lineFragmentPadding + x, y: y, width: s.width, height: s.height)
                x += s.width + 8
                rowH = max(rowH, s.height)
            }
        }
    }

    private func measureAndNotify() {
        guard let tv = textView, let tc = tv.textContainer else { return }
        let fresh = computeReserved(width: max(80, tc.containerSize.width - tc.lineFragmentPadding * 2))
        if fresh != reserved { reserved = fresh; onSizeChange?() } else { layout() }
    }

    func updateReserved() {
        guard let tv = textView, let tc = tv.textContainer else { return }
        reserved = computeReserved(width: max(80, tc.containerSize.width - tc.lineFragmentPadding * 2))
    }

    private func viewID(_ v: MarkdownVisual) -> String { v.key + "@\(v.range.location)" }

    private func isDark(_ view: NSView) -> Bool {
        view.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    }
}

/// One rendered visual: a native image view for raster images, a small web view for diagrams and SVG.
@MainActor
final class VisualView: NSView, WKScriptMessageHandler {
    private var imageView: NSImageView?
    private var web: PassThroughWebView?
    private var natural = NSSize(width: 320, height: 60)      // reported/loaded content size
    private let caption = NSTextField(labelWithString: "")
    var onSize: (() -> Void)?
    var onClick: (() -> Void)?

    // The whole visual is one target: clicks edit the source, scrolling scrolls the document.
    override func hitTest(_ point: NSPoint) -> NSView? { frame.contains(point) ? self : nil }
    override func mouseDown(with event: NSEvent) { onClick?() }
    override func scrollWheel(with event: NSEvent) { superview?.scrollWheel(with: event) }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }

    init(kind: MarkdownVisual.Kind, baseURL: URL, dark: Bool) {
        super.init(frame: NSRect(x: 0, y: 0, width: 320, height: 60))
        wantsLayer = true
        layer?.cornerRadius = 8
        switch kind {
        case .mermaid(let code): showHTML(Self.mermaidPage(code, dark: dark))
        case .svgCode(let code): showHTML(Self.svgPage(code))
        case .image(let src, let alt): loadImage(src, alt: alt, baseURL: baseURL)
        }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    func displaySize(maxWidth: CGFloat) -> NSSize {
        if web != nil { return NSSize(width: maxWidth, height: max(24, natural.height)) }   // diagrams use the full width
        let scale = natural.width > maxWidth ? maxWidth / natural.width : 1
        return NSSize(width: max(16, natural.width * scale), height: max(16, natural.height * scale))
    }

    override func layout() {
        super.layout()
        imageView?.frame = bounds
        web?.frame = bounds
        caption.frame = bounds.insetBy(dx: 8, dy: 6)
    }

    // MARK: Images

    private func loadImage(_ src: String, alt: String, baseURL: URL) {
        let url: URL?
        if src.hasPrefix("http://") || src.hasPrefix("https://") { url = URL(string: src) }
        else if src.hasPrefix("file://") { url = URL(string: src) }
        else {
            let path = (src.removingPercentEncoding ?? src).components(separatedBy: "#").first ?? src
            url = path.hasPrefix("/") ? URL(fileURLWithPath: path) : baseURL.appendingPathComponent(path).standardizedFileURL
        }
        guard let url else { showMissing(alt, src); return }
        let isSVG = url.pathExtension.lowercased() == "svg" || src.contains("img.shields.io") || src.contains("badge")
        if url.isFileURL {
            if isSVG, let svg = try? String(contentsOf: url, encoding: .utf8) { showHTML(Self.svgPage(svg)); return }
            if let img = NSImage(contentsOf: url) { showImage(img) } else { showMissing(alt, src) }
            return
        }
        URLSession.shared.dataTask(with: url) { data, response, _ in
            let type = (response as? HTTPURLResponse)?.value(forHTTPHeaderField: "Content-Type")?.lowercased() ?? ""
            DispatchQueue.main.async {
                guard let data, !data.isEmpty else { self.showMissing(alt, src); return }
                if type.contains("svg") || isSVG, let svg = String(data: data, encoding: .utf8), svg.contains("<svg") {
                    self.showHTML(Self.svgPage(svg))
                } else if let img = NSImage(data: data) {
                    self.showImage(img)
                } else {
                    self.showMissing(alt, src)
                }
            }
        }.resume()
    }

    private func showImage(_ img: NSImage) {
        let iv = NSImageView(image: img)
        iv.imageScaling = .scaleProportionallyUpOrDown
        iv.animates = true
        addSubview(iv)
        imageView = iv
        let rep = img.representations.first
        let px = rep.map { NSSize(width: CGFloat($0.pixelsWide), height: CGFloat($0.pixelsHigh)) } ?? img.size
        // treat high-res images as 2x so screenshots don't render twice their size
        natural = (px.width > 0 && px.width >= img.size.width * 1.9) ? NSSize(width: px.width / 2, height: px.height / 2) : img.size
        needsLayout = true
        onSize?()
    }

    private func showMissing(_ alt: String, _ src: String) {
        caption.stringValue = "🖼  " + (alt.isEmpty ? src : alt) + " — couldn't load"
        caption.textColor = .secondaryLabelColor
        caption.font = .systemFont(ofSize: 11)
        caption.lineBreakMode = .byTruncatingMiddle
        addSubview(caption)
        layer?.backgroundColor = NSColor.secondaryLabelColor.withAlphaComponent(0.08).cgColor
        natural = NSSize(width: 360, height: 28)
        needsLayout = true
        onSize?()
    }

    // MARK: Web content (Mermaid, SVG)

    private func showHTML(_ html: String) {
        let config = WKWebViewConfiguration()
        config.userContentController.add(WeakMessageHandler(self), name: "size")
        let w = PassThroughWebView(frame: bounds, configuration: config)
        w.setValue(false, forKey: "drawsBackground")
        addSubview(w)
        web = w
        let dir = MermaidAssets.workDirectory
        let file = dir.appendingPathComponent(UUID().uuidString + ".html")
        do {
            try html.write(to: file, atomically: true, encoding: .utf8)
            w.loadFileURL(file, allowingReadAccessTo: dir)
        } catch {
            w.loadHTMLString(html, baseURL: nil)
        }
    }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let d = message.body as? [String: Any], let h = d["h"] as? Double else { return }
        let w = d["w"] as? Double ?? Double(natural.width)
        let size = NSSize(width: CGFloat(w), height: CGFloat(h))
        if abs(size.height - natural.height) > 1 { natural = size; onSize?() }
    }

    private static func escapeHTML(_ s: String) -> String {
        s.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;").replacingOccurrences(of: ">", with: "&gt;")
    }

    /// shared/web/markdown-report-size.js: tells the view the rendered size.
    private static var reportSize: String { "<script>\n" + WebScripts.source("markdown-report-size") + "</script>" }

    static func mermaidPage(_ code: String, dark: Bool) -> String {
        """
        <!doctype html><html><head><meta charset="utf-8">
        <style>html,body{margin:0;background:transparent;overflow:hidden;font:13px -apple-system,system-ui}
        #c{display:flex;justify-content:center} #c svg{max-width:100%;height:auto}
        .err{color:#ff6b6b;font:12px ui-monospace,monospace;white-space:pre-wrap;padding:6px}</style>
        <script src="\(MermaidAssets.scriptSource)"></script>\(reportSize)
        </head><body><div id="c"></div>
        <pre id="src" style="display:none">\(escapeHTML(code))</pre>
        <script>window.__dgMermaidTheme = \(dark ? "'dark'" : "'default'");</script>
        <script>
        \(WebScripts.source("mermaid-render"))
        </script></body></html>
        """
    }

    static func svgPage(_ svg: String) -> String {
        // strip scripts and event handlers from the SVG before showing it
        let safe = svg.replacingOccurrences(of: "(?is)<script.*?</script>", with: "", options: .regularExpression)
            .replacingOccurrences(of: "\\son[a-z]+\\s*=\\s*(\"[^\"]*\"|'[^']*')", with: "", options: [.regularExpression, .caseInsensitive])
        return """
        <!doctype html><html><head><meta charset="utf-8">
        <style>html,body{margin:0;background:transparent;overflow:hidden} #c{display:inline-block} #c svg{max-width:100%;height:auto;display:block}</style>
        \(reportSize)</head><body><div id="c">\(safe)</div><script>requestAnimationFrame(report)</script></body></html>
        """
    }
}

/// Scrolling over a diagram scrolls the document, not the little web view.
final class PassThroughWebView: WKWebView {
    override func scrollWheel(with event: NSEvent) { nextResponder?.scrollWheel(with: event) }
}

/// WKUserContentController retains its handlers; this breaks the cycle with VisualView.
final class WeakMessageHandler: NSObject, WKScriptMessageHandler {
    weak var target: WKScriptMessageHandler?
    init(_ target: WKScriptMessageHandler) { self.target = target }
    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        target?.userContentController(c, didReceive: m)
    }
}

/// Mermaid's script: the copy bundled into the app by build-app.sh, or a pinned CDN copy as a fallback.
enum MermaidAssets {
    static let cdn = "https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.min.js"

    static let workDirectory: URL = {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("DesktopGlassVisuals", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        if let bundled = Bundle.main.url(forResource: "mermaid.min", withExtension: "js") {
            let dest = dir.appendingPathComponent("mermaid.min.js")
            try? FileManager.default.removeItem(at: dest)
            try? FileManager.default.copyItem(at: bundled, to: dest)
        }
        return dir
    }()

    static var scriptSource: String {
        FileManager.default.fileExists(atPath: workDirectory.appendingPathComponent("mermaid.min.js").path) ? "mermaid.min.js" : cdn
    }
}
