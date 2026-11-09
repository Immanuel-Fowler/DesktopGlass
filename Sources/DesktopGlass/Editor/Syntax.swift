import AppKit

enum SyntaxColor {
    static let keyword = NSColor.systemPink
    static let type = NSColor.systemTeal
    static let string = NSColor.systemOrange
    static let number = NSColor.systemPurple
    static let comment = NSColor.secondaryLabelColor
    static let key = NSColor.systemBlue
    static let tag = NSColor.systemPink
    static let attribute = NSColor.systemTeal
    static let heading = NSColor.systemBlue
}

/// Lightweight, regex + scanner based highlighting. Good enough to read code at a glance,
/// fast enough to re-run on every pause in typing for files up to a few hundred KB.
struct Language {
    enum Kind { case code, markup, markdown, data, plain }

    let name: String
    var kind: Kind = .code
    var keywords: [String] = []
    var caseInsensitiveKeywords = false
    var lineComments: [String] = []
    var blockComment: (open: String, close: String)? = nil
    var quotes: [Character] = ["\"", "'"]
    var multilineQuotes: [String] = []       // e.g. """ or ` (template strings)
    var highlightTypes = false
    var extraRules: [(String, NSColor)] = [] // applied before strings/comments
    var postRules: [(String, NSColor)] = []  // applied after (override strings)

    static let plain = Language(name: "Plain Text", kind: .plain, quotes: [])

    private static let cLike = ["if", "else", "for", "while", "do", "switch", "case", "default", "break", "continue",
                                "return", "struct", "enum", "union", "typedef", "static", "const", "extern", "sizeof",
                                "void", "int", "char", "float", "double", "long", "short", "unsigned", "signed", "bool",
                                "class", "public", "private", "protected", "new", "delete", "this", "namespace", "using",
                                "template", "typename", "virtual", "override", "true", "false", "null", "nullptr",
                                "import", "package", "interface", "extends", "implements", "final", "abstract", "try",
                                "catch", "throw", "throws", "var", "val", "fun", "object", "when", "is", "in", "as"]

    static func detect(_ url: URL) -> Language {
        let ext = url.pathExtension.lowercased()
        let name = url.lastPathComponent.lowercased()
        if name == "makefile" || name == "dockerfile" || name.hasPrefix(".env") || name == ".gitignore" || name == ".zshrc" || name == ".bashrc" {
            return shell
        }
        switch ext {
        case "swift": return swift
        case "js", "jsx", "mjs", "cjs", "ts", "tsx": return javascript
        case "py", "pyw": return python
        case "go": return go
        case "rs": return rust
        case "c", "h", "cpp", "cc", "hpp", "m", "mm", "java", "kt", "kts", "cs", "dart", "scala", "groovy", "gradle":
            return Language(name: ext.uppercased(), keywords: cLike, lineComments: ["//"], blockComment: ("/*", "*/"),
                            multilineQuotes: ["\"\"\""], highlightTypes: true,
                            extraRules: [("@[A-Za-z_]+", SyntaxColor.keyword)])
        case "sh", "bash", "zsh", "fish", "command": return shell
        case "rb", "rake": return ruby
        case "php": return Language(name: "PHP", keywords: cLike + ["function", "echo", "foreach", "elseif", "endif", "require", "include", "fn", "match"],
                                    lineComments: ["//", "#"], blockComment: ("/*", "*/"),
                                    extraRules: [("\\$[A-Za-z_][A-Za-z0-9_]*", SyntaxColor.type)])
        case "sql": return sql
        case "json", "jsonc", "geojson": return json
        case "yaml", "yml": return yaml
        case "toml", "ini", "cfg", "conf", "properties": return toml
        case "html", "htm", "xml", "plist", "svg", "vue", "svelte": return html
        case "css", "scss", "sass", "less": return css
        case "md", "markdown", "mdx": return markdown
        case "lua": return Language(name: "Lua", keywords: ["and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while"],
                                    lineComments: ["--"], blockComment: ("--[[", "]]"))
        default: return plain
        }
    }

