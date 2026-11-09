import AppKit
import SwiftUI
import WebKit

/// One page in the viewer. Local files reload by themselves when an AI rewrites them.
@MainActor
final class ArtifactTab: NSObject, ObservableObject, Identifiable, WKNavigationDelegate, WKUIDelegate {
    nonisolated let id = UUID()
    let webView: WKWebView
    @Published private(set) var title: String
    @Published private(set) var url: URL
    @Published private(set) var isLoading = false
    @Published private(set) var canGoBack = false
    @Published private(set) var canGoForward = false
    weak var browser: ArtifactBrowserController?
    /// Shown as a tab in the editor (next to code) rather than in the viewer window.
    @Published var inEditor = false
    /// Opened by request (dgview, desktopglass://, show_artifact, the address bar): links in this tab stay in the
    /// viewer. Other tabs only show artifacts and hand everything else to the browser.
    var openWeb = false
    private var watchTimer: Timer?
    private var fileDate: Date?

    init(url: URL, title: String?) {
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")   // right-click → Inspect Element
        webView = WKWebView(frame: .zero, configuration: config)
        self.url = url
        self.title = title ?? (url.absoluteString == "about:blank" ? "New Tab" : url.lastPathComponent)
        super.init()
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.setValue(false, forKey: "drawsBackground")
        webView.underPageBackgroundColor = .clear
        webView.allowsBackForwardNavigationGestures = true
        webView.allowsMagnification = true
        webView.customUserAgent = ArtifactPolicy.safariUserAgent
        load(url)
    }

    func load(_ target: URL) {
        url = target
        if target.isFileURL, target.pathExtension.lowercased() == "md",
           let source = try? String(contentsOf: target, encoding: .utf8) {
            // WebKit shows .md files as a blank page; show the source as readable text instead.
            webView.loadHTMLString(Self.markdownSourcePage(source, title: target.lastPathComponent), baseURL: target)
            startWatching()
        } else if target.isFileURL {
            webView.loadFileURL(target, allowingReadAccessTo: target.deletingLastPathComponent())
            startWatching()
        } else {
            stopWatching()
            webView.load(URLRequest(url: target))
        }
    }

    static func markdownSourcePage(_ source: String, title: String) -> String {
        let escape = { (s: String) in
            s.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
                .replacingOccurrences(of: ">", with: "&gt;")
        }
        return """
        <!doctype html><html><head><meta charset="utf-8"><title>\(escape(title))</title>
        <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">
        <style>
          :root { color-scheme: light dark; }
          body { margin: 0; padding: 28px 32px; background: transparent; color: CanvasText; }
          pre { margin: 0 auto; max-width: 900px; white-space: pre-wrap; word-wrap: break-word;
                font: 13px/1.55 ui-monospace, SFMono-Regular, Menlo, monospace; }
        </style></head><body><pre>\(escape(source))</pre></body></html>
        """
    }

    var isBlank: Bool { url.absoluteString == "about:blank" }

    /// Goes to what was typed in the address bar (or asked for by an agent). False if it can't be shown.
    @discardableResult
    func navigate(_ target: URL) -> Bool {
        guard ArtifactPolicy.canOpenOnRequest(target) else { return false }
        openWeb = true
        load(target)
        return true
    }

    func reload() {
        if url.isFileURL { load(url) } else { webView.reload() }
    }

    // MARK: Scripting (used by the browser MCP tools)

