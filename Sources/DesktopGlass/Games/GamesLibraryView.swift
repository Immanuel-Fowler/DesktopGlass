import AppKit
import SwiftUI

/// Library | Folder switch — only shown inside the Games folder.
struct GamesModeToggle: View {
    @ObservedObject var store: DesktopStore

    var body: some View {
        Picker("", selection: Binding(get: { store.gameLibraryOn }, set: { on in
            store.gameLibraryOn = on
            if on, let games = store.path.first { store.path = [games] }
        })) {
            Label("Library", systemImage: "gamecontroller").tag(true)
            Label("Folder", systemImage: "folder").tag(false)
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(width: 190)
        .help("Library: every Mac game in this folder, ready to play")
    }
}

struct GamesLibraryView: View {
    @ObservedObject var store: DesktopStore
    @ObservedObject private var library = GameLibrary.shared
    @State private var showFinder = false
    private let columns = [GridItem(.adaptive(minimum: 236, maximum: 380), spacing: 12)]

    private var root: URL? { store.path.first?.url }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            if !library.message.isEmpty {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "info.circle").foregroundStyle(.secondary)
                    Text(library.message).font(.system(size: 12)).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer()
                    Button { library.message = "" } label: { Image(systemName: "xmark") }.buttonStyle(.plain).foregroundStyle(.tertiary)
                }
                .padding(10)
                .glassCard(cornerRadius: 12)
            }
            if library.items.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text(library.scanning ? "Looking for games…" : "No games found in this folder.")
                        .foregroundStyle(.secondary)
                    if !library.scanning {
                        Text("Recognized: Mac apps, and shortcuts to them. Use Find Games… to add games installed elsewhere on this Mac.")
                            .font(.system(size: 12)).foregroundStyle(.tertiary).fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(.top, 10)
            }
            LazyVGrid(columns: columns, alignment: .leading, spacing: 12) {
                ForEach(Array(library.items.enumerated()), id: \.element.id) { index, item in
                    if let game = library.entry(for: item.url) {
                        GameCard(item: item, game: game, library: library,
                                 isSelected: store.keyboardActive && store.selected == index) { library.play(item.url) }
                            .id(item.id)
                    }
                }
            }
            .background(GeometryReader { g in
                Color.clear
                    .onAppear { store.gridColumns = BrowseView.columnCount(g.size.width) }
                    .onChange(of: g.size.width) { w in store.gridColumns = BrowseView.columnCount(w) }
            })
        }
        .onAppear { if let root { library.scan(root) } }
        .sheet(isPresented: $showFinder) {
            if let root { GameFinderSheet(gamesRoot: root) { summary in
                showFinder = false
                if let summary { library.message = summary }
            } }
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Games").font(.system(size: 26, weight: .semibold))
                Text(summary).foregroundStyle(.secondary).font(.system(size: 13))
            }
            Spacer()
            Menu {
                Picker("Sort By", selection: Binding(get: { library.sortOrder }, set: { library.sortOrder = $0 })) {
                    ForEach(GameLibrary.SortOrder.allCases) { Text($0.label).tag($0) }
                }
                .pickerStyle(.inline)
            } label: {
                Image(systemName: "line.3.horizontal.decrease").font(.system(size: 12)).frame(width: 26, height: 22)
            }
            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
            .padding(.horizontal, 4).padding(.vertical, 1).glassPill()
            .help("Sort")
            Button { showFinder = true } label: {
                Label("Find Games…", systemImage: "magnifyingglass").font(.system(size: 12)).padding(.horizontal, 10).padding(.vertical, 5)
            }
            .buttonStyle(.plain).glassPill()
            .help("Search this Mac for games and add shortcuts to them here")
            Button { if let root { library.scan(root, force: true) } } label: {
                Group {
                    if library.scanning { ProgressView().controlSize(.small) } else { Image(systemName: "arrow.clockwise") }
                }
                .frame(width: 26, height: 22)
            }
            .buttonStyle(.plain).glassPill().help("Rescan games")
        }
    }

    private var summary: String {
        library.items.isEmpty ? "Library" : "\(library.items.count) game\(library.items.count == 1 ? "" : "s")"
    }
}

struct GameCard: View {
    let item: Item
    let game: GameEntry
    @ObservedObject var library: GameLibrary
    let isSelected: Bool
    let play: () -> Void
    @State private var hover = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            FileIcon(url: game.target, size: 40)
            VStack(alignment: .leading, spacing: 3) {
                Text(game.title).font(.system(size: 13.5, weight: .semibold)).lineLimit(2)
                Label(game.platform.name, systemImage: game.platform.symbol)
                    .font(.system(size: 11.5)).foregroundStyle(.secondary).lineLimit(1)
                if let played = game.lastPlayed {
                    Text("Played " + Self.relative.localizedString(for: played, relativeTo: Date()))
                        .font(.system(size: 10.5)).foregroundStyle(.tertiary).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .frame(maxWidth: .infinity, minHeight: 86, alignment: .topLeading)
        .overlay(alignment: .bottomTrailing) {
            if hover || isSelected {
                Button(action: play) {
                    Image(systemName: "play.fill").font(.system(size: 12, weight: .bold)).frame(width: 30, height: 26)
                }
                .buttonStyle(.plain).glassPill().padding(8).help("Play (Return)")
            }
        }
        .glassCard(highlighted: hover || isSelected)
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous)
            .strokeBorder(Color.accentColor.opacity(isSelected ? 0.9 : 0), lineWidth: 2))
        .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .onTapGesture(perform: play)
        .onHover { hover = $0 }
        .contextMenu { menu }
        .help(game.url.path)
    }

    private static let relative: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter(); f.unitsStyle = .full; return f
    }()

    @ViewBuilder
    private var menu: some View {
        Button("Play") { play() }
        Divider()
        Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([game.target]) }
        if game.target != game.url {
            Button("Show Shortcut in Finder") { NSWorkspace.shared.activateFileViewerSelecting([game.url]) }
        }
    }
}