    static let swift = Language(
        name: "Swift",
        keywords: ["let", "var", "func", "if", "else", "guard", "return", "import", "struct", "class", "enum", "protocol",
                   "extension", "case", "switch", "default", "for", "in", "while", "repeat", "break", "continue", "true",
                   "false", "nil", "self", "Self", "init", "deinit", "throws", "throw", "rethrows", "try", "catch", "do",
                   "async", "await", "static", "private", "fileprivate", "internal", "public", "open", "final", "override",
                   "mutating", "nonmutating", "lazy", "weak", "unowned", "where", "as", "is", "some", "any", "inout",
                   "typealias", "associatedtype", "subscript", "defer", "fallthrough", "get", "set", "willSet", "didSet",
                   "actor", "nonisolated", "convenience", "required", "operator", "indirect"],
        lineComments: ["//"], blockComment: ("/*", "*/"), quotes: ["\""], multilineQuotes: ["\"\"\""],
        highlightTypes: true,
        extraRules: [("[@#][A-Za-z_][A-Za-z0-9_]*", SyntaxColor.keyword)])

    static let javascript = Language(
        name: "JavaScript / TypeScript",
        keywords: ["const", "let", "var", "function", "return", "if", "else", "for", "while", "do", "switch", "case",
                   "default", "break", "continue", "new", "delete", "typeof", "instanceof", "in", "of", "class", "extends",
                   "super", "this", "import", "export", "from", "as", "async", "await", "try", "catch", "finally", "throw",
                   "yield", "null", "undefined", "true", "false", "void", "interface", "type", "enum", "implements",
                   "public", "private", "protected", "readonly", "static", "abstract", "declare", "namespace", "keyof",
                   "satisfies", "get", "set"],
        lineComments: ["//"], blockComment: ("/*", "*/"), quotes: ["\"", "'", "`"], multilineQuotes: ["`"],
        highlightTypes: true)

    static let python = Language(
        name: "Python",
        keywords: ["def", "class", "return", "if", "elif", "else", "for", "while", "in", "not", "and", "or", "is", "None",
                   "True", "False", "import", "from", "as", "with", "try", "except", "finally", "raise", "pass", "break",
                   "continue", "lambda", "yield", "global", "nonlocal", "async", "await", "assert", "del", "self", "match", "case"],
        lineComments: ["#"], multilineQuotes: ["\"\"\"", "'''"], highlightTypes: true,
        extraRules: [("@[A-Za-z_][A-Za-z0-9_.]*", SyntaxColor.keyword)])

    static let go = Language(
        name: "Go",
        keywords: ["package", "import", "func", "return", "if", "else", "for", "range", "switch", "case", "default", "break",
                   "continue", "go", "defer", "select", "chan", "map", "struct", "interface", "type", "var", "const", "nil",
                   "true", "false", "fallthrough", "goto"],
        lineComments: ["//"], blockComment: ("/*", "*/"), quotes: ["\"", "'", "`"], multilineQuotes: ["`"], highlightTypes: true)

    static let rust = Language(
        name: "Rust",
        keywords: ["fn", "let", "mut", "pub", "struct", "enum", "impl", "trait", "use", "mod", "crate", "self", "Self", "super",
                   "match", "if", "else", "for", "while", "loop", "in", "return", "break", "continue", "as", "ref", "move",
                   "where", "async", "await", "dyn", "true", "false", "const", "static", "type", "unsafe", "extern"],
        lineComments: ["//"], blockComment: ("/*", "*/"), quotes: ["\""], highlightTypes: true,
        extraRules: [("\\b[a-z_]+!", SyntaxColor.keyword), ("#!?\\[[^\\]]*\\]", SyntaxColor.comment)])

    static let shell = Language(
        name: "Shell",
        keywords: ["if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function",
                   "return", "export", "local", "echo", "exit", "source", "alias", "set", "unset", "readonly", "shift", "trap"],
        lineComments: ["#"],
        extraRules: [("\\$\\{?[A-Za-z_][A-Za-z0-9_]*\\}?", SyntaxColor.type)])

    static let ruby = Language(
        name: "Ruby",
        keywords: ["def", "end", "class", "module", "if", "elsif", "else", "unless", "while", "until", "for", "in", "do",
                   "return", "yield", "begin", "rescue", "ensure", "raise", "nil", "true", "false", "self", "require", "then"],
        lineComments: ["#"], highlightTypes: true, extraRules: [(":[A-Za-z_][A-Za-z0-9_]*", SyntaxColor.type)])