    /// Runs JavaScript in the page and returns its result (strings, numbers, bools, arrays, dictionaries).
    func evaluate(_ js: String) async throws -> Any? {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Any?, Error>) in
            webView.evaluateJavaScript(js) { result, error in
                if let error { cont.resume(throwing: error) } else { cont.resume(returning: result) }
            }
        }
    }

    func waitUntilLoaded(timeout: TimeInterval = 15) async {
        let deadline = Date().addingTimeInterval(timeout)
        try? await Task.sleep(nanoseconds: 150_000_000)
        while (isLoading || webView.isLoading) && Date() < deadline {
            try? await Task.sleep(nanoseconds: 150_000_000)
        }
    }

    func snapshotPNG(maxWidth: CGFloat = 1400) async -> Data? {
        let config = WKSnapshotConfiguration()
        config.snapshotWidth = NSNumber(value: Double(min(maxWidth, max(320, webView.bounds.width))))
        let image: NSImage? = await withCheckedContinuation { cont in
            webView.takeSnapshot(with: config) { image, _ in cont.resume(returning: image) }
        }
        guard let tiff = image?.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) else { return nil }
        return rep.representation(using: .png, properties: [:])
    }

    var info: [String: Any] {
        ["id": id.uuidString, "title": title, "url": url.isFileURL ? url.path : url.absoluteString,
         "loading": isLoading, "isFile": url.isFileURL, "shownIn": inEditor ? "editor" : "window"]
    }
    func goBack() { webView.goBack() }
    func goForward() { webView.goForward() }

    func stop() {
        stopWatching()
        webView.stopLoading()
    }

    // MARK: Live reload for files

    private func startWatching() {
        fileDate = EditorDocument.modificationDate(url)
        if watchTimer == nil {
            watchTimer = Timer.scheduledTimer(timeInterval: 1.0, target: self, selector: #selector(checkFile),
                                              userInfo: nil, repeats: true)
        }
    }

    private func stopWatching() {
        watchTimer?.invalidate()
        watchTimer = nil
    }

    @objc private func checkFile() {
        guard url.isFileURL, let date = EditorDocument.modificationDate(url), date != fileDate else { return }
        fileDate = date
        webView.reload()
    }

    // MARK: Navigation policy

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        guard let target = navigationAction.request.url else { return .allow }
        // Frames inside an artifact (sandboxed iframes, embeds) load whatever they need.
        if navigationAction.targetFrame?.isMainFrame == false { return .allow }
        if ArtifactPolicy.isAllowed(target) { return .allow }
        if openWeb && ArtifactPolicy.isWebPage(target) { return .allow }      // a page you asked for: its links stay here
        // An artifact that needs you signed in: sign in here, so the session stays in the viewer and it comes back.
        if ArtifactPolicy.isSignIn(target) { return .allow }
        if ArtifactPolicy.canHandOff(target) { NSWorkspace.shared.open(target) }   // not an artifact: the default browser
        return .cancel
    }

    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        let target = navigationAction.request.url
        // Sign-in popups ("Continue with Google") must be real popups sharing this page's configuration — that's how
        // the result gets back to the page. Loading them in the tab instead makes Google answer "400 malformed".
        if target == nil || target?.absoluteString == "about:blank" || target.map(ArtifactPolicy.isSignIn) == true {
            return SignInPopups.shared.open(configuration: configuration, features: windowFeatures)
        }
        if let target {
            if ArtifactPolicy.isAllowed(target) || (openWeb && ArtifactPolicy.isWebPage(target)) {
                browser?.open(target, title: nil, placement: inEditor ? .editor : .window, explicit: openWeb)
            }
            else if ArtifactPolicy.canHandOff(target) { NSWorkspace.shared.open(target) }
        }
        return nil
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) { refreshState(loading: true) }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { refreshState(loading: false) }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { refreshState(loading: false) }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        refreshState(loading: false)
    }

    private func refreshState(loading: Bool) {
        isLoading = loading
        canGoBack = webView.canGoBack
        canGoForward = webView.canGoForward
        if let current = webView.url, !loading { url = current }
        if let t = webView.title, !t.isEmpty { title = t }
    }
}

@MainActor
final class ArtifactBrowserController: NSObject, ObservableObject, NSWindowDelegate {
    static let shared = ArtifactBrowserController()

    /// Where a page opens: the editor's tabs (next to the code) or the viewer window. `.auto` uses the editor
    /// whenever it's open.
    enum Placement { case auto, editor, window }

