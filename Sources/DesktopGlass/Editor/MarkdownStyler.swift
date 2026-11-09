import AppKit

/// Live-preview styling for Markdown: the text stays fully editable, but headings are big, **bold** is bold,
/// `code` is monospaced, links are clickable and the Markdown symbols fade back. Applied to the text storage,
/// so it never touches the file's contents or the undo history.
enum MarkdownStyler {
    private static var cache: [String: NSRegularExpression] = [:]

    private static func regex(_ pattern: String) -> NSRegularExpression? {
        if let r = cache[pattern] { return r }
        let r = try? NSRegularExpression(pattern: pattern, options: [.anchorsMatchLines])
        cache[pattern] = r
        return r
    }

    static func bodyFont(_ base: CGFloat) -> NSFont { .systemFont(ofSize: base + 2) }

    static func bodyParagraph() -> NSParagraphStyle {
        let p = NSMutableParagraphStyle()
        p.lineSpacing = 3
        p.paragraphSpacing = 7
        return p
    }

    static func baseAttributes(_ base: CGFloat) -> [NSAttributedString.Key: Any] {
        [.font: bodyFont(base), .foregroundColor: NSColor.labelColor, .paragraphStyle: bodyParagraph()]
    }

    /// `visuals`: diagrams/images found in the text; `expanded`: ones whose source is being edited (shown as text);
    /// `reserved`: height to leave under each paragraph (by paragraph start) for the rendered visuals.
    static func style(_ storage: NSTextStorage, baseSize base: CGFloat, visuals: [MarkdownVisual] = [],
                      expanded: Set<String> = [], reserved: [Int: CGFloat] = [:]) {
        let text = storage.string
        let full = NSRange(location: 0, length: (text as NSString).length)
        let dim: [NSAttributedString.Key: Any] = [.foregroundColor: NSColor.tertiaryLabelColor]
        let mono = NSFont.monospacedSystemFont(ofSize: base, weight: .regular)
        let codeBackground = NSColor.gray.withAlphaComponent(0.22)
        let fm = NSFontManager.shared

        func each(_ pattern: String, _ body: (NSTextCheckingResult) -> Void) {
            regex(pattern)?.enumerateMatches(in: text, range: full) { m, _, _ in if let m { body(m) } }
        }
        func convert(_ range: NSRange, _ trait: NSFontTraitMask) {
            storage.enumerateAttribute(.font, in: range) { value, sub, _ in
                if let f = value as? NSFont { storage.addAttribute(.font, value: fm.convert(f, toHaveTrait: trait), range: sub) }
            }
        }
        func valid(_ r: NSRange) -> Bool { r.location != NSNotFound && r.length > 0 }

        storage.beginEditing()
        storage.setAttributes(baseAttributes(base), range: full)

        // Headings
        let sizes: [CGFloat] = [2.0, 1.6, 1.32, 1.16, 1.05, 1.0]
        each("^(#{1,6})([ \\t]+)(.*)$") { m in
            let level = m.range(at: 1).length
            let font = NSFont.systemFont(ofSize: (base + 2) * sizes[level - 1], weight: level <= 2 ? .bold : .semibold)
            storage.addAttribute(.font, value: font, range: m.range)
            let p = NSMutableParagraphStyle()
            p.paragraphSpacingBefore = level <= 2 ? 14 : 8
            p.paragraphSpacing = level <= 2 ? 8 : 5
            storage.addAttribute(.paragraphStyle, value: p, range: m.range)
            storage.addAttributes(dim, range: NSUnionRange(m.range(at: 1), m.range(at: 2)))
        }

        // Lists and task lists: accent marker, wrapped lines hang under the text
        each("^([ \\t]*)([-*+]|\\d+[.)])([ \\t]+)(\\[[ xX]\\][ \\t]+)?") { m in
            storage.addAttributes([.foregroundColor: NSColor.controlAccentColor,
                                   .font: NSFont.systemFont(ofSize: base + 2, weight: .semibold)], range: m.range(at: 2))
            if valid(m.range(at: 4)) { storage.addAttribute(.foregroundColor, value: NSColor.controlAccentColor, range: m.range(at: 4)) }
            let line = (text as NSString).lineRange(for: NSRange(location: m.range.location, length: 0))
            let p = NSMutableParagraphStyle()
            p.lineSpacing = 3
            p.paragraphSpacing = 3
            p.headIndent = CGFloat(m.range.length) * (base + 2) * 0.55
            storage.addAttribute(.paragraphStyle, value: p, range: line)
        }

        // Block quotes
        each("^>[ \\t]?.*$") { m in
            storage.addAttribute(.foregroundColor, value: NSColor.secondaryLabelColor, range: m.range)
            convert(m.range, .italicFontMask)
            storage.addAttributes(dim, range: NSRange(location: m.range.location, length: 1))
        }

        // Emphasis
        each("(\\*\\*|__)(?=\\S)(.+?)(?<=\\S)\\1") { m in
            convert(m.range(at: 2), .boldFontMask)
            storage.addAttributes(dim, range: m.range(at: 1))
            storage.addAttributes(dim, range: NSRange(location: NSMaxRange(m.range) - m.range(at: 1).length, length: m.range(at: 1).length))
        }
        each("(?<![*_\\w])([*_])(?=[^\\s*_])(.+?)(?<=[^\\s*_])\\1(?![*_\\w])") { m in
            convert(m.range(at: 2), .italicFontMask)
            storage.addAttributes(dim, range: m.range(at: 1))
            storage.addAttributes(dim, range: NSRange(location: NSMaxRange(m.range) - 1, length: 1))
        }
        each("~~(.+?)~~") { m in
            storage.addAttribute(.strikethroughStyle, value: NSUnderlineStyle.single.rawValue, range: m.range(at: 1))
            storage.addAttributes(dim, range: NSRange(location: m.range.location, length: 2))
            storage.addAttributes(dim, range: NSRange(location: NSMaxRange(m.range) - 2, length: 2))
        }

        // Links: [text](url) and bare URLs
        each("\\[([^\\]\\n]+)\\]\\(([^)\\s]+)(?:\\s+\"[^\"]*\")?\\)") { m in
            let target = (text as NSString).substring(with: m.range(at: 2))
            storage.addAttributes([.foregroundColor: NSColor.linkColor, .underlineStyle: NSUnderlineStyle.single.rawValue,
                                   .link: target], range: m.range(at: 1))
            storage.addAttributes(dim, range: NSRange(location: m.range.location, length: 1))
            let tail = NSRange(location: NSMaxRange(m.range(at: 1)), length: NSMaxRange(m.range) - NSMaxRange(m.range(at: 1)))
            storage.addAttributes(dim, range: tail)
            storage.addAttribute(.font, value: NSFont.systemFont(ofSize: base - 1), range: tail)
        }
        each("(?<![(\\[])\\bhttps?://[^\\s<>()\\]]+") { m in
            let target = (text as NSString).substring(with: m.range)
            storage.addAttributes([.foregroundColor: NSColor.linkColor, .link: target], range: m.range)
        }

        // Inline code, then fenced code blocks (which win over everything inside them)
        each("(`)([^`\\n]+)(`)") { m in
            storage.addAttributes([.font: mono, .backgroundColor: codeBackground], range: m.range(at: 2))
            storage.addAttributes(dim, range: m.range(at: 1))
            storage.addAttributes(dim, range: m.range(at: 3))
        }
        each("^```([^\\n]*)\\n(?:.*\\n)*?```[ \\t]*$") { m in
            // A dark glass panel with a hairline border is drawn behind `.codeBlock` ranges by EditorTextView.
            func para(before: CGFloat = 0, after: CGFloat = 0) -> NSParagraphStyle {
                let p = NSMutableParagraphStyle()
                p.lineSpacing = 1.5
                p.headIndent = 16
                p.firstLineHeadIndent = 16
                p.tailIndent = -16
                p.paragraphSpacingBefore = before
                p.paragraphSpacing = after
                return p
            }
            let code = NSFont.monospacedSystemFont(ofSize: max(10, base - 1), weight: .regular)
            storage.setAttributes([.font: code, .foregroundColor: NSColor.labelColor, .paragraphStyle: para(),
                                   .codeBlock: true], range: m.range)
            let ns = text as NSString
            let first = ns.lineRange(for: NSRange(location: m.range.location, length: 0))
            let last = ns.lineRange(for: NSRange(location: NSMaxRange(m.range) - 1, length: 0))
            storage.addAttributes(dim, range: first)
            storage.addAttributes(dim, range: last)
            storage.addAttribute(.paragraphStyle, value: para(before: 10), range: first)
            storage.addAttribute(.paragraphStyle, value: para(after: 10), range: last)
            // Syntax highlighting with the editor's own highlighter, chosen by the fence's language.
            let bodyStart = NSMaxRange(first), bodyEnd = last.location
            if bodyEnd > bodyStart, let lang = fenceLanguage(ns.substring(with: m.range(at: 1))) {
                let body = NSRange(location: bodyStart, length: bodyEnd - bodyStart)
                for (r, color) in SyntaxHighlighter.tokens(in: ns.substring(with: body), language: lang) {
                    storage.addAttribute(.foregroundColor, value: color, range: NSRange(location: body.location + r.location, length: r.length))
                }
            }
        }

        // Tables and rules
        each("^\\|.*\\|[ \\t]*$") { m in storage.addAttribute(.font, value: mono, range: m.range) }
        each("^(-{3,}|\\*{3,}|_{3,})[ \\t]*$") { m in storage.addAttributes(dim, range: m.range) }

        // Visuals: hide the markup of standalone diagrams/images (unless it's being edited) …
        let ns = text as NSString
        let hidden: [NSAttributedString.Key: Any] = {
            let p = NSMutableParagraphStyle()
            p.minimumLineHeight = 0.1
            p.maximumLineHeight = 0.1
            return [.font: NSFont.systemFont(ofSize: 0.1), .foregroundColor: NSColor.clear, .paragraphStyle: p]
        }()
        for v in visuals where !expanded.contains("\(v.key)@\(v.range.location)") {
            if v.standalone {
                storage.setAttributes(hidden, range: ns.paragraphRange(for: v.range))
            } else {
                storage.addAttributes([.font: NSFont.systemFont(ofSize: 0.1), .foregroundColor: NSColor.clear], range: v.range)
            }
        }
        // … and leave room under each paragraph for its rendered visuals.
        for (start, height) in reserved where start < ns.length {
            guard let v = visuals.first(where: { $0.paragraph.location == start }) else { continue }
            let lastChar = max(v.paragraph.location, NSMaxRange(v.paragraph) - 1)
            let line = ns.paragraphRange(for: NSRange(location: min(lastChar, ns.length - 1), length: 0))
            let current = storage.attribute(.paragraphStyle, at: line.location, effectiveRange: nil) as? NSParagraphStyle
            let p = (current ?? bodyParagraph()).mutableCopy() as! NSMutableParagraphStyle
            p.paragraphSpacing += height
            storage.addAttribute(.paragraphStyle, value: p, range: line)
        }

        storage.endEditing()
    }

