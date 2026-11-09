import AppKit
import SwiftUI

/// Runs the engine's `dg mcp-install` and keeps the per-app MCP status for Settings.
@MainActor
final class MCPManager: ObservableObject {
    static let shared = MCPManager()

    struct Client: Identifiable, Decodable {
        let id: String
        let name: String
        let detected: Bool
        let installed: Bool
        let note: String?
        /// Agent CLIs: whether their global instructions mention DesktopGlass (nil = the app has no such file).
        let instructions: Bool?
        /// Claude Code: whether the hook that shows HTML/SVG it writes in the viewer is installed.
        let autoshow: Bool?
        /// Claude Code, Codex: whether the SessionStart hook that tells them they run inside DesktopGlass is installed.
        let sessionHook: Bool?

        /// Everything DesktopGlass adds to this app is in place.
        var fullyInstalled: Bool { installed && instructions != false && autoshow != false && sessionHook != false }
    }
    struct TestResult: Decodable { let ok: Bool; let tools: [String]; let status: String; let ms: Int; let error: String }
    struct Server: Decodable { let command: String; let args: [String] }
    private struct Output: Decodable {
        let server: Server
        let clients: [Client]
        let done: [String]?
        let errors: [String]?
        let test: TestResult?
    }

    @Published private(set) var clients: [Client] = []
    @Published private(set) var server: Server?
    @Published private(set) var busy: String?          // "install" | "uninstall" | "test" | "status"
    @Published private(set) var message = ""
    @Published private(set) var lastTest: TestResult?

    /// The engine: the `dg` bundled next to the app's executable, else the one install.sh links into ~/.local/bin.
    static var engine: URL? {
        let bundled = Bundle.main.executableURL?.deletingLastPathComponent().appendingPathComponent("dg")
        let linked = Layout.home.appendingPathComponent(".local/bin/dg")
        return [bundled, linked].compactMap { $0 }.first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }

    var detected: [Client] { clients.filter(\.detected) }
    var installedCount: Int { clients.filter(\.installed).count }

    func refresh() { run("status") }
    func installAll() { run("install") }
    func uninstallAll() { run("uninstall") }
    func test() { run("test") }

    private func run(_ command: String) {
        guard busy == nil else { return }
        guard let engine = Self.engine else {
            message = "Run install.sh first — the DesktopGlass engine (dg) isn't installed yet"
            return
        }
        busy = command
        DispatchQueue.global(qos: .userInitiated).async {
            let p = Process()
            p.executableURL = engine
            p.arguments = ["mcp-install", command]
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = "\(Layout.home.path)/.local/bin:/opt/homebrew/bin:/usr/local/bin:" + (env["PATH"] ?? "/usr/bin:/bin")
            p.environment = env
            let out = Pipe(), err = Pipe()
            p.standardOutput = out
            p.standardError = err
            p.standardInput = FileHandle.nullDevice
            var decoded: Output?
            var failure = ""
            do {
                try p.run()
                let data = out.fileHandleForReading.readDataToEndOfFile()
                let errData = err.fileHandleForReading.readDataToEndOfFile()
                p.waitUntilExit()
                decoded = try? JSONDecoder().decode(Output.self, from: data)
                if decoded == nil { failure = String(data: errData, encoding: .utf8)?.split(separator: "\n").last.map(String.init) ?? "No output" }
            } catch {
                failure = error.localizedDescription
            }
            let result = decoded
            let problem = failure
            DispatchQueue.main.async {
                self.busy = nil
                guard let result else { self.message = "Script failed: \(problem)"; return }
                self.clients = result.clients
                self.server = result.server
                switch command {
                case "install":
                    let done = result.done ?? []
                    var text = done.isEmpty ? "No AI apps found to add it to" : "Added to \(done.joined(separator: ", "))"
                    if done.contains(where: { ["Claude Desktop", "Cursor", "LM Studio", "Windsurf", "VS Code"].contains($0) }) {
                        text += " · restart those apps to load it"
                    }
                    if let errors = result.errors, !errors.isEmpty { text += " · " + errors.joined(separator: "; ") }
                    self.message = text
                case "uninstall":
                    self.message = (result.done ?? []).isEmpty ? "Nothing to remove" : "Removed from \(result.done!.joined(separator: ", "))"
                case "test":
                    self.lastTest = result.test
                    if let t = result.test {
                        self.message = t.ok ? "Server OK · \(t.tools.count) tools · answered in \(t.ms) ms · \(t.status)"
                                            : "Server didn't respond: \(t.error)"
                    }
                default:
                    break
                }
            }
        }
    }

    /// Standard mcpServers JSON, for any app the installer doesn't know about.
    var configSnippet: String {
        guard let s = server else { return "" }
        let args = s.args.map { "\"\($0)\"" }.joined(separator: ", ")
        return """
        {
          "mcpServers": {
            "desktopglass": {
              "command": "\(s.command)",
              "args": [\(args)]
            }
          }
        }
        """
    }
}