    /// Every page, in the editor or the window — the MCP browser tools see them all.
    @Published private(set) var tabs: [ArtifactTab] = []
    /// The page most recently shown anywhere (what the MCP tools act on by default).
    @Published var active: ArtifactTab?
    /// The page selected in the viewer window.
    @Published var windowActive: ArtifactTab?
    let window: NSWindow
    var onVisibilityChange: (() -> Void)?
    /// Set by the app: whether the editor is open, how to show a tab there, and what to do when one closes.
    var preferEditor: () -> Bool = { false }
    var showInEditor: (ArtifactTab) -> Void = { _ in }
    var onTabClosed: (ArtifactTab) -> Void = { _ in }
    /// Set by the app: opens a Markdown file as an editor document in Preview. False if it couldn't.
    var openMarkdownInEditor: (URL) -> Bool = { _ in false }
    /// The file the last `open` showed as an editor document instead of a page (nil if it opened a page).
    private(set) var lastDocument: URL?

    var windowTabs: [ArtifactTab] { tabs.filter { !$0.inEditor } }
    var editorTabs: [ArtifactTab] { tabs.filter(\.inEditor) }

    private override init() {
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1100, height: 760),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                          backing: .buffered, defer: false)
        super.init()
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.title = "Artifacts"
        window.isOpaque = false
        window.backgroundColor = .clear
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 520, height: 360)
        window.collectionBehavior = [.fullScreenPrimary]
        window.delegate = self
        window.setFrameAutosaveName("DesktopGlassArtifacts")

        let host = NSHostingView(rootView: ArtifactBrowserView(browser: self))
        host.translatesAutoresizingMaskIntoConstraints = false
        let blur = NSVisualEffectView()
        blur.material = .underWindowBackground
        blur.blendingMode = .behindWindow
        blur.state = .active
        blur.addSubview(host)
        NSLayoutConstraint.activate([
            host.leadingAnchor.constraint(equalTo: blur.leadingAnchor),
            host.trailingAnchor.constraint(equalTo: blur.trailingAnchor),
            host.topAnchor.constraint(equalTo: blur.topAnchor),
            host.bottomAnchor.constraint(equalTo: blur.bottomAnchor),
        ])
        window.contentView = blur
        // The viewer's allow-lists come from the engine; fetch them whenever it (re)starts.
        EngineClient.shared.on("engine/ready") { _ in Task { await ArtifactPolicy.load() } }
    }

    var isVisible: Bool { window.isVisible }
    var excludedPorts: Set<Int> = []

    func tab(_ id: String?) -> ArtifactTab? {
        guard let id, !id.isEmpty else { return active }
        return tabs.first { $0.id.uuidString == id || $0.id.uuidString.hasPrefix(id) }
    }

    /// Shows the viewer even with nothing open, so the local-server picker is one click away.
    func showWindow() {
        if !window.isVisible { window.center() }
        NSApp.setActivationPolicy(.regular)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        onVisibilityChange?()
    }

    /// Shows `url` (reusing its tab if it's already open, wherever that is). Returns false if it isn't an artifact.
    @discardableResult
    /// `explicit`: asked for by name (dgview, desktopglass://, show_artifact, the address bar) — any web page may open,
    /// and its links stay in the viewer.
    func open(_ url: URL, title: String?, placement: Placement = .auto, explicit: Bool = false) -> Bool {
        guard ArtifactPolicy.tables != nil else {
            // The engine's rules haven't arrived yet (start-up): open it once they do.
            ArtifactPolicy.whenLoaded { [weak self] in self?.open(url, title: title, placement: placement, explicit: explicit) }
            return true
        }
        guard ArtifactPolicy.isAllowed(url) || (explicit && ArtifactPolicy.isWebPage(url)) else {
            if ArtifactPolicy.canHandOff(url) { NSWorkspace.shared.open(url) }
            return false
        }
        let toEditor = placement == .editor || (placement == .auto && preferEditor())
        // WebKit can't render Markdown (it shows a blank page), so with the editor open a .md file opens there as a
        // document in Preview, and any blank web tab left over for it is closed.
        lastDocument = nil
        if url.isFileURL, url.pathExtension.lowercased() == "md", toEditor, openMarkdownInEditor(url) {
            if let stale = tabs.first(where: { $0.url.standardizedFileURL == url.standardizedFileURL }) { close(stale) }
            lastDocument = url
            return true
        }
        let tab: ArtifactTab
        if let existing = tabs.first(where: { $0.url.absoluteString == url.absoluteString }) {
            existing.reload()
            if placement != .auto { existing.inEditor = toEditor }
            if explicit { existing.openWeb = true }
            tab = existing
        } else {
            tab = ArtifactTab(url: url, title: title)
            tab.browser = self
            tab.inEditor = toEditor
            tab.openWeb = explicit
            tabs.append(tab)
        }
        focus(tab)
        return true
    }

    /// A blank tab with the address bar ready for typing.
    @discardableResult
    func newTab(placement: Placement = .auto) -> ArtifactTab {
        let tab = ArtifactTab(url: URL(string: "about:blank")!, title: "New Tab")
        tab.browser = self
        tab.inEditor = placement == .editor || (placement == .auto && preferEditor())
        tab.openWeb = true
        tabs.append(tab)
        focus(tab)
        return tab
    }

    /// Brings a tab forward where it lives.
    func focus(_ tab: ArtifactTab) {
        active = tab
        if tab.inEditor {
            showInEditor(tab)
            if windowActive === tab { windowActive = windowTabs.last }
            if windowTabs.isEmpty && window.isVisible { window.orderOut(nil); onVisibilityChange?() }
        } else {
            windowActive = tab
            showWindow()
        }
    }

    /// Moves a page between the editor's tabs and the viewer window.
    func move(_ tab: ArtifactTab, toEditor: Bool) {
        guard tab.inEditor != toEditor else { return focus(tab) }
        if tab.inEditor { onTabClosed(tab) }                // leaving the editor
        tab.inEditor = toEditor
        focus(tab)
    }

    func close(_ tab: ArtifactTab) {
        tab.stop()
        guard let i = tabs.firstIndex(where: { $0 === tab }) else { return }
        tabs.remove(at: i)
        onTabClosed(tab)
        if active === tab { active = tabs.last }
        if windowActive === tab { windowActive = windowTabs.last }
        if !tab.inEditor && windowTabs.isEmpty && window.isVisible { window.performClose(nil) }
    }

    /// Closing the window closes its pages; pages in the editor stay.
    func windowWillClose(_ notification: Notification) {
        for t in windowTabs {
            t.stop()
            onTabClosed(t)
        }
        tabs.removeAll { !$0.inEditor }
        windowActive = nil
        if let a = active, !tabs.contains(where: { $0 === a }) { active = tabs.last }
        DispatchQueue.main.async { self.onVisibilityChange?() }
    }
}