    static let sql = Language(
        name: "SQL",
        keywords: ["select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set", "delete",
                   "create", "table", "alter", "drop", "index", "join", "left", "right", "inner", "outer", "on", "group",
                   "by", "order", "having", "limit", "offset", "as", "distinct", "null", "primary", "key", "foreign",
                   "references", "default", "union", "case", "when", "then", "else", "end", "is", "in", "like", "returning"],
        caseInsensitiveKeywords: true, lineComments: ["--"], blockComment: ("/*", "*/"), quotes: ["'", "\""])

    static let json = Language(
        name: "JSON", kind: .data, keywords: ["true", "false", "null"], quotes: ["\""],
        postRules: [("\"(?:[^\"\\\\\\n]|\\\\.)*\"(?=\\s*:)", SyntaxColor.key)])

    static let yaml = Language(
        name: "YAML", kind: .data, keywords: ["true", "false", "null", "yes", "no", "on", "off"], lineComments: ["#"],
        extraRules: [("(?m)^\\s*-?\\s*[A-Za-z0-9_.\\-\"']+(?=\\s*:)", SyntaxColor.key), ("(?m)^---$", SyntaxColor.comment)])

    static let toml = Language(
        name: "TOML / INI", kind: .data, keywords: ["true", "false"], lineComments: ["#", ";"],
        extraRules: [("(?m)^\\s*\\[[^\\]]*\\]", SyntaxColor.keyword), ("(?m)^\\s*[A-Za-z0-9_.\\-]+(?=\\s*=)", SyntaxColor.key)])

    static let html = Language(
        name: "HTML / XML", kind: .markup, blockComment: ("<!--", "-->"), quotes: ["\"", "'"],
        extraRules: [("</?[A-Za-z][A-Za-z0-9:\\-]*|/?>", SyntaxColor.tag),
                     ("\\b[A-Za-z_:][A-Za-z0-9_:.\\-]*(?==)", SyntaxColor.attribute),
                     ("&[A-Za-z#0-9]+;", SyntaxColor.number)])

    static let css = Language(
        name: "CSS", kind: .markup, lineComments: [], blockComment: ("/*", "*/"),
        extraRules: [("[A-Za-z\\-]+(?=\\s*:[^;{]*[;}])", SyntaxColor.key),
                     ("#[0-9A-Fa-f]{3,8}\\b", SyntaxColor.number),
                     ("@[A-Za-z\\-]+", SyntaxColor.keyword),
                     ("[.#][A-Za-z_][A-Za-z0-9_\\-]*(?=[^;{}]*\\{)", SyntaxColor.type)])

    static let markdown = Language(
        name: "Markdown", kind: .markdown, quotes: [],
        extraRules: [("(?m)^#{1,6} .*$", SyntaxColor.heading),
                     ("(?m)^\\s*([-*+]|\\d+\\.) ", SyntaxColor.keyword),
                     ("\\*\\*[^*\\n]+\\*\\*|__[^_\\n]+__", SyntaxColor.type),
                     ("\\[[^\\]\\n]+\\]\\([^)\\n]+\\)", SyntaxColor.key),
                     ("(?m)^>.*$", SyntaxColor.comment),
                     ("`[^`\\n]+`", SyntaxColor.string),
                     ("(?s)```.*?```", SyntaxColor.string)])
}

enum SyntaxHighlighter {
    private static var cache: [String: [(NSRegularExpression, NSColor)]] = [:]
    private static var postCache: [String: [(NSRegularExpression, NSColor)]] = [:]

