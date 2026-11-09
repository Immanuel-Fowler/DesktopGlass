import AppKit
import SwiftUI
import WebKit

/// Sites whose generated artifacts need you signed in. Signing in here (Settings → Accounts) puts the session in
/// the viewer's own cookie store — the same one every viewer tab uses — so their artifacts open signed in.
/// DesktopGlass never reads or stores passwords or tokens: the site's own page does the sign-in, and the status
/// below only checks whether a session cookie of that name exists.
struct ArtifactAccount: Identifiable, Hashable {
    let id: String
    let name: String
    let detail: String
    let loginURL: URL
    let domains: [String]
    /// Cookie names that mean "signed in"; empty → any cookie whose name mentions session/auth/token.
    let sessionCookies: [String]

    static let all: [ArtifactAccount] = [
        ArtifactAccount(id: "claude", name: "Claude", detail: "Claude artifacts and shared chats",
                        loginURL: URL(string: "https://claude.ai/login")!, domains: ["claude.ai"], sessionCookies: ["sessionKey"]),
        ArtifactAccount(id: "chatgpt", name: "ChatGPT / Codex", detail: "ChatGPT canvases and Codex shares",
                        loginURL: URL(string: "https://chatgpt.com/auth/login")!, domains: ["chatgpt.com", "openai.com"],
                        sessionCookies: ["__Secure-next-auth.session-token"]),
        ArtifactAccount(id: "google", name: "Google", detail: "Gemini canvases and AI Studio apps",
                        loginURL: URL(string: "https://accounts.google.com/ServiceLogin?continue=https%3A%2F%2Fgemini.google.com%2Fapp")!,
                        domains: ["google.com"], sessionCookies: ["SID", "__Secure-1PSID"]),
        ArtifactAccount(id: "vercel", name: "Vercel", detail: "v0 and protected Vercel previews",
                        loginURL: URL(string: "https://vercel.com/login")!, domains: ["vercel.com", "v0.app", "v0.dev"], sessionCookies: []),
        ArtifactAccount(id: "github", name: "GitHub", detail: "Private GitHub Pages and Codespaces previews",
                        loginURL: URL(string: "https://github.com/login")!, domains: ["github.com"], sessionCookies: ["user_session"]),
    ]

    func owns(host: String?) -> Bool {
        guard let h = host?.lowercased() else { return false }
        return domains.contains { h == $0 || h.hasSuffix("." + $0) }
    }

    /// The account a page belongs to, if it's one of these sites.
    static func forPage(_ url: URL) -> ArtifactAccount? { all.first { $0.owns(host: url.host) } }

    func owns(_ cookie: HTTPCookie) -> Bool {
        let host = cookie.domain.lowercased().trimmingCharacters(in: CharacterSet(charactersIn: "."))
        return domains.contains { host == $0 || host.hasSuffix("." + $0) }
    }

    func isSession(_ cookie: HTTPCookie) -> Bool {
        guard owns(cookie), !cookie.value.isEmpty, (cookie.expiresDate ?? .distantFuture) > Date() else { return false }
        if !sessionCookies.isEmpty { return sessionCookies.contains(cookie.name) }
        let n = cookie.name.lowercased()
        return n.contains("session") || n.contains("auth") || n.contains("token")
    }
}

@MainActor
final class ArtifactAccounts: ObservableObject {
    static let shared = ArtifactAccounts()
    @Published private(set) var signedIn: Set<String> = []
    private var window: SignInWindow?


    func refresh() {
        Task { @MainActor in
            let cookies = await WKWebsiteDataStore.default().httpCookieStore.allCookies()
            signedIn = Set(ArtifactAccount.all.filter { a in cookies.contains(where: a.isSession) }.map(\.id))
        }
    }

    func signIn(_ account: ArtifactAccount) {
        window?.close()
        let w = SignInWindow(account: account) { [weak self] in
            self?.window = nil
            self?.refresh()
            // pages from that site pick up the new session
            for tab in ArtifactBrowserController.shared.tabs where account.owns(host: tab.url.host) { tab.reload() }
        }
        window = w
        w.show()
    }

    /// Removes that site's cookies and stored data from the viewer (it doesn't sign you out anywhere else).
    func signOut(_ account: ArtifactAccount) {
        Task { @MainActor in
            let store = WKWebsiteDataStore.default()
            for c in await store.httpCookieStore.allCookies() where account.owns(c) {
                await store.httpCookieStore.deleteCookie(c)
            }
            let types = WKWebsiteDataStore.allWebsiteDataTypes()
            let records = await store.dataRecords(ofTypes: types)
            let mine = records.filter { r in account.domains.contains { r.displayName == $0 || r.displayName.hasSuffix("." + $0) } }
            await store.removeData(ofTypes: types, for: mine)
            refresh()
        }
    }
}

/// A small browser window for signing in to one account. Uses the viewer's cookie store, supports the site's
/// sign-in popups, and says when you're signed in.
@MainActor
final class SignInWindow: NSObject, NSWindowDelegate, WKNavigationDelegate, WKUIDelegate {
    private let account: ArtifactAccount
    private let window: NSWindow
    private let webView: WKWebView
    private let onClose: () -> Void
    private let state = SignInState()