// MARK: - UI

struct ArtifactBrowserView: View {
    @ObservedObject var browser: ArtifactBrowserController

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Color.clear.frame(width: 70, height: 1)
                if let tab = browser.windowActive {
                    NavButtons(tab: tab)
                    AddressBar(tab: tab).frame(minWidth: 160, idealWidth: 300, maxWidth: 380)
                }
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 4) {
                        ForEach(browser.windowTabs) { tab in ArtifactTabChip(tab: tab, browser: browser) }
                    }
                    .padding(.vertical, 6)
                }
                Button { browser.newTab(placement: .window) } label: { Image(systemName: "plus").frame(width: 22, height: 22) }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("New tab")
                WindowDragArea().frame(minWidth: 8, maxWidth: .infinity, maxHeight: .infinity)
                if let tab = browser.windowActive {
                    TabActions(tab: tab)
                    Button { browser.move(tab, toEditor: true) } label: { Image(systemName: "rectangle.split.2x1") }
                        .buttonStyle(.plain).foregroundStyle(.secondary).help("Move this page into the editor, next to the code")
                }
                ServersButton(browser: browser)
            }
            .padding(.trailing, 12)
            .frame(height: 44)
            Divider().opacity(0.25)
            if let tab = browser.windowActive {
                WebPage(tab: tab, browser: browser).id(tab.id)
            } else {
                VStack(spacing: 18) {
                    VStack(spacing: 8) {
                        Image(systemName: "sparkles.rectangle.stack").font(.system(size: 34, weight: .light)).foregroundStyle(.tertiary)
                        Text("Artifacts from your AI tools show up here").foregroundStyle(.secondary)
                        Text("Use the show_artifact MCP tool, or run `dgview <file-or-url>` in any terminal")
                            .font(.system(size: 12)).foregroundStyle(.tertiary)
                    }
                    ServerList(browser: browser)
                        .frame(width: 460)
                        .glassCard(cornerRadius: 16)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .ignoresSafeArea()
    }
}

