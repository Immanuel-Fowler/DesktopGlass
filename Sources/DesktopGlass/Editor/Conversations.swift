import CryptoKit
import Foundation

/// A conversation an agent CLI saved for a folder, which the IDE terminal can pick up again.
struct PastConversation: Identifiable, Hashable {
    let id: String
    let title: String
    let updated: Date
}

/// How the terminal starts a CLI on an earlier conversation.
enum ConversationResume: Hashable {
    case last                 // the most recent conversation in this folder
    case picker               // the CLI's own chooser
    case session(String)      // a specific conversation from ConversationHistory
}

extension Harness {
    /// Arguments that resume a conversation, or nil when this CLI can't do it that way. The model isn't passed:
    /// a resumed conversation keeps the one it was started with.
    func resumeArguments(_ r: ConversationResume) -> [String]? {
        switch (self, r) {
        case (.claude, .last), (.copilot, .last), (.opencode, .last), (.antigravity, .last): return ["--continue"]
        case (.claude, .picker), (.copilot, .picker): return ["--resume"]
        case (.claude, .session(let id)), (.copilot, .session(let id)), (.gemini, .session(let id)): return ["--resume", id]
        case (.codex, .last): return ["resume", "--last"]
        case (.codex, .picker): return ["resume"]
        case (.codex, .session(let id)): return ["resume", id]
        case (.gemini, .last): return ["--resume", "latest"]
        case (.cursor, .last): return ["resume"]
        case (.cursor, .picker): return ["ls"]
        case (.cursor, .session(let id)): return ["--resume", id]
        case (.opencode, .session(let id)): return ["--session", id]
        case (.antigravity, .session(let id)): return ["--conversation", id]
        default: return nil
        }
    }

    /// Agent CLIs keep their conversations; local model chats (Ollama, MLX, LM Studio) don't.
    var canResume: Bool { resumeArguments(.last) != nil }
    var hasResumePicker: Bool { resumeArguments(.picker) != nil }
}

/// Reads each CLI's saved conversations for one folder, newest first. Read-only, and every format is parsed
/// defensively: anything unrecognized is skipped, and the terminal can still use the CLI's own picker.
enum ConversationHistory {
    static func list(_ h: Harness, folder: URL, binary: String? = nil,
                     home: URL = FileManager.default.homeDirectoryForCurrentUser, limit: Int = 60) -> [PastConversation] {
        let dir = folder.standardizedFileURL.path
        let found: [PastConversation]
        switch h {
        case .claude: found = claude(dir, home: home, limit: limit)
        case .codex: found = codex(dir, home: home, limit: limit)
        case .gemini: found = gemini(dir, home: home)
        case .copilot: found = copilot(dir, home: home)
        case .antigravity: found = antigravity(dir, home: home)
        case .opencode: found = binary.map { opencode(dir, binary: $0) } ?? []
        default: found = []            // Cursor keeps chats in a database; its own picker (`ls`) lists them
        }
        return Array(found.sorted { $0.updated > $1.updated }.prefix(limit))
    }

    // MARK: Claude Code — ~/.claude/projects/<folder path with every non-alphanumeric as "-">/<session>.jsonl

    static func claudeProjectName(_ path: String) -> String {
        String(path.unicodeScalars.map { CharacterSet.alphanumerics.contains($0) && $0.isASCII ? Character($0) : "-" })
    }

    static func claude(_ dir: String, home: URL, limit: Int) -> [PastConversation] {
        let project = home.appendingPathComponent(".claude/projects").appendingPathComponent(claudeProjectName(dir))
        return files(in: project, ext: "jsonl").prefix(limit * 2).compactMap { file, modified in
            var title: String?, firstPrompt: String?, cwd: String?
            for line in headLines(file, bytes: 256_000) {
                guard let o = json(line) else { continue }
                if cwd == nil, let c = o["cwd"] as? String { cwd = c }
                switch o["type"] as? String {
                case "custom-title": title = (o["customTitle"] as? String) ?? title
                case "summary": title = title ?? (o["summary"] as? String)
                case "user" where firstPrompt == nil && o["isMeta"] as? Bool != true:
                    firstPrompt = prompt((o["message"] as? [String: Any])?["content"])
                default: break
                }
            }
            if let cwd, standardized(cwd) != dir { return nil }     // another folder whose name maps the same way
            guard let name = title ?? firstPrompt else { return nil }
            return PastConversation(id: file.deletingPathExtension().lastPathComponent, title: name, updated: modified)
        }
    }

    // MARK: Codex — $CODEX_HOME/sessions/YYYY/MM/DD/rollout-…-<id>.jsonl, names in session_index.jsonl