struct MCPSection: View {
    @ObservedObject private var mcp = MCPManager.shared
    @State private var copied = false
    @AppStorage("mcp.allow.edit") private var allowEdit = false
    @AppStorage("mcp.allow.control") private var allowControl = false
    @AppStorage("mcp.allow.script") private var allowScript = false
    @AppStorage("mcp.allow.play") private var allowPlay = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("DesktopGlass MCP server").font(.system(size: 13, weight: .medium))
                    Text(summary).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(2)
                }
                Spacer()
                actionButton("Test", "test", primary: false) { mcp.test() }
                    .help("Runs the server and calls it like an AI app would")
                actionButton(mcp.installedCount > 0 && mcp.detected.allSatisfy(\.fullyInstalled) ? "Installed ✓" : "Install in All Apps",
                             "install", primary: true) { mcp.installAll() }
                    .help("Adds the server to every AI app found on this Mac — no prompts")
            }
            .padding(.vertical, 9)

            if !mcp.message.isEmpty {
                Text(mcp.message)
                    .font(.system(size: 11.5))
                    .foregroundStyle(mcp.message.hasPrefix("Script failed") || mcp.message.hasPrefix("Server didn't") ? Color.orange : Color.secondary)
                    .padding(.bottom, 8)
            }

            // Per-app status
            let shown = mcp.clients.filter(\.detected)
            if !shown.isEmpty {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 170), spacing: 8)], alignment: .leading, spacing: 6) {
                    ForEach(shown) { c in
                        HStack(spacing: 6) {
                            Image(systemName: c.installed ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(c.installed ? Color.green : Color.secondary)
                            Text(c.name).font(.system(size: 12)).lineLimit(1)
                            if c.instructions == true {
                                Image(systemName: "text.badge.checkmark").font(.system(size: 10)).foregroundStyle(.secondary)
                            }
                        }
                        .help(clientHelp(c))
                    }
                }
                .padding(.bottom, 10)
            }

            // What AI apps may do (reading is always allowed)
            VStack(alignment: .leading, spacing: 6) {
                Text("AI apps can always read your Desktop index and open the artifact viewer. Also allow them to:")
                    .font(.system(size: 11.5)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                Toggle("Edit files — descriptions, answers, HTML and CSS", isOn: $allowEdit)
                Toggle("Change settings and start re-indexing", isOn: $allowControl)
                Toggle("Click, type and run JavaScript in the artifact viewer", isOn: $allowScript)
                Toggle("Launch games from the Games folder", isOn: $allowPlay)
            }
            .toggleStyle(.checkbox)
            .font(.system(size: 12))
            .padding(.bottom, 10)

            // Tools + command
            VStack(alignment: .leading, spacing: 6) {
                Text("33 tools — Desktop: overview, folders, search, recent changes, edit descriptions, answer questions · Indexing: status, re-index, schedule, backend, preferences · Browser: show artifacts, local servers, read, screenshot, click, type, run JS · HTML: outline, set styles, split style, group, text · Windows: editor, overlay")
                    .font(.system(size: 11)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                if let s = mcp.server {
                    HStack(alignment: .top, spacing: 8) {
                        Text(([s.command] + s.args).joined(separator: " "))
                            .font(.system(size: 11, design: .monospaced))
                            .textSelection(.enabled)
                            .lineLimit(2)
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button(copied ? "Copied" : "Copy JSON") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(mcp.configSnippet, forType: .string)
                            copied = true
                            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { copied = false }
                        }
                        .buttonStyle(.plain).foregroundStyle(Color.accentColor).font(.system(size: 11.5))
                        .help("mcpServers config for any other MCP app")
                    }
                }
            }
            .padding(.bottom, 9)

            if mcp.installedCount > 0 {
                HStack {
                    Spacer()
                    Button("Remove from all apps") { mcp.uninstallAll() }
                        .buttonStyle(.plain).foregroundStyle(.secondary).font(.system(size: 11.5))
                        .disabled(mcp.busy != nil)
                }
                .padding(.bottom, 8)
            }
        }
        .onAppear { if mcp.clients.isEmpty { mcp.refresh() } }
    }

    private var summary: String {
        if mcp.busy == "status" && mcp.clients.isEmpty { return "Looking for AI apps…" }
        let found = mcp.detected.count
        if found == 0 { return "Lets AI apps search and read your organized Desktop. No MCP apps found yet." }
        return "Lets AI apps search and read your organized Desktop · in \(mcp.installedCount) of \(found) apps found"
    }

    private func actionButton(_ title: String, _ key: String, primary: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if mcp.busy == key { ProgressView().controlSize(.small) }
                Text(title).fontWeight(primary ? .semibold : .regular)
            }
            .padding(.horizontal, 12).padding(.vertical, 5)
        }
        .buttonStyle(.plain)
        .glassPill()
        .disabled(mcp.busy != nil)
    }
}

extension MCPSection {
    func clientHelp(_ c: MCPManager.Client) -> String {
        if let note = c.note { return note }
        guard c.installed else { return "Not added yet" }
        var text = "\(c.name) can use the DesktopGlass tools"
        if c.instructions == true { text += " and its instructions point it at your Desktop index" }
        if !c.fullyInstalled { text += " · Install again to add the newest DesktopGlass setup" }
        if c.autoshow == true { text += ". In DesktopGlass's terminal, HTML and SVG files it writes open in the viewer" }
        if c.sessionHook == true { text += ". Sessions started in DesktopGlass's terminal know they're inside it and show their output there" }
        if c.id == "codex" && c.sessionHook == true { text += " (Codex asks you to trust the hook once)" }
        return text
    }
}