struct NavButtons: View {
    @ObservedObject var tab: ArtifactTab
    var body: some View {
        HStack(spacing: 2) {
            Button { tab.goBack() } label: { Image(systemName: "chevron.left").frame(width: 24, height: 22) }
                .disabled(!tab.canGoBack)
            Button { tab.goForward() } label: { Image(systemName: "chevron.right").frame(width: 24, height: 22) }
                .disabled(!tab.canGoForward)
            Button { tab.reload() } label: {
                Image(systemName: tab.isLoading ? "xmark" : "arrow.clockwise").frame(width: 24, height: 22)
            }
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 4)
        .glassPill()
    }
}

private struct ArtifactTabChip: View {
    @ObservedObject var tab: ArtifactTab
    @ObservedObject var browser: ArtifactBrowserController
    @State private var hover = false
    private var isActive: Bool { browser.windowActive === tab }

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: tab.url.isFileURL ? "doc.richtext" : (tab.url.host?.contains("localhost") == true ? "server.rack" : "globe"))
                .font(.system(size: 10.5)).foregroundStyle(.secondary)
            Text(tab.title).font(.system(size: 12, weight: isActive ? .semibold : .regular)).lineLimit(1).frame(maxWidth: 180)
            if tab.isLoading { ProgressView().controlSize(.mini) }
            Button { browser.close(tab) } label: { Image(systemName: "xmark").font(.system(size: 8.5, weight: .bold)) }
                .buttonStyle(.plain).foregroundStyle(.secondary).opacity(hover || isActive ? 1 : 0)
        }
        .padding(.horizontal, 10).padding(.vertical, 5)
        .background(Capsule().fill(Color.white.opacity(isActive ? 0.14 : (hover ? 0.06 : 0))))
        .contentShape(Capsule())
        .onTapGesture { browser.windowActive = tab; browser.active = tab }
        .onHover { hover = $0 }
        .help(tab.url.isFileURL ? tab.url.path : tab.url.absoluteString)
    }
}

/// The page, or for a blank tab a start page: type an address above, or pick a server running on this Mac.
@MainActor
struct WebPage: View {
    @ObservedObject var tab: ArtifactTab
    let browser: ArtifactBrowserController

    var body: some View {
        if tab.isBlank {
            VStack(spacing: 16) {
                VStack(spacing: 6) {
                    Image(systemName: "globe").font(.system(size: 30, weight: .light)).foregroundStyle(.tertiary)
                    Text("Type an address above — a URL, localhost:port, or a file path").foregroundStyle(.secondary)
                }
                ServerList(browser: browser, navigate: { tab.navigate($0) })
                    .frame(width: 440)
                    .glassCard(cornerRadius: 16)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            WebViewHost(webView: tab.webView)
        }
    }
}

/// Where the page is, editable: Return goes there (any web page, localhost:port, or a viewable file).
@MainActor
struct AddressBar: View {
    @ObservedObject var tab: ArtifactTab
    @State private var text = ""
    @State private var bad = false
    @FocusState private var focused: Bool