    private static func compiled(_ lang: Language) -> (pre: [(NSRegularExpression, NSColor)], post: [(NSRegularExpression, NSColor)]) {
        if let pre = cache[lang.name], let post = postCache[lang.name] { return (pre, post) }
        var pre: [(NSRegularExpression, NSColor)] = []
        func add(_ pattern: String, _ color: NSColor, _ opts: NSRegularExpression.Options = [], to list: inout [(NSRegularExpression, NSColor)]) {
            if let re = try? NSRegularExpression(pattern: pattern, options: opts) { list.append((re, color)) }
        }
        if lang.kind != .markdown && lang.kind != .plain {
            add("\\b(?:0x[0-9A-Fa-f_]+|\\d[\\d_]*(?:\\.\\d+)?(?:[eE][+-]?\\d+)?)\\b", SyntaxColor.number, to: &pre)
        }
        if lang.highlightTypes { add("\\b[A-Z][A-Za-z0-9_]*\\b", SyntaxColor.type, to: &pre) }
        if !lang.keywords.isEmpty {
            let words = lang.keywords.map(NSRegularExpression.escapedPattern(for:)).joined(separator: "|")
            add("\\b(?:\(words))\\b", SyntaxColor.keyword, lang.caseInsensitiveKeywords ? [.caseInsensitive] : [], to: &pre)
        }
        for (pattern, color) in lang.extraRules { add(pattern, color, to: &pre) }
        var post: [(NSRegularExpression, NSColor)] = []
        for (pattern, color) in lang.postRules { add(pattern, color, to: &post) }
        cache[lang.name] = pre
        postCache[lang.name] = post
        return (pre, post)
    }

    /// Ranges and colors, in application order (later entries win).
    static func tokens(in text: String, language lang: Language) -> [(NSRange, NSColor)] {
        if lang.kind == .plain { return [] }
        let ns = text as NSString
        let full = NSRange(location: 0, length: ns.length)
        var out: [(NSRange, NSColor)] = []
        let rules = compiled(lang)
        for (re, color) in rules.pre {
            re.enumerateMatches(in: text, range: full) { m, _, _ in if let r = m?.range { out.append((r, color)) } }
        }
        out.append(contentsOf: scanStringsAndComments(ns, lang))
        for (re, color) in rules.post {
            re.enumerateMatches(in: text, range: full) { m, _, _ in if let r = m?.range { out.append((r, color)) } }
        }
        return out
    }

    /// Single pass so a "//" inside a string isn't a comment and quotes inside comments aren't strings.
    private static func scanStringsAndComments(_ ns: NSString, _ lang: Language) -> [(NSRange, NSColor)] {
        let len = ns.length
        guard len > 0 else { return [] }
        var buf = [unichar](repeating: 0, count: len)
        ns.getCharacters(&buf, range: NSRange(location: 0, length: len))
        let lineComments = lang.lineComments.map { Array($0.utf16) }
        let block = lang.blockComment.map { (Array($0.open.utf16), Array($0.close.utf16)) }
        let multi = lang.multilineQuotes.map { Array($0.utf16) }.sorted { $0.count > $1.count }
        let quotes = Set(lang.quotes.compactMap { $0.utf16.first })
        let newline: unichar = 10, backslash: unichar = 92

        func matches(_ seq: [unichar], at i: Int) -> Bool {
            guard i + seq.count <= len else { return false }
            for k in 0..<seq.count where buf[i + k] != seq[k] { return false }
            return true
        }
        func find(_ seq: [unichar], from start: Int, escapes: Bool) -> Int? {
            var j = start
            while j + seq.count <= len {
                if escapes && buf[j] == backslash { j += 2; continue }
                if matches(seq, at: j) { return j }
                j += 1
            }
            return nil
        }

        var out: [(NSRange, NSColor)] = []
        var i = 0
        while i < len {
            if let (open, close) = block, matches(open, at: i) {
                let end = find(close, from: i + open.count, escapes: false).map { $0 + close.count } ?? len
                out.append((NSRange(location: i, length: end - i), SyntaxColor.comment)); i = end; continue
            }
            if let lc = lineComments.first(where: { matches($0, at: i) }) {
                // '#' only starts a comment at line start or after whitespace (keeps "$#", "a#b" intact)
                let ok = lc != [35] || i == 0 || buf[i - 1] == 32 || buf[i - 1] == 9 || buf[i - 1] == newline
                if ok {
                    var end = i
                    while end < len && buf[end] != newline { end += 1 }
                    out.append((NSRange(location: i, length: end - i), SyntaxColor.comment)); i = end; continue
                }
            }
            if let mq = multi.first(where: { matches($0, at: i) }) {
                let end = find(mq, from: i + mq.count, escapes: true).map { $0 + mq.count } ?? len
                out.append((NSRange(location: i, length: end - i), SyntaxColor.string)); i = end; continue
            }
            let c = buf[i]
            if quotes.contains(c) {
                var j = i + 1
                while j < len && buf[j] != c && buf[j] != newline { j += buf[j] == backslash ? 2 : 1 }
                let end = min(j + 1, len)
                out.append((NSRange(location: i, length: end - i), SyntaxColor.string)); i = end; continue
            }
            i += 1
        }
        return out
    }
}

