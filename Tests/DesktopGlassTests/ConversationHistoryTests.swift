import XCTest
@testable import DesktopGlass

/// Past conversations: the resume command for each CLI, and reading each CLI's saved sessions for one folder.
/// Session files are written into a temporary home folder in each CLI's real layout.
final class ConversationHistoryTests: XCTestCase {
    private var home: TempDir!
    private var project: URL!

    override func setUpWithError() throws {
        home = try TempDir()
        project = try home.dir("Desktop/Projects/site")
    }

    private func jsonl(_ objects: [[String: Any]]) -> String {
        objects.map { String(data: try! JSONSerialization.data(withJSONObject: $0), encoding: .utf8)! }.joined(separator: "\n") + "\n"
    }

    // MARK: Resume commands

    func testResumeArguments() {
        XCTAssertEqual(Harness.claude.resumeArguments(.last), ["--continue"])
        XCTAssertEqual(Harness.claude.resumeArguments(.session("abc")), ["--resume", "abc"])
        XCTAssertEqual(Harness.codex.resumeArguments(.last), ["resume", "--last"])
        XCTAssertEqual(Harness.codex.resumeArguments(.picker), ["resume"])
        XCTAssertEqual(Harness.codex.resumeArguments(.session("abc")), ["resume", "abc"])
        XCTAssertEqual(Harness.gemini.resumeArguments(.session("abc")), ["--resume", "abc"])
        XCTAssertEqual(Harness.cursor.resumeArguments(.picker), ["ls"])
        XCTAssertEqual(Harness.copilot.resumeArguments(.picker), ["--resume"])
        XCTAssertEqual(Harness.opencode.resumeArguments(.session("ses_1")), ["--session", "ses_1"])
        XCTAssertNil(Harness.gemini.resumeArguments(.picker))
        XCTAssertNil(Harness.opencode.resumeArguments(.picker))
    }

    func testEveryAgentCLICanResumeAndLocalModelsCant() {
        for h in Harness.agents { XCTAssertTrue(h.canResume, h.label) }
        for h in [Harness.ollama, .mlx, .lmstudio, .apple, .llamacpp] { XCTAssertFalse(h.canResume, h.label) }
    }