    var body: some View {
        TextField("Address, localhost:port or file", text: $text)
            .textFieldStyle(.plain)
            .font(.system(size: 12))
            .focused($focused)
            .onSubmit(go)
            .padding(.horizontal, 10).padding(.vertical, 4)
            .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(Color.primary.opacity(0.06)))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(bad ? Color.orange : Color.primary.opacity(focused ? 0.25 : 0.08)))
            .onAppear {
                text = display(tab.url)
                if tab.isBlank { DispatchQueue.main.async { focused = true } }
            }
            .onChange(of: tab.url) { u in if !focused { text = display(u) } }
            .onChange(of: focused) { f in if !f { text = display(tab.url); bad = false } }
            .help(bad ? "Can't open that here — web pages, localhost, and HTML, SVG, PDF, image, Markdown, text or JSON files open in the viewer" : "")
    }

    private func display(_ u: URL) -> String {
        u.absoluteString == "about:blank" ? "" : (u.isFileURL ? u.path : u.absoluteString)
    }

    private func go() {
        let typed = text
        Task {
            guard let url = await ArtifactPolicy.address(typed), tab.navigate(url) else {
                bad = true
                NSSound.beep()
                return
            }
            bad = false
            focused = false
        }
    }
}

struct TabActions: View {
    @ObservedObject var tab: ArtifactTab
    var body: some View {
        HStack(spacing: 10) {
            if tab.url.isFileURL {
                Button { EditorRouter.open(tab.url) } label: { Image(systemName: "pencil") }.help("Edit in DesktopGlass")
                Button { NSWorkspace.shared.activateFileViewerSelecting([tab.url]) } label: { Image(systemName: "folder") }
                    .help("Show in Finder")
            }
            if let account = ArtifactAccount.forPage(tab.url) {
                Button { ArtifactAccounts.shared.signIn(account) } label: { Image(systemName: "person.crop.circle") }
                    .help("Sign in to \(account.name) for the viewer — this page reloads signed in")
            }
            Button { NSWorkspace.shared.open(tab.url) } label: { Image(systemName: "safari") }.help("Open in your browser")
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(tab.url.isFileURL ? tab.url.path : tab.url.absoluteString, forType: .string)
            } label: { Image(systemName: "link") }.help("Copy link")
        }
        .buttonStyle(.plain)
        .foregroundStyle(.secondary)
    }
}

@MainActor
struct WebViewHost: NSViewRepresentable {
    let webView: WKWebView

    func makeNSView(context: Context) -> NSView {
        let container = NSView()
        attach(container)
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        if container.subviews.first !== webView { attach(container) }
    }

    @MainActor
    private func attach(_ container: NSView) {
        container.subviews.forEach { $0.removeFromSuperview() }
        webView.frame = container.bounds
        webView.autoresizingMask = [.width, .height]
        container.addSubview(webView)
    }
}


// MARK: - Local web servers

@MainActor
final class LocalServerModel: ObservableObject {
    static let shared = LocalServerModel()
    @Published private(set) var servers: [LocalServer] = []
    @Published private(set) var scanning = false
    @Published private(set) var lastScan: Date?

    func scan(excluding ports: Set<Int>) {
        guard !scanning else { return }
        scanning = true
        Task {
            let found = (try? await EngineClient.shared.call("servers/list", ["exclude": Array(ports)])) as? [[String: Any]] ?? []
            servers = found.compactMap(LocalServer.init(json:))
            scanning = false
            lastScan = Date()
        }
    }
}