    init(account: ArtifactAccount, onClose: @escaping () -> Void) {
        self.account = account
        self.onClose = onClose
        webView = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        webView.customUserAgent = ArtifactPolicy.safariUserAgent
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 520, height: 720),
                          styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        super.init()
        webView.navigationDelegate = self
        webView.uiDelegate = self
        window.title = "Sign in to \(account.name)"
        window.isReleasedWhenClosed = false
        window.delegate = self
        let header = NSHostingView(rootView: SignInHeader(account: account, state: state) { [weak self] in self?.close() })
        let container = NSView()
        for v in [header, webView] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            container.addSubview(v)
        }
        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: container.topAnchor),
            header.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            header.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            webView.topAnchor.constraint(equalTo: header.bottomAnchor),
            webView.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            webView.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])
        window.contentView = container
    }

    func show() {
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        webView.load(URLRequest(url: account.loginURL))
    }

    func close() { window.close() }

    func windowWillClose(_ notification: Notification) {
        webView.stopLoading()
        onClose()
    }

    private func checkSignedIn() {
        let account = self.account, state = self.state
        Task { @MainActor in
            let cookies = await WKWebsiteDataStore.default().httpCookieStore.allCookies()
            state.signedIn = cookies.contains(where: account.isSession)
            if state.signedIn { ArtifactAccounts.shared.refresh() }
        }
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { checkSignedIn() }

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        SignInPopups.policy(for: navigationAction)
    }

    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        SignInPopups.shared.open(configuration: configuration, features: windowFeatures)
    }
}

@MainActor
final class SignInState: ObservableObject {
    @Published var signedIn = false
}

private struct SignInHeader: View {
    let account: ArtifactAccount
    @ObservedObject var state: SignInState
    let done: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: state.signedIn ? "checkmark.circle.fill" : "person.crop.circle")
                .foregroundStyle(state.signedIn ? Color.green : Color.secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(state.signedIn ? "Signed in to \(account.name)" : "Sign in to \(account.name)")
                    .font(.system(size: 12.5, weight: .semibold))
                Text(state.signedIn ? "Its artifacts now open signed in. You can close this window."
                                    : "For the artifact viewer only. DesktopGlass never sees your password.")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
            }
            Spacer()
            Button(state.signedIn ? "Done" : "Cancel", action: done).keyboardShortcut(state.signedIn ? .defaultAction : .cancelAction)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
    }
}

/// Windows that sites open with window.open() to sign you in ("Continue with Google"). They must be real popups
/// sharing the opener's configuration — that's how the result gets back to the page — and they close themselves.
@MainActor
final class SignInPopups: NSObject, WKUIDelegate, WKNavigationDelegate, NSWindowDelegate {
    static let shared = SignInPopups()
    private var windows: [(window: NSWindow, webView: WKWebView)] = []

    func open(configuration: WKWebViewConfiguration, features: WKWindowFeatures) -> WKWebView {
        let size = NSSize(width: CGFloat(features.width?.doubleValue ?? 500), height: CGFloat(features.height?.doubleValue ?? 640))
        let web = WKWebView(frame: NSRect(origin: .zero, size: size), configuration: configuration)
        web.customUserAgent = ArtifactPolicy.safariUserAgent
        web.uiDelegate = self
        web.navigationDelegate = self
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.titled, .closable, .resizable],
                              backing: .buffered, defer: false)
        window.title = "Sign In"
        window.contentView = web
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        windows.append((window, web))
        return web
    }

    /// Sign-in pages may go anywhere secure; anything else (an app link, a file) is refused or handed to the browser.
    static func policy(for action: WKNavigationAction) -> WKNavigationActionPolicy {
        guard let url = action.request.url, let scheme = url.scheme?.lowercased() else { return .allow }
        if ["https", "about", "blob", "data"].contains(scheme) { return .allow }
        if scheme == "http", ["localhost", "127.0.0.1"].contains(url.host ?? "") { return .allow }
        if ArtifactPolicy.canHandOff(url) { NSWorkspace.shared.open(url) }
        return .cancel
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction) async -> WKNavigationActionPolicy {
        Self.policy(for: navigationAction)
    }

    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        open(configuration: configuration, features: windowFeatures)
    }

    /// The page called window.close() — sign-in finished.
    func webViewDidClose(_ webView: WKWebView) {
        windows.first { $0.webView === webView }?.window.close()
        ArtifactAccounts.shared.refresh()
    }

    func windowWillClose(_ notification: Notification) {
        guard let w = notification.object as? NSWindow else { return }
        windows.removeAll { $0.window === w }
    }
}

/// Settings → Accounts.
struct AccountsSection: View {
    @ObservedObject private var accounts = ArtifactAccounts.shared
    @AppStorage(CodexVisualizations.enabledKey) private var showCodexVisuals = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Sign in once so artifacts from these sites open right away in the viewer, already signed in. The sign-in stays in DesktopGlass's viewer; your passwords never pass through DesktopGlass.")
                .font(.system(size: 11.5)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                .padding(.bottom, 10)
            ForEach(ArtifactAccount.all) { a in
                HStack(spacing: 10) {
                    Circle().fill(accounts.signedIn.contains(a.id) ? Color.green : Color.secondary.opacity(0.35)).frame(width: 7, height: 7)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(a.name).font(.system(size: 12.5, weight: .medium))
                        Text(a.detail).font(.system(size: 11)).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if accounts.signedIn.contains(a.id) {
                        Button("Sign Out") { accounts.signOut(a) }
                            .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 5).glassPill()
                            .help("Removes \(a.name)'s session from the viewer only")
                    } else {
                        Button("Sign In") { accounts.signIn(a) }
                            .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 5).glassPill()
                    }
                }
                .padding(.vertical, 6)
            }
            Divider().opacity(0.3).padding(.vertical, 8)
            Toggle("Show Codex visualizations automatically", isOn: $showCodexVisuals)
                .toggleStyle(.checkbox).font(.system(size: 12))
            Text("Codex saves each visualization as a page; it can only draw them in the Codex app (elsewhere it says “Visualization unavailable on this device”). DesktopGlass opens new ones in the viewer — next to your code when the editor is open.")
                .font(.system(size: 11)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 2)
        }
        .onAppear { accounts.refresh() }
    }
}
