import Foundation
import SwiftUI

/// The indexing preferences every CLI follows, saved as _Organize Log/preferences.json.
/// `dg inventory` applies private/skip itself and hands style/flagStatus/notes to the AI inside its scan output.
struct IndexPreferencesData: Codable, Equatable {
    var version = 1
    var style = "terse"            // "terse" | "sentence"
    var `private`: [String] = []   // Desktop-relative folder paths: described from names only
    var skip: [String] = []        // Desktop-relative folder paths: never described
    var flagStatus = true
    var notes = ""

    init() {}

    /// From the engine's `prefs/get` / `prefs/set` result.
    init?(json: [String: Any]) {
        guard let raw = try? JSONSerialization.data(withJSONObject: json),
              let d = try? JSONDecoder().decode(IndexPreferencesData.self, from: raw) else { return nil }
        self = d
    }

    var json: [String: Any] {
        ["version": version, "style": style, "private": `private`, "skip": skip, "flagStatus": flagStatus, "notes": notes]
    }
}

/// The Settings model for preferences.json. The engine owns the file (`prefs/get`, `prefs/set`, and a
/// `prefs/changed` notification when an AI app changes it); edits here are saved after a short pause.
@MainActor
final class IndexPreferences: ObservableObject {
    static let shared = IndexPreferences()
    static var fileURL: URL { Layout.logDir.appendingPathComponent("preferences.json") }

    @Published var data = IndexPreferencesData() {
        didSet { if data != oldValue, !applying { scheduleSave() } }
    }
    @Published private(set) var lastError: String?
    /// Set while applying the engine's copy, so that doesn't count as an edit.
    private var applying = false
    private var loaded = false

    enum Mode: String, CaseIterable {
        case include = "Include", privateNames = "Names only", skip = "Skip"
        var wire: String {
            switch self {
            case .include: return "include"
            case .privateNames: return "names_only"
            case .skip: return "skip"
            }
        }
    }

    init() {
        let engine = EngineClient.shared
        engine.on("engine/ready") { [weak self] _ in self?.load() }
        engine.on("prefs/changed") { [weak self] p in self?.apply(p) }
        load()
    }

    func load() {
        guard EngineClient.shared.ready else { return }
        Task {
            do {
                if let r = try await EngineClient.shared.call("prefs/get") as? [String: Any] { apply(r) }
            } catch {
                lastError = error.localizedDescription
            }
        }
    }

    private func apply(_ json: [String: Any]) {
        guard let d = IndexPreferencesData(json: json) else { return }
        loaded = true
        applying = true
        data = d
        applying = false
    }

    func mode(for path: String) -> Mode {
        if data.skip.contains(path) { return .skip }
        if data.private.contains(path) { return .privateNames }
        return .include
    }

    /// Names-only / skip / include for one folder (the engine keeps each folder in one list, both sorted).
    func setMode(_ mode: Mode, for path: String) {
        NSObject.cancelPreviousPerformRequests(withTarget: Saver.shared)
        send(["folder": path, "mode": mode.wire]) { [weak self] d in
            guard let self else { return }
            self.applying = true
            self.data.private = d.private
            self.data.skip = d.skip
            self.applying = false
        }
    }

    private func scheduleSave() {
        NSObject.cancelPreviousPerformRequests(withTarget: Saver.shared)
        Saver.shared.perform(#selector(Saver.save), with: nil, afterDelay: 0.4)
    }

    /// Saves now (and creates preferences.json if it doesn't exist yet), once the engine is up.
    func saveNow() {
        NSObject.cancelPreviousPerformRequests(withTarget: Saver.shared)
        send([:], then: nil)
    }

    /// `prefs/set` with the current data (if we have the engine's copy) plus `extra`.
    private func send(_ extra: [String: Any], then: ((IndexPreferencesData) -> Void)?) {
        Task {
            await EngineClient.shared.waitUntilReady()
            var params = extra
            if loaded { params["data"] = data.json }
            do {
                let r = try await EngineClient.shared.call("prefs/set", params) as? [String: Any] ?? [:]
                lastError = nil
                if let d = IndexPreferencesData(json: r) {
                    if !loaded { apply(r) } else { then?(d) }
                }
            } catch {
                lastError = error.localizedDescription
            }
        }
    }

    /// perform(afterDelay:) needs an NSObject target.
    final class Saver: NSObject {
        static let shared = Saver()
        @objc func save() { MainActor.assumeIsolatedCompat { IndexPreferences.shared.saveNow() } }
    }
}

extension MainActor {
    /// perform(afterDelay:) always fires on the main thread; this just tells the compiler so on macOS 13.
    static func assumeIsolatedCompat(_ body: @escaping @MainActor () -> Void) {
        if #available(macOS 14.0, *) {
            MainActor.assumeIsolated(body)
        } else {
            DispatchQueue.main.async { body() }
        }
    }
}