/// A web server running on this Mac (dev server, preview, notebook…), as found by the engine (`servers/list`:
/// `lsof`, then an HTTP probe of each port).
struct LocalServer: Identifiable, Hashable {
    let port: Int
    let process: String
    let pid: Int
    var title: String
    var status: Int
    var id: Int { port }
    var url: URL { URL(string: "http://localhost:\(port)/")! }

    init?(json d: [String: Any]) {
        guard let port = d["port"] as? Int else { return nil }
        self.port = port
        process = d["process"] as? String ?? ""
        pid = d["pid"] as? Int ?? 0
        title = d["title"] as? String ?? ""
        status = d["status"] as? Int ?? 0
    }
}

struct ServerList: View {
    @ObservedObject var browser: ArtifactBrowserController
    var placement: ArtifactBrowserController.Placement = .auto
    @ObservedObject private var model = LocalServerModel.shared
    var onPick: (() -> Void)? = nil
    /// When set, picking a server opens it here (a blank tab) instead of in a new tab.
    var navigate: ((URL) -> Void)? = nil

    init(browser: ArtifactBrowserController, placement: ArtifactBrowserController.Placement = .auto,
         navigate: ((URL) -> Void)? = nil, onPick: (() -> Void)? = nil) {
        self.browser = browser
        self.placement = placement
        self.navigate = navigate
        self.onPick = onPick
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Label("Running on this Mac", systemImage: "server.rack").font(.system(size: 12, weight: .semibold))
                Spacer()
                if model.scanning { ProgressView().controlSize(.small) }
                Button { model.scan(excluding: browser.excludedPorts) } label: { Image(systemName: "arrow.clockwise") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Scan again")
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            Divider().opacity(0.25)
            if model.servers.isEmpty {
                Text(model.scanning ? "Looking for web servers…" : "No web servers running. Start one (npm run dev, python -m http.server…) and scan again.")
                    .font(.system(size: 12)).foregroundStyle(.secondary)
                    .padding(14)
            } else {
                ScrollView {
                    VStack(spacing: 2) {
                        ForEach(model.servers) { server in
                            ServerRow(server: server) {
                                if let navigate { navigate(server.url) }
                                else { browser.open(server.url, title: server.title, placement: placement) }
                                onPick?()
                            }
                        }
                    }
                    .padding(6)
                }
                .frame(maxHeight: 320)
            }
        }
        .onAppear { model.scan(excluding: browser.excludedPorts) }
    }
}

private struct ServerRow: View {
    let server: LocalServer
    let open: () -> Void
    @State private var hover = false

    var body: some View {
        HStack(spacing: 10) {
            Text(":\(server.port)").font(.system(size: 13, weight: .semibold, design: .monospaced)).frame(width: 64, alignment: .leading)
            VStack(alignment: .leading, spacing: 1) {
                Text(server.title).font(.system(size: 12.5)).lineLimit(1)
                Text("\(server.process) · pid \(server.pid)\(server.status >= 400 ? " · HTTP \(server.status)" : "")")
                    .font(.system(size: 10.5)).foregroundStyle(.tertiary).lineLimit(1)
            }
            Spacer()
            Image(systemName: "arrow.up.right").font(.system(size: 10, weight: .semibold)).foregroundStyle(.secondary).opacity(hover ? 1 : 0)
        }
        .padding(.horizontal, 10).padding(.vertical, 7)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color.white.opacity(hover ? 0.08 : 0)))
        .contentShape(Rectangle())
        .onTapGesture(perform: open)
        .onHover { hover = $0 }
    }
}

struct ServersButton: View {
    @ObservedObject var browser: ArtifactBrowserController
    @State private var showing = false

    var body: some View {
        Button { showing.toggle() } label: {
            Image(systemName: "server.rack").frame(width: 26, height: 22)
        }
        .buttonStyle(.plain).glassPill()
        .help("Open a web server running on this Mac")
        .popover(isPresented: $showing, arrowEdge: .bottom) {
            ServerList(browser: browser) { showing = false }.frame(width: 420)
        }
    }
}