    func testResumeCommandIsQuotedAndDropsTheModel() {
        XCTAssertEqual(Harness.codex.terminalCommand(binary: "/opt/bin/codex", model: "gpt-5.5", resume: .session("a'b")),
                       #"'/opt/bin/codex' 'resume' 'a'\''b'"#)
        XCTAssertEqual(Harness.claude.terminalCommand(binary: "/bin/claude", model: "opus", resume: .last), "'/bin/claude' '--continue'")
        XCTAssertNil(Harness.ollama.terminalCommand(binary: "/bin/ollama", resume: .last))
    }

    // MARK: Claude Code

    func testClaudeProjectFolderName() {
        XCTAssertEqual(ConversationHistory.claudeProjectName("/Users/me/My App_1.0"), "-Users-me-My-App-1-0")
    }

    func testClaudeSessions() throws {
        let dir = ".claude/projects/" + ConversationHistory.claudeProjectName(project.path)
        try home.file("\(dir)/s1.jsonl", jsonl([
            ["type": "user", "cwd": project.path, "message": ["role": "user", "content": "<command-name>/init</command-name>"]],
            ["type": "user", "cwd": project.path, "message": ["role": "user", "content": [["type": "text", "text": "Build the   chart\npage"]]]],
        ]))
        try home.file("\(dir)/s2.jsonl", jsonl([
            ["type": "summary", "summary": "Fix login bug"],
            ["type": "user", "cwd": project.path, "message": ["content": "the login is broken"]],
        ]))
        try home.file("\(dir)/s3.jsonl", jsonl([
            ["type": "user", "cwd": "/somewhere/else", "message": ["content": "not this folder"]],
        ]))
        try home.file("\(dir)/s4.jsonl", jsonl([
            ["type": "user", "cwd": project.path, "isMeta": true, "message": ["content": "meta"]],
            ["type": "custom-title", "customTitle": "Renamed by me"],
        ]))
        let found = ConversationHistory.list(.claude, folder: project, home: home.root)
        XCTAssertEqual(Set(found.map(\.id)), ["s1", "s2", "s4"])
        XCTAssertEqual(found.first { $0.id == "s1" }?.title, "Build the chart page")
        XCTAssertEqual(found.first { $0.id == "s2" }?.title, "Fix login bug")
        XCTAssertEqual(found.first { $0.id == "s4" }?.title, "Renamed by me")
    }

    // MARK: Codex

    func testCodexSessions() throws {
        let day = ".codex/sessions/2026/11/04"
        try home.file("\(day)/rollout-2026-11-04T10-00-00-aaa.jsonl", jsonl([
            ["type": "session_meta", "payload": ["id": "aaa", "cwd": project.path, "timestamp": "2026-11-04T10:00:00.123Z"]],
            ["type": "response_item", "payload": ["type": "message", "role": "user", "content": [["type": "input_text", "text": "# AGENTS.md instructions for x"]]]],
            ["type": "response_item", "payload": ["type": "message", "role": "user", "content": [["type": "input_text", "text": "<environment_context>…"]]]],
            ["type": "response_item", "payload": ["type": "message", "role": "user", "content": [["type": "input_text", "text": "Fix the build"]]]],
        ]))
        try home.file("\(day)/rollout-2026-11-04T11-00-00-bbb.jsonl", jsonl([
            ["type": "session_meta", "payload": ["id": "bbb", "cwd": project.path, "timestamp": "2026-11-04T11:00:00Z"]],
        ]))
        try home.file("\(day)/rollout-2026-11-04T12-00-00-ccc.jsonl", jsonl([
            ["type": "session_meta", "payload": ["id": "ccc", "cwd": "/elsewhere", "timestamp": "2026-11-04T12:00:00Z"]],
            ["type": "response_item", "payload": ["type": "message", "role": "user", "content": [["type": "input_text", "text": "other"]]]],
        ]))
        try home.file(".codex/session_index.jsonl", jsonl([
            ["id": "bbb", "thread_name": "Named thread", "updated_at": "2026-11-04T18:30:00.123456Z"],
            ["id": "aaa", "thread_name": "", "updated_at": "2026-11-04T10:05:00Z"],
        ]))
        let found = ConversationHistory.list(.codex, folder: project, home: home.root)
        XCTAssertEqual(found.map(\.id), ["bbb", "aaa"], "newest first; other folders left out")
        XCTAssertEqual(found[0].title, "Named thread")
        XCTAssertEqual(found[1].title, "Fix the build")
        XCTAssertEqual(found[0].updated, ConversationHistory.date("2026-11-04T18:30:00Z"))
    }

    // MARK: Gemini CLI

    func testGeminiProjectHash() {
        XCTAssertEqual(ConversationHistory.geminiProjectHash("/Users/me/Projects/site"),
                       "3aa31b12463af2c08da4a4f1d6a4f954c09e77b89b50e28ef99c242286147b51")
    }

    func testGeminiSessions() throws {
        let chats = ".gemini/tmp/\(ConversationHistory.geminiProjectHash(project.path))/chats"
        let session: [String: Any] = ["sessionId": "g-1", "lastUpdated": "2026-11-03T09:00:00.000Z",
                                      "messages": [["type": "gemini", "content": "hi"], ["type": "user", "content": "Plan the release"]]]
        try home.data("\(chats)/session-2026-11-03.json", try JSONSerialization.data(withJSONObject: session))
        try home.file("\(chats)/broken.json", "{")
        let found = ConversationHistory.list(.gemini, folder: project, home: home.root)
        XCTAssertEqual(found.map(\.id), ["g-1"])
        XCTAssertEqual(found.first?.title, "Plan the release")
    }

    // MARK: GitHub Copilot CLI

    func testCopilotSessions() throws {
        try home.file(".copilot/session-state/c-1/workspace.yaml",
                      "id: c-1\ncwd: \(project.path)\nname: Release notes\nuser_named: true\nupdated_at: 2026-11-02T08:00:00.000Z\n")
        try home.file(".copilot/session-state/c-2/workspace.yaml", "id: c-2\ncwd: \"\(project.path)\"\nname: hi\nupdated_at: 2026-11-04T08:00:00.000Z\n")
        try home.file(".copilot/session-state/c-2/events.jsonl", jsonl([
            ["type": "session.start", "data": ["sessionId": "c-2"]],
            ["type": "user.message", "data": ["content": "Add dark mode"]],
        ]))
        try home.file(".copilot/session-state/c-3/workspace.yaml", "id: c-3\ncwd: /elsewhere\nname: x\n")
        let found = ConversationHistory.list(.copilot, folder: project, home: home.root)
        XCTAssertEqual(found.map(\.id), ["c-2", "c-1"])
        XCTAssertEqual(found[0].title, "Add dark mode", "an automatic name is replaced by the first prompt")
        XCTAssertEqual(found[1].title, "Release notes", "a name the user chose is kept")
    }

    // MARK: Antigravity CLI

    func testAntigravityConversations() throws {
        let uri = project.absoluteString                         // file:///…/site/
        let other = "file:///Users/me/elsewhere"
        let meta: [String: Any] = ["conversations": [
            "a1": ["is_internal": false, "last_modified_time": "2026-11-04T10:00:00.123456789-05:00",
                   "summary": ["ID": "a1", "Title": "Add a login page", "UpdatedAt": "2026-11-04T15:00:00.123456Z",
                               "WorkspaceURIs": [uri], "Internal": false]],
            "a2": ["is_internal": false, "summary": ["ID": "a2", "Title": "", "Preview": "Fix the tests",
                                                     "UpdatedAt": "2026-11-03T15:00:00Z", "WorkspaceURIs": [uri]]],
            "a3": ["is_internal": true, "summary": ["ID": "a3", "Title": "internal", "WorkspaceURIs": [uri]]],
            "a4": ["is_internal": false, "summary": ["ID": "a4", "Title": "elsewhere", "WorkspaceURIs": [other]]],
        ]]
        try home.data(".gemini/antigravity-cli/cache/conversation_metadata.json", try JSONSerialization.data(withJSONObject: meta))
        let found = ConversationHistory.list(.antigravity, folder: project, home: home.root)
        XCTAssertEqual(found.map(\.id), ["a1", "a2"])
        XCTAssertEqual(found.map(\.title), ["Add a login page", "Fix the tests"])
        XCTAssertEqual(Harness.antigravity.resumeArguments(.session("a1")), ["--conversation", "a1"])
        XCTAssertEqual(Harness.antigravity.resumeArguments(.last), ["--continue"])
    }

    // MARK: OpenCode

    func testOpenCodeSessionList() {
        let json = """
        [{"id": "ses_a", "title": "Refactor", "directory": "\(project.path)", "time": {"created": 1762200000000, "updated": 1762300000000}},
         {"id": "ses_b", "title": "", "directory": "/elsewhere", "time": {"updated": 1762300000000}},
         {"id": "ses_c", "title": "", "time": {"updated": 1762100000}}]
        """
        let found = ConversationHistory.opencodeSessions(Data(json.utf8), dir: project.path)
        XCTAssertEqual(found.map(\.id), ["ses_a", "ses_c"])
        XCTAssertEqual(found[0].title, "Refactor")
        XCTAssertEqual(found[0].updated, Date(timeIntervalSince1970: 1_762_300_000))
        XCTAssertEqual(found[1].title, "Session ses_c")
        XCTAssertEqual(found[1].updated, Date(timeIntervalSince1970: 1_762_100_000))
        XCTAssertTrue(ConversationHistory.opencodeSessions(Data("not json".utf8), dir: "/").isEmpty)
    }

    // MARK: Helpers

    func testPromptFiltersPlumbingAndShortens() {
        XCTAssertNil(ConversationHistory.prompt("<environment_context>"))
        XCTAssertNil(ConversationHistory.prompt("   "))
        XCTAssertNil(ConversationHistory.prompt(42))
        XCTAssertEqual(ConversationHistory.prompt([["type": "image"], ["type": "text", "text": "Describe this"]]), "Describe this")
        let long = String(repeating: "a", count: 200)
        XCTAssertEqual(ConversationHistory.prompt(long)?.count, 141)
    }

    func testDatesWithAnyFraction() {
        let plain = ConversationHistory.date("2026-11-04T18:30:00Z")
        XCTAssertNotNil(plain)
        XCTAssertEqual(ConversationHistory.date("2026-11-04T18:30:00.123456Z"), plain)
        XCTAssertEqual(ConversationHistory.date("2026-11-04T18:30:00.1Z"), plain)
        XCTAssertNil(ConversationHistory.date("yesterday"))
    }

    func testFoldersWithoutHistory() {
        for h in Harness.agents where h != .opencode {
            XCTAssertTrue(ConversationHistory.list(h, folder: project, home: home.root).isEmpty, h.label)
        }
    }
}
