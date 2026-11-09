import AppKit

/// One open file. Owns its own text view so each tab keeps its undo history, scroll position and selection.
@MainActor
final class EditorDocument: NSObject, ObservableObject, NSTextViewDelegate {
    let url: URL
    let language: Language
    let indentUnit: String
    @Published private(set) var isDirty = false
    @Published private(set) var line = 1
    @Published private(set) var column = 1
    @Published private(set) var isPreviewing = false
    @Published var isVisual = false
    private(set) var diskDate: Date?
    private var initialText: String
    private var scroll: NSScrollView?
    private var visualsController: MarkdownVisualsController?
    private var shownVisuals: [MarkdownVisual] = []
    private var expandedVisuals: Set<String> = []
    private var fontSize: CGFloat

    static let maxBytes = 8_000_000
    static let maxHighlightLength = 400_000

    /// Returns nil for binary or oversized files (those open in their default app instead).
    init?(url: URL, fontSize: CGFloat) {
        guard let data = try? Data(contentsOf: url), data.count <= Self.maxBytes else { return nil }
        let text: String
        if let utf8 = String(data: data, encoding: .utf8) { text = utf8 }
        else if !data.contains(0), let latin = String(data: data, encoding: .isoLatin1) { text = latin }
        else { return nil }
        if text.contains("\u{0}") { return nil }
        self.url = url
        self.language = Language.detect(url)
        self.initialText = text
        self.fontSize = fontSize
        let tabs = text.components(separatedBy: "\n\t").count
        let spaces2 = text.components(separatedBy: "\n  ").count - text.components(separatedBy: "\n    ").count
        if tabs > 3 && tabs > text.components(separatedBy: "\n    ").count { indentUnit = "\t" }
        else if spaces2 > 10 { indentUnit = "  " }
        else { indentUnit = "    " }
        self.diskDate = Self.modificationDate(url)
        super.init()
    }

    var name: String { url.lastPathComponent }
    var isMarkdown: Bool { language.kind == .markdown }
    var isWebPreviewable: Bool { ["html", "htm", "svg"].contains(url.pathExtension.lowercased()) }
    var isHTML: Bool { ["html", "htm"].contains(url.pathExtension.lowercased()) }

    /// Applies precise text edits (from the visual HTML editor or the MCP tools) as one undoable change.
    func applyEdits(_ edits: [(NSRange, String)]) {
        let tv = textView
        tv.undoManager?.beginUndoGrouping()
        for (range, replacement) in edits.sorted(by: { $0.0.location > $1.0.location }) {
            guard NSMaxRange(range) <= (tv.string as NSString).length else { continue }
            if tv.shouldChangeText(in: range, replacementString: replacement) {
                tv.textStorage?.replaceCharacters(in: range, with: replacement)
                tv.didChangeText()
            }
        }
        tv.undoManager?.endUndoGrouping()
    }
    var isLoaded: Bool { scroll != nil }

    var scrollView: NSScrollView {
        if let s = scroll { return s }
        let s = makeEditor()
        scroll = s
        return s
    }

    var textView: NSTextView { scrollView.documentView as! NSTextView }

    var text: String { isLoaded ? textView.string : initialText }

    private var font: NSFont { .monospacedSystemFont(ofSize: fontSize, weight: .regular) }

    private func makeEditor() -> NSScrollView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.borderType = .noBorder