/// Settings section: style, names-only/skip per folder, status flags and free-form notes.
struct IndexPreferencesSection: View {
    @ObservedObject var store: DesktopStore
    @ObservedObject var reindexer: Reindexer
    @ObservedObject private var prefs = IndexPreferences.shared

    private var folders: [(path: String, depth: Int)] {
        var out: [(path: String, depth: Int)] = []
        for root in store.roots where root.kind == .group {
            out.append((path: root.name, depth: 0))
            for child in root.children where child.kind == .group {
                out.append((path: root.name + "/" + child.name, depth: 1))
            }
        }
        // keep entries for folders that no longer exist visible, so they can be cleared
        let known = Set(out.map { $0.path })
        for p in (prefs.data.private + prefs.data.skip) where !known.contains(p) { out.append((path: p, depth: 0)) }
        return out
    }

    var body: some View {
        VStack(spacing: 0) {
            settingsRow("Blurb style", "How Claude, Codex and the others write each line") {
                Picker("", selection: $prefs.data.style) {
                    Text("Terse").tag("terse")
                    Text("One sentence").tag("sentence")
                }
                .pickerStyle(.segmented).labelsHidden().frame(width: 220)
            }
            settingsRow("Flag project status", "Mark projects as active, backup or archived") {
                Toggle("", isOn: $prefs.data.flagStatus).labelsHidden().toggleStyle(.switch)
            }
            VStack(alignment: .leading, spacing: 6) {
                Text("Notes for the AI").font(.system(size: 13, weight: .medium))
                Text("Context every CLI gets, e.g. renamed projects or what a folder is for")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
                TextEditor(text: $prefs.data.notes)
                    .font(.system(size: 12))
                    .scrollContentBackgroundHidden()
                    .frame(height: 64)
                    .padding(6)
                    .background(RoundedRectangle(cornerRadius: 8).fill(Color.black.opacity(0.08)))
            }
            .padding(.vertical, 9)

            VStack(alignment: .leading, spacing: 4) {
                Text("Folders").font(.system(size: 13, weight: .medium))
                Text("Names only: described from file names, contents never opened. Skip: never described.")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 9)
            ForEach(folders, id: \.path) { folder in
                HStack {
                    Image(systemName: folder.depth == 0 ? Symbols.folder(folder.path) : "folder")
                        .foregroundStyle(.secondary).frame(width: 18)
                    Text(folder.depth == 0 ? folder.path : String(folder.path.split(separator: "/").last ?? ""))
                        .font(.system(size: 12.5))
                    Spacer()
                    Picker("", selection: Binding(
                        get: { prefs.mode(for: folder.path) },
                        set: { prefs.setMode($0, for: folder.path) })) {
                        ForEach(IndexPreferences.Mode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.segmented).labelsHidden().frame(width: 250)
                }
                .padding(.leading, CGFloat(folder.depth) * 22)
                .padding(.vertical, 4)
            }

            settingsRow("Apply to existing descriptions", prefs.lastError.map { "Couldn't save: \($0)" }
                        ?? "New settings apply to the next changes; this rewrites everything now") {
                Button {
                    prefs.saveNow()
                    reindexer.run(reason: "settings-rewrite", full: true)
                } label: {
                    Text(reindexer.running ? "Indexing…" : "Rewrite All")
                        .padding(.horizontal, 12).padding(.vertical, 5)
                }
                .buttonStyle(.plain).glassPill().disabled(reindexer.running)
            }
            .padding(.top, 6)
        }
    }

    private func settingsRow<Control: View>(_ title: String, _ detail: String, @ViewBuilder control: () -> Control) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 13, weight: .medium))
                Text(detail).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(2)
            }
            Spacer()
            control()
        }
        .padding(.vertical, 9)
    }
}

extension View {
    /// TextEditor's background is opaque on macOS 13; hide it where the API exists.
    @ViewBuilder
    func scrollContentBackgroundHidden() -> some View {
        if #available(macOS 13.0, *) { self.scrollContentBackground(.hidden) } else { self }
    }
}
