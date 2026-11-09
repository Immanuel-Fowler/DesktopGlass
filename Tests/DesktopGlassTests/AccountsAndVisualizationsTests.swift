import XCTest
@testable import DesktopGlass

/// Which viewer cookies count as "signed in", and turning Codex visualization fragments into pages.
final class AccountsAndVisualizationsTests: XCTestCase {
    private func cookie(_ domain: String, _ name: String, value: String = "x", expires: Date? = nil) -> HTTPCookie {
        var p: [HTTPCookiePropertyKey: Any] = [.domain: domain, .path: "/", .name: name, .value: value]
        if let expires { p[.expires] = expires }
        return HTTPCookie(properties: p)!
    }

    private func account(_ id: String) -> ArtifactAccount { ArtifactAccount.all.first { $0.id == id }! }

    // MARK: Accounts

    func testSessionCookies() {
        let claude = account("claude")
        XCTAssertTrue(claude.isSession(cookie(".claude.ai", "sessionKey")))
        XCTAssertFalse(claude.isSession(cookie(".claude.ai", "anthropic-device-id")), "other cookies aren't a session")
        XCTAssertFalse(claude.isSession(cookie(".evilclaude.ai", "sessionKey")), "lookalike domain")
        XCTAssertFalse(claude.isSession(cookie(".claude.ai", "sessionKey", value: "")))
        XCTAssertFalse(claude.isSession(cookie(".claude.ai", "sessionKey", expires: Date(timeIntervalSinceNow: -60))), "expired")
    }

    func testAccountsWithoutNamedCookiesUseAGenericRule() {
        let vercel = account("vercel")
        XCTAssertTrue(vercel.isSession(cookie("vercel.com", "authorization")))
        XCTAssertTrue(vercel.isSession(cookie(".v0.app", "__Secure-session")))
        XCTAssertFalse(vercel.isSession(cookie("vercel.com", "theme")))
    }

    func testPagesMapToTheirAccount() {
        XCTAssertEqual(ArtifactAccount.forPage(URL(string: "https://claude.ai/artifact/abc")!)?.id, "claude")
        XCTAssertEqual(ArtifactAccount.forPage(URL(string: "https://chatgpt.com/canvas/shared/1")!)?.id, "chatgpt")
        XCTAssertEqual(ArtifactAccount.forPage(URL(string: "https://my-app.v0.app/")!)?.id, "vercel")
        XCTAssertNil(ArtifactAccount.forPage(URL(string: "http://localhost:5173/")!))
        XCTAssertNil(ArtifactAccount.forPage(URL(fileURLWithPath: "/tmp/a.html")))
    }

    // MARK: Codex visualizations

    func testFragmentsBecomePages() {
        let page = CodexVisualizations.page(for: "<div class=\"chart\">42</div>", title: "Sales <Q3>")
        XCTAssertTrue(page.hasPrefix("<!doctype html>"))
        XCTAssertTrue(page.contains("<div class=\"chart\">42</div>"))
        XCTAssertTrue(page.contains("<title>Sales &lt;Q3></title>"))
        let full = "<!DOCTYPE html><html><body>x</body></html>"
        XCTAssertEqual(CodexVisualizations.page(for: full, title: "x"), full, "full documents are left alone")
    }

    func testRecentFilesFindTodayAndYesterday() throws {
        let dir = try TempDir()
        let root = dir.root.appendingPathComponent("visualizations")
        let now = ISO8601DateFormatter().date(from: "2026-11-04T15:00:00Z")!
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = .current
        func day(_ d: Date) -> String {
            let c = cal.dateComponents([.year, .month, .day], from: d)
            return String(format: "%04d/%02d/%02d", c.year!, c.month!, c.day!)
        }
        try dir.file("visualizations/\(day(now))/thread-a/chart.html", "<div/>")
        try dir.file("visualizations/\(day(now))/thread-a/notes.txt", "x")
        try dir.file("visualizations/\(day(now.addingTimeInterval(-86_400)))/thread-b/board.html", "<div/>")
        try dir.file("visualizations/\(day(now.addingTimeInterval(-5 * 86_400)))/thread-c/old.html", "<div/>")
        let found = CodexVisualizations.recentFiles(in: [root, dir.root.appendingPathComponent("missing")], now: now)
        XCTAssertEqual(Set(found.map(\.lastPathComponent)), ["chart.html", "board.html"])
    }

    func testPagesAreWrittenOutsideCodexFolders() throws {
        let dir = try TempDir()
        let source = try dir.file("codex/visualizations/2026/11/04/thread-a/chart.html", "<b>hi</b>")
        let pages = dir.root.appendingPathComponent("pages")
        let out = try CodexVisualizations.writePage(for: source, in: pages)
        XCTAssertEqual(out.path, pages.appendingPathComponent("thread-a/chart.html").path)
        XCTAssertTrue(try String(contentsOf: out, encoding: .utf8).contains("<b>hi</b>"))
        XCTAssertEqual(try String(contentsOf: source, encoding: .utf8), "<b>hi</b>", "Codex's file is untouched")
    }
}