    static func codex(_ dir: String, home: URL, limit: Int) -> [PastConversation] {
        let codexHome = ProcessInfo.processInfo.environment["CODEX_HOME"].map { URL(fileURLWithPath: $0) }
            ?? home.appendingPathComponent(".codex")
        var names: [String: (String, Date?)] = [:]
        for line in headLines(codexHome.appendingPathComponent("session_index.jsonl"), bytes: 4_000_000) {
            guard let o = json(line), let id = o["id"] as? String else { continue }
            names[id] = ((o["thread_name"] as? String) ?? "", (o["updated_at"] as? String).flatMap(date))
        }
        let sessions = codexHome.appendingPathComponent("sessions")
        guard let e = FileManager.default.enumerator(at: sessions, includingPropertiesForKeys: [.contentModificationDateKey]) else { return [] }
        let all = e.compactMap { $0 as? URL }.filter { $0.pathExtension == "jsonl" }
            .sorted { $0.lastPathComponent > $1.lastPathComponent }          // rollout-<timestamp>-… : newest first
        var out: [PastConversation] = []
        for file in all.prefix(800) where out.count < limit {
            let lines = headLines(file, bytes: 512_000)
            guard let first = lines.first.flatMap(json), first["type"] as? String == "session_meta",
                  let meta = first["payload"] as? [String: Any], let id = meta["id"] as? String,
                  let cwd = meta["cwd"] as? String, standardized(cwd) == dir else { continue }
            let index = names[id]
            var title = index?.0 ?? ""
            if title.isEmpty {
                for line in lines.dropFirst() {
                    guard let o = json(line), let p = o["payload"] as? [String: Any], p["role"] as? String == "user",
                          let text = prompt(p["content"]) else { continue }
                    title = text
                    break
                }
            }
            guard !title.isEmpty else { continue }
            let modified = (try? file.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
            let updated = index?.1 ?? modified ?? (meta["timestamp"] as? String).flatMap(date) ?? .distantPast
            out.append(PastConversation(id: id, title: title, updated: updated))
        }
        return out
    }

    // MARK: Gemini CLI — ~/.gemini/tmp/<sha256 of the folder path>/chats/session-*.json

    static func geminiProjectHash(_ path: String) -> String {
        SHA256.hash(data: Data(path.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    static func gemini(_ dir: String, home: URL) -> [PastConversation] {
        let chats = home.appendingPathComponent(".gemini/tmp").appendingPathComponent(geminiProjectHash(dir)).appendingPathComponent("chats")
        return files(in: chats, ext: "json").compactMap { file, modified in
            guard let data = try? Data(contentsOf: file),
                  let o = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let id = o["sessionId"] as? String else { return nil }
            let messages = o["messages"] as? [[String: Any]] ?? []
            guard let first = messages.first(where: { $0["type"] as? String == "user" }), let title = prompt(first["content"]) else { return nil }
            let updated = (o["lastUpdated"] as? String).flatMap(date) ?? modified
            return PastConversation(id: id, title: title, updated: updated)
        }
    }

    // MARK: GitHub Copilot CLI — ~/.copilot/session-state/<id>/workspace.yaml (+ events.jsonl)

    static func copilot(_ dir: String, home: URL) -> [PastConversation] {
        let root = home.appendingPathComponent(".copilot/session-state")
        let folders = (try? FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)) ?? []
        return folders.compactMap { session in
            guard let yaml = try? String(contentsOf: session.appendingPathComponent("workspace.yaml"), encoding: .utf8) else { return nil }
            var fields: [String: String] = [:]
            for line in yaml.components(separatedBy: .newlines) {
                guard let colon = line.firstIndex(of: ":"), !line.hasPrefix(" ") else { continue }
                var value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
                if value.count >= 2, let q = value.first, q == "\"" || q == "'", value.last == q { value = String(value.dropFirst().dropLast()) }
                fields[String(line[..<colon])] = value
            }
            guard let cwd = fields["cwd"], standardized(cwd) == dir else { return nil }
            let id = fields["id"] ?? session.lastPathComponent
            var title = fields["name"] ?? ""
            if title.isEmpty || fields["user_named"] != "true" {
                for line in headLines(session.appendingPathComponent("events.jsonl"), bytes: 512_000) {
                    guard let o = json(line), o["type"] as? String == "user.message",
                          let text = prompt((o["data"] as? [String: Any])?["content"]) else { continue }
                    title = text
                    break
                }
            }
            guard !title.isEmpty else { return nil }
            let updated = fields["updated_at"].flatMap(date) ?? fields["created_at"].flatMap(date) ?? .distantPast
            return PastConversation(id: id, title: title, updated: updated)
        }
    }

    // MARK: Antigravity CLI — ~/.gemini/antigravity-cli/cache/conversation_metadata.json

    static func antigravity(_ dir: String, home: URL) -> [PastConversation] {
        let file = home.appendingPathComponent(".gemini/antigravity-cli/cache/conversation_metadata.json")
        guard let data = try? Data(contentsOf: file),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let conversations = root["conversations"] as? [String: Any] else { return [] }
        return conversations.compactMap { key, value in
            guard let entry = value as? [String: Any], entry["is_internal"] as? Bool != true,
                  let s = entry["summary"] as? [String: Any], s["Internal"] as? Bool != true else { return nil }
            let folders = (s["WorkspaceURIs"] as? [String] ?? []).compactMap { URL(string: $0)?.standardizedFileURL.path }
            guard folders.contains(dir) else { return nil }
            let title = [s["Title"], s["Preview"]].compactMap { ($0 as? String).flatMap(prompt) }.first ?? "Conversation \(key.prefix(8))"
            let updated = (s["UpdatedAt"] as? String).flatMap(date) ?? (entry["last_modified_time"] as? String).flatMap(date) ?? .distantPast
            return PastConversation(id: (s["ID"] as? String) ?? key, title: title, updated: updated)
        }
    }

    // MARK: OpenCode — `opencode session list --format json` (run in the folder: sessions belong to a project)

    static func opencode(_ dir: String, binary: String) -> [PastConversation] {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: binary)
        p.arguments = ["session", "list", "--format", "json", "--max-count", "60"]
        p.currentDirectoryURL = URL(fileURLWithPath: dir)
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return [] }
        DispatchQueue.global().asyncAfter(deadline: .now() + 15) { if p.isRunning { p.terminate() } }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return opencodeSessions(data, dir: dir)
    }

    static func opencodeSessions(_ data: Data, dir: String) -> [PastConversation] {
        guard let any = try? JSONSerialization.jsonObject(with: data) else { return [] }
        let rows = (any as? [[String: Any]]) ?? ((any as? [String: Any])?["sessions"] as? [[String: Any]]) ?? []
        return rows.compactMap { o in
            guard let id = o["id"] as? String else { return nil }
            if let d = (o["directory"] as? String) ?? (o["cwd"] as? String), standardized(d) != dir { return nil }
            let time = o["time"] as? [String: Any]
            let stamp = time?["updated"] ?? o["updated"] ?? time?["created"] ?? o["created"]
            let updated: Date
            if let n = stamp as? Double { updated = Date(timeIntervalSince1970: n > 10_000_000_000 ? n / 1000 : n) }
            else if let s = stamp as? String, let d = date(s) { updated = d }
            else { updated = .distantPast }
            let title = (o["title"] as? String).flatMap { $0.isEmpty ? nil : $0 } ?? "Session \(id.prefix(8))"
            return PastConversation(id: id, title: title, updated: updated)
        }
    }

    // MARK: Helpers

    /// Files with an extension in a folder, newest first, with their modification dates.
    private static func files(in dir: URL, ext: String) -> [(URL, Date)] {
        let urls = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: [.contentModificationDateKey])) ?? []
        return urls.filter { $0.pathExtension == ext }.map { u in
            (u, (try? u.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate ?? .distantPast)
        }.sorted { $0.1 > $1.1 }
    }