    /// ```` ```ts ````, ```` ```python title="x" ```` … → the editor's Language, or nil for plain text.
    static func fenceLanguage(_ info: String) -> Language? {
        guard let word = info.split(whereSeparator: { " \t{,".contains($0) }).first?.lowercased(), !word.isEmpty else { return nil }
        let aliases = ["javascript": "js", "node": "js", "typescript": "ts", "python": "py", "python3": "py", "py3": "py",
                       "bash": "sh", "shell": "sh", "console": "sh", "shellsession": "sh", "terminal": "sh",
                       "rust": "rs", "ruby": "rb", "golang": "go", "yml": "yaml", "objc": "m", "objective-c": "m",
                       "c++": "cpp", "csharp": "cs", "c#": "cs", "kotlin": "kt", "markdown": "md", "svg": "svg",
                       "dockerfile": "sh", "make": "sh", "makefile": "sh", "ini": "ini", "plist": "plist"]
        let lang = Language.detect(URL(fileURLWithPath: "/fence." + (aliases[word] ?? word)))
        return lang.kind == .plain ? nil : lang
    }
}

extension NSAttributedString.Key {
    /// Marks a fenced code block in Markdown Preview; EditorTextView draws its panel.
    static let codeBlock = NSAttributedString.Key("DesktopGlass.codeBlock")
}