/// Line numbers in the gutter (TextKit 1), transparent so the glass shows through.
final class LineNumberRuler: NSRulerView {
    private weak var textView: NSTextView?
    private var lineStarts: [Int] = [0]
    private var lineStartsValid = false

    init(textView: NSTextView) {
        self.textView = textView
        super.init(scrollView: textView.enclosingScrollView, orientation: .verticalRuler)
        clientView = textView
        ruleThickness = 44
        NotificationCenter.default.addObserver(self, selector: #selector(textChanged), name: NSText.didChangeNotification, object: textView)
        NotificationCenter.default.addObserver(self, selector: #selector(redraw), name: NSTextView.didChangeSelectionNotification, object: textView)
        if let clip = textView.enclosingScrollView?.contentView {
            clip.postsBoundsChangedNotifications = true
            NotificationCenter.default.addObserver(self, selector: #selector(redraw), name: NSView.boundsDidChangeNotification, object: clip)
        }
    }

    required init(coder: NSCoder) { fatalError("init(coder:) is not used") }

    @objc func textChanged() { lineStartsValid = false; needsDisplay = true }
    @objc func redraw() { needsDisplay = true }
    func invalidate() { lineStartsValid = false; needsDisplay = true }

    override var isOpaque: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        drawHashMarksAndLabels(in: dirtyRect)   // skip the default opaque background
    }

    private func rebuildLineStarts(_ ns: NSString) {
        var starts = [0]
        let len = ns.length
        var buf = [unichar](repeating: 0, count: len)
        ns.getCharacters(&buf, range: NSRange(location: 0, length: len))
        for i in 0..<len where buf[i] == 10 { starts.append(i + 1) }
        lineStarts = starts
        lineStartsValid = true
        let digits = max(3, String(starts.count).count)
        let font = labelFont
        let width = ("8" as NSString).size(withAttributes: [.font: font]).width * CGFloat(digits) + 18
        if abs(ruleThickness - width) > 0.5 { ruleThickness = width }
    }

    private var labelFont: NSFont {
        .monospacedDigitSystemFont(ofSize: max(9, (textView?.font?.pointSize ?? 13) - 2), weight: .regular)
    }

    private func lineIndex(for location: Int) -> Int {
        var lo = 0, hi = lineStarts.count - 1
        while lo < hi {
            let mid = (lo + hi + 1) / 2
            if lineStarts[mid] <= location { lo = mid } else { hi = mid - 1 }
        }
        return lo
    }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let tv = textView, let lm = tv.layoutManager, let tc = tv.textContainer else { return }
        let ns = tv.string as NSString
        if !lineStartsValid { rebuildLineStarts(ns) }
        let font = labelFont
        let dim: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.tertiaryLabelColor]
        let bright: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.secondaryLabelColor]
        let cursorLine = lineIndex(for: min(tv.selectedRange().location, ns.length))
        let originY = convert(NSPoint.zero, from: tv).y + tv.textContainerInset.height

        func label(_ index: Int, _ fragment: NSRect) {
            let s = "\(index + 1)" as NSString
            let attrs = index == cursorLine ? bright : dim
            let size = s.size(withAttributes: attrs)
            let y = originY + fragment.minY + (fragment.height - size.height) / 2
            s.draw(at: NSPoint(x: ruleThickness - size.width - 8, y: y), withAttributes: attrs)
        }

        if ns.length == 0 {
            label(0, lm.extraLineFragmentRect)
            return
        }
        let glyphs = lm.glyphRange(forBoundingRect: tv.visibleRect, in: tc)
        let chars = lm.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        var line = lineIndex(for: chars.location)
        while line < lineStarts.count {
            let start = lineStarts[line]
            if start > NSMaxRange(chars) || start >= ns.length { break }
            let glyph = lm.glyphIndexForCharacter(at: start)
            label(line, lm.lineFragmentRect(forGlyphAt: glyph, effectiveRange: nil))
            line += 1
        }
        if ns.hasSuffix("\n"), lm.extraLineFragmentTextContainer != nil, line == lineStarts.count - 1 {
            label(lineStarts.count - 1, lm.extraLineFragmentRect)
        }
    }
}