    /// Whole lines from the start of a file, reading at most `bytes` (session logs can be very large).
    static func headLines(_ url: URL, bytes: Int) -> [String] {
        guard let h = try? FileHandle(forReadingFrom: url) else { return [] }
        defer { try? h.close() }
        guard let data = try? h.read(upToCount: bytes), !data.isEmpty else { return [] }
        var lines = String(decoding: data, as: UTF8.self).components(separatedBy: "\n")
        if data.count == bytes { lines.removeLast() }                 // probably cut mid-line
        return lines.filter { !$0.isEmpty }
    }

    private static func json(_ line: String) -> [String: Any]? {
        try? JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any]
    }

    /// The text of a user message (a string, or text parts), unless it's tool plumbing rather than something typed.
    static func prompt(_ content: Any?) -> String? {
        var text: String?
        if let s = content as? String { text = s }
        else if let parts = content as? [[String: Any]] {
            text = parts.lazy.compactMap { ($0["text"] as? String) }.first { !$0.isEmpty }
        }
        guard var t = text?.trimmingCharacters(in: .whitespacesAndNewlines), !t.isEmpty else { return nil }
        let plumbing = ["<", "# AGENTS.md", "Caveat:", "[Request interrupted"]
        if plumbing.contains(where: { t.hasPrefix($0) }) { return nil }
        t = t.replacingOccurrences(of: "\\s+", with: " ", options: .regularExpression)
        return t.count > 140 ? String(t.prefix(140)) + "…" : t
    }

    private static func standardized(_ path: String) -> String {
        URL(fileURLWithPath: path).standardizedFileURL.path
    }

    /// ISO 8601 with any number of fractional digits (Codex writes microseconds).
    static func date(_ s: String) -> Date? {
        let trimmed = s.replacingOccurrences(of: "\\.\\d+", with: "", options: .regularExpression)
        return iso.date(from: trimmed)
    }

    private static let iso: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()
}