/// The editor's text view. In Markdown Preview it draws a translucent dark panel with a hairline border behind each
/// fenced code block, so the glass still shows through.
final class EditorTextView: NSTextView {
    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        guard let storage = textStorage, storage.length > 0, let lm = layoutManager, let tc = textContainer else { return }
        let origin = textContainerOrigin
        let glyphs = lm.glyphRange(forBoundingRect: rect.offsetBy(dx: -origin.x, dy: -origin.y), in: tc)
        let visible = lm.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil)
        guard visible.length > 0 else { return }
        let full = NSRange(location: 0, length: storage.length)
        let dark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
        let fill = NSColor.black.withAlphaComponent(dark ? 0.30 : 0.06)
        let stroke = (dark ? NSColor.white : NSColor.black).withAlphaComponent(dark ? 0.14 : 0.12)
        var location = visible.location
        while location < NSMaxRange(visible) {
            var block = NSRange()
            let marked = storage.attribute(.codeBlock, at: location, longestEffectiveRange: &block, in: full) != nil
            if marked {
                let g = lm.glyphRange(forCharacterRange: block, actualCharacterRange: nil)
                var box = lm.boundingRect(forGlyphRange: g, in: tc)
                // the first line's spacing-before sits above the text; keep the panel tight to the code
                if let p = storage.attribute(.paragraphStyle, at: block.location, effectiveRange: nil) as? NSParagraphStyle {
                    box.origin.y += p.paragraphSpacingBefore
                    box.size.height -= p.paragraphSpacingBefore
                }
                if let p = storage.attribute(.paragraphStyle, at: NSMaxRange(block) - 1, effectiveRange: nil) as? NSParagraphStyle {
                    box.size.height -= p.paragraphSpacing
                }
                let panel = NSRect(x: origin.x + tc.lineFragmentPadding, y: origin.y + box.minY - 6,
                                   width: tc.containerSize.width - tc.lineFragmentPadding * 2, height: box.height + 12)
                if panel.height > 14 {
                    let path = NSBezierPath(roundedRect: panel.insetBy(dx: 0.5, dy: 0.5), xRadius: 9, yRadius: 9)
                    fill.setFill(); path.fill()
                    stroke.setStroke(); path.lineWidth = 1; path.stroke()
                }
            }
            location = NSMaxRange(block)
        }
    }
}