// MARK: - Find Games

/// A game found on this Mac that isn't in the Games folder yet.
struct FoundGame: Identifiable, Hashable {
    let url: URL
    let title: String
    let platform: GamePlatform
    let source: String
    var id: URL { url }
}

/// Searches the Mac for games (the engine's `games/search`: apps whose Info.plist says they're games, and Steam
/// libraries) and adds Finder-alias shortcuts to them in the Games folder (`games/add_shortcuts`).
@MainActor
final class GameFinder: ObservableObject {
    @Published private(set) var results: [FoundGame] = []
    @Published private(set) var scanning = false
    @Published private(set) var status = ""
    @Published var selected: Set<URL> = []

    func scan(gamesRoot: URL) {
        guard !scanning else { return }
        scanning = true
        status = "Searching this Mac…"
        results = []
        Task {
            let r = (try? await EngineClient.shared.call("games/search", ["root": gamesRoot.path])) as? [String: Any]
            let found = (r?["games"] as? [[String: Any]] ?? []).compactMap { g -> FoundGame? in
                guard let path = g["path"] as? String else { return nil }
                return FoundGame(url: URL(fileURLWithPath: path), title: g["title"] as? String ?? "",
                                 platform: GamePlatform(rawValue: g["platform_id"] as? String ?? "") ?? .mac,
                                 source: g["source"] as? String ?? "")
            }
            results = found
            selected = Set(found.map(\.url))
            scanning = false
            status = r?["status"] as? String ?? "Couldn't search: the DesktopGlass engine isn't running"
        }
    }

    /// Creates a Finder alias in the Games folder for each selected game. Returns a summary line.
    func addShortcuts(to root: URL) async -> String {
        let picked = results.filter { selected.contains($0.url) }
        let games = picked.map { ["path": $0.url.path, "title": $0.title, "source": $0.source] }
        let r: [String: Any]
        do {
            r = try await EngineClient.shared.call("games/add_shortcuts", ["root": root.path, "games": games]) as? [String: Any] ?? [:]
        } catch {
            return error.localizedDescription
        }
        let failed = r["failed"] as? [String] ?? []
        results.removeAll { selected.contains($0.url) && !failed.contains($0.title) }
        selected = []
        GameLibrary.shared.scan(root, force: true)
        return r["summary"] as? String ?? ""
    }
}

struct GameFinderSheet: View {
    let gamesRoot: URL
    let close: (String?) -> Void
    @StateObject private var finder = GameFinder()

    private var grouped: [(String, [FoundGame])] {
        var order: [String] = []
        var map: [String: [FoundGame]] = [:]
        for g in finder.results {
            if map[g.source] == nil { order.append(g.source) }
            map[g.source, default: []].append(g)
        }
        return order.map { ($0, map[$0]!) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Find Games on This Mac").font(.headline)
            Text("Looks for Mac apps marked as games and Mac games in Steam libraries. Selected games get a shortcut (a Finder alias) in \(gamesRoot.lastPathComponent) — nothing is moved or copied.")
                .font(.system(size: 12)).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                if finder.scanning { ProgressView().controlSize(.small) }
                Text(finder.status).font(.system(size: 12)).foregroundStyle(.secondary)
                Spacer()
                if !finder.results.isEmpty {
                    Button(finder.selected.count == finder.results.count ? "Select None" : "Select All") {
                        finder.selected = finder.selected.count == finder.results.count ? [] : Set(finder.results.map(\.url))
                    }
                    .buttonStyle(.plain).foregroundStyle(Color.accentColor).font(.system(size: 12))
                }
            }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2) {
                    ForEach(grouped, id: \.0) { source, games in
                        Text(source).font(.system(size: 11, weight: .semibold)).foregroundStyle(.secondary)
                            .padding(.top, 8).padding(.bottom, 2).lineLimit(1).truncationMode(.middle)
                        ForEach(games) { g in row(g) }
                    }
                }
            }
            .frame(height: 340)
            HStack {
                Button("Search Again") { finder.scan(gamesRoot: gamesRoot) }.disabled(finder.scanning)
                Spacer()
                Button("Cancel") { close(nil) }.keyboardShortcut(.cancelAction)
                Button("Add \(finder.selected.count) Shortcut\(finder.selected.count == 1 ? "" : "s")") {
                    Task { close(await finder.addShortcuts(to: gamesRoot)) }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(finder.selected.isEmpty || finder.scanning)
            }
        }
        .padding(20)
        .frame(width: 560)
        .onAppear { finder.scan(gamesRoot: gamesRoot) }
    }

    private func row(_ g: FoundGame) -> some View {
        Toggle(isOn: Binding(get: { finder.selected.contains(g.url) },
                             set: { on in if on { finder.selected.insert(g.url) } else { finder.selected.remove(g.url) } })) {
            HStack(spacing: 8) {
                FileIcon(url: g.url, size: 18)
                Text(g.title).font(.system(size: 12.5)).lineLimit(1)
                Text(g.platform.name).font(.system(size: 11)).foregroundStyle(.tertiary)
            }
        }
        .toggleStyle(.checkbox)
        .help(g.url.path)
    }
}