        let storage = NSTextStorage()
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let container = NSTextContainer(containerSize: NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = false
        layout.addTextContainer(container)

        let tv = EditorTextView(frame: .zero, textContainer: container)
        tv.minSize = .zero
        tv.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        tv.isVerticallyResizable = true
        tv.isHorizontallyResizable = true
        tv.autoresizingMask = [.width]
        tv.textContainerInset = NSSize(width: 6, height: 10)
        tv.drawsBackground = false
        tv.isRichText = false
        tv.importsGraphics = false
        tv.allowsUndo = true
        tv.isAutomaticQuoteSubstitutionEnabled = false
        tv.isAutomaticDashSubstitutionEnabled = false
        tv.isAutomaticTextReplacementEnabled = false
        tv.isAutomaticSpellingCorrectionEnabled = false
        tv.isContinuousSpellCheckingEnabled = false
        tv.isGrammarCheckingEnabled = false
        tv.smartInsertDeleteEnabled = false
        tv.usesFindBar = true
        tv.isIncrementalSearchingEnabled = true
        tv.insertionPointColor = .controlAccentColor
        tv.string = initialText
        applyBaseAttributes(tv)
        tv.delegate = self

        scroll.documentView = tv
        let ruler = LineNumberRuler(textView: tv)
        scroll.verticalRulerView = ruler
        scroll.hasVerticalRuler = true
        scroll.rulersVisible = true
        scroll.postsFrameChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(editorResized), name: NSView.frameDidChangeNotification, object: scroll)
        highlightNow(tv)
        return scroll
    }

    private func applyBaseAttributes(_ tv: NSTextView) {
        let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: NSColor.labelColor]
        tv.font = font
        tv.typingAttributes = attrs
        if let storage = tv.textStorage, storage.length > 0 {
            storage.setAttributes(attrs, range: NSRange(location: 0, length: storage.length))
        }
    }

    func setFontSize(_ size: CGFloat) {
        fontSize = size
        guard isLoaded else { return }
        if isPreviewing { styleMarkdownNow(); return }
        applyBaseAttributes(textView)
        highlightNow(textView)
        (scrollView.verticalRulerView as? LineNumberRuler)?.invalidate()
    }

    // MARK: Highlighting

    private func scheduleHighlight() {
        NSObject.cancelPreviousPerformRequests(withTarget: self, selector: #selector(highlightTick), object: nil)
        perform(#selector(highlightTick), with: nil, afterDelay: 0.15)
    }

    @objc private func highlightTick() {
        guard isLoaded else { return }
        if isPreviewing { styleMarkdownNow() } else { highlightNow(textView) }
    }

    // MARK: Markdown live preview

    /// Switches between the plain-text source view and the styled, still-editable preview.
    func setPreview(_ on: Bool) {
        guard isMarkdown, on != isPreviewing else { return }
        isPreviewing = on
        let tv = textView
        let scroll = scrollView
        let selection = tv.selectedRange()
        if on {
            tv.layoutManager?.removeTemporaryAttribute(.foregroundColor, forCharacterRange: NSRange(location: 0, length: (tv.string as NSString).length))
            tv.isHorizontallyResizable = false
            tv.textContainer?.widthTracksTextView = true
            scroll.hasHorizontalScroller = false
            scroll.rulersVisible = false
            // Source mode lets the text view grow wider than the window (no wrapping); snap it back to the
            // visible width so Preview wraps and lays out visuals correctly without needing a window resize.
            tv.setFrameSize(NSSize(width: scroll.contentSize.width, height: tv.frame.height))
            updatePreviewInsets()
            styleMarkdownNow()
            DispatchQueue.main.async { [weak self] in      // once the pane has its final size
                guard let self, self.isPreviewing else { return }
                self.updatePreviewInsets()
                self.styleMarkdownNow()
            }
        } else {
            visualsController?.removeAll()
            visualsController = nil
            tv.textContainer?.widthTracksTextView = false
            tv.textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
            tv.isHorizontallyResizable = true
            tv.textContainerInset = NSSize(width: 6, height: 10)
            scroll.hasHorizontalScroller = true
            scroll.rulersVisible = true
            applyBaseAttributes(tv)
            highlightNow(tv)
            (scroll.verticalRulerView as? LineNumberRuler)?.invalidate()
        }
        tv.setSelectedRange(selection)
        tv.scrollRangeToVisible(selection)
    }

    /// Keeps preview text at a comfortable reading width, centered in the pane.
    private func updatePreviewInsets() {
        guard isPreviewing, let s = scroll, let tv = s.documentView as? NSTextView else { return }
        let width = s.contentSize.width
        guard width > 40 else { return }                 // not laid out yet; editorResized will call again
        if abs(tv.frame.width - width) > 0.5 { tv.setFrameSize(NSSize(width: width, height: tv.frame.height)) }
        let side = max(28, (width - 780) / 2)
        tv.textContainerInset = NSSize(width: side, height: 30)
        tv.textContainer?.containerSize = NSSize(width: max(200, width - side * 2), height: CGFloat.greatestFiniteMagnitude)
    }

    @objc private func editorResized() {
        updatePreviewInsets()
        if isPreviewing { scheduleHighlight() }          // visuals re-flow to the new width
    }

    private func styleMarkdownNow() {
        guard isLoaded, let storage = textView.textStorage else { return }
        guard storage.length <= Self.maxHighlightLength else { return }
        let visuals = MarkdownVisual.find(in: storage.string)
        let ctrl: MarkdownVisualsController
        if let existing = visualsController {
            ctrl = existing
        } else {
            ctrl = MarkdownVisualsController(textView: textView, baseURL: url.deletingLastPathComponent())
            ctrl.onSizeChange = { [weak self] in self?.styleMarkdownNow() }
            visualsController = ctrl
        }
        let expanded = ctrl.expanded(visuals, caret: textView.selectedRange())
        shownVisuals = visuals
        expandedVisuals = expanded
        ctrl.sync(visuals, expanded: expanded)
        ctrl.updateReserved()
        MarkdownStyler.style(storage, baseSize: fontSize, visuals: visuals, expanded: expanded, reserved: ctrl.reserved)
        textView.typingAttributes = MarkdownStyler.baseAttributes(fontSize)
        ctrl.layout()
        textView.needsDisplay = true                    // code-block panels span many lines
    }

    func textView(_ textView: NSTextView, clickedOnLink link: Any, at charIndex: Int) -> Bool {
        let raw = (link as? URL)?.absoluteString ?? (link as? String) ?? ""
        if let url = URL(string: raw), url.scheme != nil {
            if url.isFileURL { EditorRouter.open(url) } else { NSWorkspace.shared.open(url) }
            return true
        }
        // relative link → another file in the project
        let path = raw.components(separatedBy: "#").first ?? raw
        let target = url.deletingLastPathComponent().appendingPathComponent(path).standardizedFileURL
        if FileManager.default.fileExists(atPath: target.path) { EditorRouter.open(target) }
        return true
    }

    private func highlightNow(_ tv: NSTextView) {
        guard let lm = tv.layoutManager else { return }
        let text = tv.string
        let full = NSRange(location: 0, length: (text as NSString).length)
        lm.removeTemporaryAttribute(.foregroundColor, forCharacterRange: full)
        guard full.length <= Self.maxHighlightLength else { return }
        for (range, color) in SyntaxHighlighter.tokens(in: text, language: language)
        where NSMaxRange(range) <= full.length {
            lm.addTemporaryAttribute(.foregroundColor, value: color, forCharacterRange: range)
        }
    }

    // MARK: NSTextViewDelegate

    func textDidChange(_ notification: Notification) {
        if !isDirty { isDirty = true }
        scheduleHighlight()   // re-styles the preview too
    }

    func textViewDidChangeSelection(_ notification: Notification) {
        guard isLoaded else { return }
        // moving the caret into (or out of) a diagram's source shows (or re-renders) it
        if isPreviewing, let ctrl = visualsController,
           ctrl.expanded(shownVisuals, caret: textView.selectedRange()) != expandedVisuals {
            scheduleHighlight()
        }
        let ns = textView.string as NSString
        let loc = min(textView.selectedRange().location, ns.length)
        guard ns.length < 3_000_000 else { return }
        var count = 1, lastNewline = -1
        var buf = [unichar](repeating: 0, count: loc)
        ns.getCharacters(&buf, range: NSRange(location: 0, length: loc))
        for (i, c) in buf.enumerated() where c == 10 { count += 1; lastNewline = i }
        if line != count { line = count }
        let col = loc - lastNewline
        if column != col { column = col }
    }

    func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        if selector == #selector(NSResponder.insertNewline(_:)) {
            let ns = textView.string as NSString
            let sel = textView.selectedRange()
            let lineRange = ns.lineRange(for: NSRange(location: sel.location, length: 0))
            let before = ns.substring(with: NSRange(location: lineRange.location, length: sel.location - lineRange.location))
            if isMarkdown, let continued = continueList(before, lineStart: lineRange.location, in: textView) { return continued }
            let indent = String(before.prefix { $0 == " " || $0 == "\t" })
            let trimmed = before.trimmingCharacters(in: .whitespaces)
            let extra = (!isMarkdown && (trimmed.last.map { "{[(:".contains($0) } ?? false)) ? indentUnit : ""
            textView.insertText("\n" + indent + extra, replacementRange: sel)
            return true
        }
        if selector == #selector(NSResponder.insertTab(_:)) && indentUnit != "\t" {
            textView.insertText(indentUnit, replacementRange: textView.selectedRange())
            return true
        }
        return false
    }

    /// Return in a Markdown list continues it ("- ", "2. ", "- [ ] "); Return on an empty item ends the list.
    private func continueList(_ before: String, lineStart: Int, in tv: NSTextView) -> Bool? {
        let pattern = "^([ \\t]*)([-*+]|(\\d+)([.)]))([ \\t]+)(\\[[ xX]\\][ \\t]+)?"
        guard let re = try? NSRegularExpression(pattern: pattern),
              let m = re.firstMatch(in: before, range: NSRange(location: 0, length: (before as NSString).length)) else { return nil }
        let ns = before as NSString
        if m.range.length == ns.length {                    // empty item → remove the marker, end the list
            tv.insertText("", replacementRange: NSRange(location: lineStart, length: ns.length))
            return true
        }
        let indent = ns.substring(with: m.range(at: 1))
        var marker = ns.substring(with: m.range(at: 2))
        if m.range(at: 3).location != NSNotFound, let n = Int(ns.substring(with: m.range(at: 3))) {
            marker = "\(n + 1)" + ns.substring(with: m.range(at: 4))
        }
        let gap = ns.substring(with: m.range(at: 5))
        let task = m.range(at: 6).location != NSNotFound ? "[ ] " : ""
        tv.insertText("\n" + indent + marker + gap + task, replacementRange: tv.selectedRange())
        return true
    }

    // MARK: Disk

    @discardableResult
    func save() -> Bool {
        guard isDirty || !FileManager.default.fileExists(atPath: url.path) else { return true }
        do {
            try text.write(to: url, atomically: true, encoding: .utf8)
            isDirty = false
            diskDate = Self.modificationDate(url)
            return true
        } catch {
            NSAlert(error: error).runModal()
            return false
        }
    }

    /// Picks up changes made outside the editor (another app, git, an AI CLI) when this tab has no unsaved edits.
    func reloadIfChangedOnDisk() {
        guard !isDirty, let date = Self.modificationDate(url), date != diskDate,
              let data = try? Data(contentsOf: url), let fresh = String(data: data, encoding: .utf8) else { return }
        diskDate = date
        initialText = fresh
        guard isLoaded else { return }
        let tv = textView
        let selection = tv.selectedRange()
        tv.string = fresh
        if isPreviewing { styleMarkdownNow() } else { applyBaseAttributes(tv) }
        tv.undoManager?.removeAllActions()
        tv.setSelectedRange(NSRange(location: min(selection.location, (fresh as NSString).length), length: 0))
        if !isPreviewing { highlightNow(tv) }
        (scrollView.verticalRulerView as? LineNumberRuler)?.invalidate()
        isDirty = false
    }

    static func modificationDate(_ url: URL) -> Date? {
        (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
    }
}
