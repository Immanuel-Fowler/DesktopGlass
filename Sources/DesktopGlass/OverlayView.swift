import SwiftUI

struct OverlayView: View {
    static let searchPlaceholder = "Search your Desktop"
    @ObservedObject var store: DesktopStore
    @ObservedObject var reindexer: Reindexer
    @ObservedObject var detector: HarnessDetector
    @ObservedObject var terminal: TerminalSession
    let close: () -> Void
    @FocusState private var searchFocused: Bool
    @AppStorage("hotkey") private var hotkey = HotKeyChoice.optCmdD.rawValue

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().opacity(0.25)
            // Same layout as the IDE: the agents terminal is docked on the right for good, and collapses to a rail.
            HSplitView {
                HStack(spacing: 0) {
                    Sidebar(store: store).frame(width: 214)
                    Divider().opacity(0.25)
                    Group {
                        if store.showingSettings { SettingsView(store: store, reindexer: reindexer, detector: detector) }
                        else if store.isSearching { SearchResultsView(store: store) }
                        else { BrowseView(store: store) }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                .frame(minWidth: 520)
                .padding(.trailing, store.showTerminal ? 0 : 29)
                if store.showTerminal {
                    TerminalPanel(terminal: terminal, model: store, detector: detector)
                        .frame(minWidth: 260, idealWidth: 400, maxWidth: 900)
                }
            }
            .overlay(alignment: .trailing) {
                if !store.showTerminal {
                    HStack(spacing: 0) {
                        Divider().opacity(0.25)
                        CollapsedTerminalRail(terminal: terminal, model: store)
                    }
                }
            }
            Divider().opacity(0.25)
            footer
        }
        .frame(minWidth: 820, minHeight: 520)
        .onChange(of: store.focusToken) { _ in
            searchFocused = false
            DispatchQueue.main.async { searchFocused = true }
        }
        .onReceive(NotificationCenter.default.publisher(for: .showSettings)) { _ in store.showingSettings = true }
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(.secondary)
            TextField(Self.searchPlaceholder, text: $store.query)
                .textFieldStyle(.plain)
                .font(.system(size: 20, weight: .medium))
                .focused($searchFocused)
                .onSubmit { store.activateFirstResult() }
                .onChange(of: store.query) { q in if !q.isEmpty { store.showingSettings = false } }
            if store.isSearching {
                Button { store.query = "" } label: { Image(systemName: "xmark.circle.fill") }
                    .buttonStyle(.plain).foregroundStyle(.secondary)
            }
            Button { reindexer.run(reason: "button") } label: {
                HStack(spacing: 6) {
                    if reindexer.running { ProgressView().controlSize(.small) } else { Image(systemName: "sparkles") }
                    Text(reindexer.running ? "Indexing…" : "Re-index")
                }
                .font(.system(size: 12.5, weight: .medium))
                .padding(.horizontal, 12).padding(.vertical, 6)
            }
            .buttonStyle(.plain)
            .glassPill()
            .disabled(reindexer.running)
            .help("Update folder descriptions with \(Harness.current.label) (only changed folders)")
            Button { store.showingSettings.toggle() } label: {
                Image(systemName: store.showingSettings ? "gearshape.fill" : "gearshape")
                    .font(.system(size: 13, weight: .medium)).frame(width: 28, height: 28)
            }
            .buttonStyle(.plain)
            .glassPill()
            .help("Settings")
            Button(action: close) {
                Image(systemName: "xmark").font(.system(size: 11, weight: .bold)).frame(width: 28, height: 28)
            }
            .buttonStyle(.plain)
            .glassPill()
            .help("Back to the island (Esc)")
            Button { NSApp.terminate(nil) } label: {
                Image(systemName: "power").font(.system(size: 12, weight: .bold)).frame(width: 28, height: 28)
            }
            .buttonStyle(.plain)
            .foregroundStyle(.red)
            .glassPill()
            .help("Quit DesktopGlass")
        }
        .padding(.horizontal, 20).padding(.vertical, 14)
    }

    private func relative(_ date: Date, _ now: Date) -> String {
        now.timeIntervalSince(date) < 60 ? "just now" : date.formatted(.relative(presentation: .named))
    }

    private func indexText(now: Date) -> String {
        guard let indexed = store.lastIndexed else {
            return "Not indexed yet — choose a backend in ⚙️ Settings, then click Re-index"
        }
        var text = "Updated \(relative(indexed, now))"
        if let checked = store.lastChecked, checked.timeIntervalSince(indexed) > 60 {
            text += " · checked \(relative(checked, now))"
        }
        return text
    }

    private var indexHelp: String {
        let f = DateFormatter(); f.dateStyle = .medium; f.timeStyle = .short
        var lines: [String] = []
        if let d = store.lastIndexed { lines.append("Descriptions updated: \(f.string(from: d))") }
        if let d = store.lastChecked { lines.append("Last successful check: \(f.string(from: d))") }
        return lines.joined(separator: "\n")
    }

    private var footer: some View {
        HStack(spacing: 12) {
            TimelineView(.periodic(from: .now, by: 30)) { context in
                Text(indexText(now: context.date))
                    .help(indexHelp)
            }
            if store.openQuestions > 0 {
                Button("\(store.openQuestions) question\(store.openQuestions == 1 ? "" : "s") for you") {
                    Actions.answerQuestions()
                }
                .buttonStyle(.plain)
                .foregroundStyle(Color.accentColor)
            }
            Spacer()
            if let problem = reindexer.problem {
                ProblemChip(problem: problem, reindexer: reindexer)
            } else if !reindexer.status.isEmpty {
                Text(reindexer.status).lineLimit(1)
            }
            Text("↑↓←→/WASD move · ↩ open · ⇥ back · / search · ⌘= ⌘- size").foregroundStyle(.tertiary).lineLimit(1)
            Text(IslandPrefs.commandD ? "⌘D · \(hotkey)" : hotkey).foregroundStyle(.tertiary)
        }
        .font(.system(size: 11.5))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 20).padding(.vertical, 9)
    }
}

struct ProblemChip: View {
    let problem: IndexProblem
    @ObservedObject var reindexer: Reindexer

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            Text(problem.message).lineLimit(1)
            switch problem {
            case .signedOut:
                Button("Sign In") { Actions.signIn() }.buttonStyle(.plain).foregroundStyle(Color.accentColor)
                Text("·").foregroundStyle(.tertiary)
                Button(reindexer.checkingAuth ? "Checking…" : "Check again") { reindexer.checkAuth() }
                    .buttonStyle(.plain).foregroundStyle(Color.accentColor).disabled(reindexer.checkingAuth)
            case .notRunning(let h):
                Button(h.appBundles.isEmpty ? "Help" : "Open \(h.label)") { Actions.signIn(h) }
                    .buttonStyle(.plain).foregroundStyle(Color.accentColor)
                Text("·").foregroundStyle(.tertiary)
                Button(reindexer.checkingAuth ? "Checking…" : "Check again") { reindexer.checkAuth() }
                    .buttonStyle(.plain).foregroundStyle(Color.accentColor).disabled(reindexer.checkingAuth)
            case .needsConsent(let h):
                Button("Review…") { if Consent.ask(h) { reindexer.run(reason: "button") } }
                    .buttonStyle(.plain).foregroundStyle(Color.accentColor)
            case .notInstalled, .limited:
                Button("Settings") { NotificationCenter.default.post(name: .showSettings, object: nil) }
                    .buttonStyle(.plain).foregroundStyle(Color.accentColor)
            case .failed:
                Button("View log") { NSWorkspace.shared.open(Layout.logDir.appendingPathComponent("reindex.log")) }
                    .buttonStyle(.plain).foregroundStyle(Color.accentColor)
            }
        }
        .help(reindexer.status)
    }
}

extension Notification.Name {
    static let showSettings = Notification.Name("DesktopGlassShowSettings")
}

// MARK: Sidebar

struct Sidebar: View {
    @ObservedObject var store: DesktopStore

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                SidebarRow(title: "Overview", symbol: "square.grid.2x2",
                           selected: store.path.isEmpty && !store.isSearching && !store.showingSettings) { store.select(root: nil) }
                ForEach(store.roots.filter { $0.kind == .group }) { root in
                    SidebarRow(title: root.name, symbol: store.isGamesFolder(root) ? "gamecontroller" : Symbols.folder(root.name),
                               count: root.childCount,
                               selected: store.path.first == root && !store.isSearching && !store.showingSettings) { store.select(root: root) }
                        .contextMenu {
                            if store.isGamesFolder(root) {
                                if !store.gamesFolderName.isEmpty { Button("Stop Using as Games Folder") { store.gamesFolderName = "" } }
                            } else {
                                Button("Use as Games Folder") { store.gamesFolderName = root.name }
                            }
                        }
                }
                Divider().opacity(0.2).padding(.vertical, 6)
                AppsSection(store: store)
            }
            .padding(10)
        }
    }
}

struct SidebarRow: View {
    let title: String
    let symbol: String
    var count: Int? = nil
    let selected: Bool
    let action: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                Image(systemName: symbol).frame(width: 18).foregroundStyle(selected ? HierarchicalShapeStyle.primary : HierarchicalShapeStyle.secondary)
                Text(title).lineLimit(1)
                Spacer(minLength: 4)
                if let count { Text("\(count)").font(.system(size: 11)).foregroundStyle(.secondary) }
            }
            .font(.system(size: 13, weight: selected ? .semibold : .regular))
            .padding(.horizontal, 10).padding(.vertical, 7)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(Color.white.opacity(selected ? 0.16 : (hover ? 0.07 : 0))))
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
    }
}

// MARK: Browse

struct BrowseView: View {
    @ObservedObject var store: DesktopStore
    private let columns = [GridItem(.adaptive(minimum: 236, maximum: 380), spacing: 12)]

    var body: some View {
        ScrollViewReader { proxy in
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                if !store.path.isEmpty {
                    HStack {
                        Breadcrumbs(store: store)
                        Spacer()
                        if store.inGamesFolder { GamesModeToggle(store: store) }
                        if !store.libraryActive { NewItemButton(store: store) }
                    }
                }
                if store.libraryActive {
                    GamesLibraryView(store: store)
                } else {
                if let group = store.path.last {
                    GroupHeader(item: group)
                } else {
                    HStack(alignment: .top) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Desktop").font(.system(size: 26, weight: .semibold))
                            Text("\(store.roots.count) items").foregroundStyle(.secondary)
                        }
                        Spacer()
                        NewItemButton(store: store)
                    }
                }
                if store.currentItems.isEmpty {
                    Text("This folder is empty.").foregroundStyle(.secondary).padding(.top, 20)
                }
                LazyVGrid(columns: columns, alignment: .leading, spacing: 12) {
                    ForEach(Array(store.currentItems.enumerated()), id: \.element.id) { index, item in
                        ItemCard(item: item, isSelected: store.keyboardActive && store.selected == index) { store.activate(item) }
                            .id(item.id)
                    }
                }
                .background(GeometryReader { g in
                    Color.clear
                        .onAppear { store.gridColumns = Self.columnCount(g.size.width) }
                        .onChange(of: g.size.width) { w in store.gridColumns = Self.columnCount(w) }
                })
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .contextMenu {
            if !store.libraryActive {
                Button("New Folder…") { store.startNewItem(folder: true) }
                Button("New File…") { store.startNewItem(folder: false) }
            }
        }
        .onChange(of: store.selected) { i in
            let items = store.currentItems
            if store.keyboardActive, items.indices.contains(i) {
                withAnimation(.easeOut(duration: 0.12)) { proxy.scrollTo(items[i].id, anchor: .center) }
            }
        }
        }
    }

    /// Matches GridItem(.adaptive(minimum: 236), spacing: 12).
    static func columnCount(_ width: CGFloat) -> Int { max(1, Int((width + 12) / (236 + 12))) }
}

/// "+" → a small form to make a folder or file in the folder being browsed (also from right-clicking the grid).
struct NewItemButton: View {
    @ObservedObject var store: DesktopStore

    var body: some View {
        Menu {
            Button("New Folder…") { store.startNewItem(folder: true) }
            Button("New File…") { store.startNewItem(folder: false) }
        } label: {
            Image(systemName: "plus").font(.system(size: 12, weight: .semibold)).frame(width: 28, height: 28)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .glassPill()
        .help("New folder or file here")
        .popover(isPresented: Binding(get: { store.newItem != nil }, set: { if !$0 { store.newItem = nil } }),
                 arrowEdge: .bottom) {
            if let request = store.newItem { NewItemForm(store: store, request: request) }
        }
    }
}

struct NewItemForm: View {
    @ObservedObject var store: DesktopStore
    let request: NewItemRequest
    @State private var isFolder: Bool
    @State private var name = ""
    @State private var problem: String?
    @FocusState private var focused: Bool

    init(store: DesktopStore, request: NewItemRequest) {
        self.store = store
        self.request = request
        _isFolder = State(initialValue: request.isFolder)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Picker("", selection: $isFolder) {
                Label("Folder", systemImage: "folder").tag(true)
                Label("File", systemImage: "doc").tag(false)
            }
            .pickerStyle(.segmented).labelsHidden()
            TextField(isFolder ? "Folder name" : "File name, e.g. notes.md", text: $name)
                .textFieldStyle(.plain)
                .padding(.horizontal, 9).padding(.vertical, 6)
                .background(RoundedRectangle(cornerRadius: 7, style: .continuous).fill(Color.primary.opacity(0.08)))
                .focused($focused)
                .onSubmit(create)
                .onChange(of: name) { _ in problem = nil }
            if let problem { Text(problem).font(.system(size: 11)).foregroundStyle(.orange) }
            HStack {
                Text("in \(request.dir.path == Layout.desktop.path ? "Desktop" : request.dir.lastPathComponent)")
                    .font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1)
                Spacer()
                Button("Cancel") { store.newItem = nil }.keyboardShortcut(.cancelAction)
                Button("Create", action: create).keyboardShortcut(.defaultAction)
            }
        }
        .padding(14)
        .frame(width: 280)
        .onAppear { DispatchQueue.main.async { focused = true } }
    }

    private func create() { problem = store.createItem(named: name, folder: isFolder, in: request.dir) }
}

struct Breadcrumbs: View {
    @ObservedObject var store: DesktopStore

    var body: some View {
        HStack(spacing: 6) {
            Button("Desktop") { store.select(root: nil) }.buttonStyle(.plain)
            ForEach(Array(store.path.enumerated()), id: \.element.id) { index, item in
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .semibold)).foregroundStyle(.tertiary)
                Button(item.name) { store.path = Array(store.path.prefix(index + 1)) }
                    .buttonStyle(.plain)
                    .fontWeight(index == store.path.count - 1 ? .semibold : .regular)
            }
        }
        .font(.system(size: 12))
        .foregroundStyle(.secondary)
    }
}

struct GroupHeader: View {
    let item: Item

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                Text(item.info?.title ?? item.name).font(.system(size: 26, weight: .semibold))
                Button { Actions.reveal(item) } label: { Image(systemName: "folder") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Show in Finder")
                Button { Actions.editHere(item) } label: { Image(systemName: "pencil") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Open this folder in the DesktopGlass editor")
                Button { Actions.openInEditor(item) } label: { Image(systemName: "chevron.left.forwardslash.chevron.right") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).help("Open in \(Editor.current.rawValue)")
            }
            if let summary = item.info?.summary {
                Text(summary).font(.system(size: 14)).foregroundStyle(.secondary)
            } else if item.kind != .group {
                Text(item.blurb ?? (item.kind == .project ? "Project · \(item.childCount) items" : "\(item.childCount) items"))
                    .font(.system(size: 14)).foregroundStyle(.secondary)
            } else {
                Text("No description yet — Re-index to have Claude write one.").font(.system(size: 13)).foregroundStyle(.tertiary)
            }
            if let tags = item.info?.tags, !tags.isEmpty {
                HStack(spacing: 6) {
                    ForEach(tags, id: \.self) { tag in
                        Text(tag).font(.system(size: 11, weight: .medium))
                            .padding(.horizontal, 9).padding(.vertical, 3)
                            .background(Capsule().fill(Color.white.opacity(0.10)))
                    }
                }
            }
            if let notes = item.info?.notes {
                Text(notes).font(.system(size: 12)).foregroundStyle(.tertiary).lineLimit(3)
            }
        }
    }
}

struct ItemCard: View {
    let item: Item
    var isSelected = false
    let activate: () -> Void
    @State private var hover = false

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            FileIcon(url: item.url, size: 40)
            VStack(alignment: .leading, spacing: 3) {
                Text(item.name).font(.system(size: 13.5, weight: .semibold)).lineLimit(1)
                if let blurb = item.blurb, !blurb.isEmpty {
                    Text(blurb).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(2)
                }
                Text(item.subtitle).font(.system(size: 10.5)).foregroundStyle(.tertiary).lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .frame(maxWidth: .infinity, minHeight: 86, alignment: .topLeading)
        .overlay(alignment: .topTrailing) {
            if hover { ItemActions(item: item).padding(6) }
        }
        .glassCard(highlighted: hover || isSelected)
        .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous)
            .strokeBorder(Color.accentColor.opacity(isSelected ? 0.9 : 0), lineWidth: 2))
        .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .onTapGesture(perform: activate)
        .onHover { hover = $0 }
        .contextMenu { ItemMenu(item: item) }
        .help(item.url.path)
    }
}

struct ItemActions: View {
    let item: Item

    var body: some View {
        HStack(spacing: 2) {
            if item.canEditHere {
                iconButton("pencil", "Edit in DesktopGlass") { Actions.editHere(item) }
            }
            iconButton("arrow.up.forward.app", "Open") { Actions.open(item) }
            iconButton("folder", "Show in Finder") { Actions.reveal(item) }
            if item.canOpenInEditor {
                iconButton("chevron.left.forwardslash.chevron.right", "Open in \(Editor.current.rawValue)") { Actions.openInEditor(item) }
            }
        }
        .padding(3)
        .glassPill()
    }

    private func iconButton(_ symbol: String, _ help: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol).font(.system(size: 11, weight: .semibold)).frame(width: 24, height: 22)
        }
        .buttonStyle(.plain)
        .help(help)
    }
}

struct ItemMenu: View {
    let item: Item

    var body: some View {
        if item.canEditHere { Button("Edit in DesktopGlass") { Actions.editHere(item) } }
        if item.kind == .file, ArtifactPolicy.fileExtensions.contains(item.url.pathExtension.lowercased()),
           ["html", "htm", "svg", "pdf"].contains(item.url.pathExtension.lowercased()) {
            Button("Open in Artifact Viewer") { ArtifactRouter.open(item.url, title: item.name) }
        }
        Button("Open") { Actions.open(item) }
        Button("Show in Finder") { Actions.reveal(item) }
        if item.canOpenInEditor { Button("Open in \(Editor.current.rawValue)") { Actions.openInEditor(item) } }
        Divider()
        Button("Copy Path") { Actions.copyPath(item) }
    }
}

// MARK: Search

struct SearchResultsView: View {
    @ObservedObject var store: DesktopStore

    var body: some View {
        let results = store.results
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                if results.isEmpty {
                    Text("No matches for “\(store.query)”").foregroundStyle(.secondary).padding(.top, 24)
                }
                ForEach(Array(results.enumerated()), id: \.element.id) { index, item in
                    SearchRow(item: item, isFirst: index == store.selected) { store.activate(item) }
                }
            }
            .padding(14)
        }
    }
}

struct SearchRow: View {
    let item: Item
    let isFirst: Bool
    let activate: () -> Void
    @State private var hover = false

    var body: some View {
        HStack(spacing: 12) {
            FileIcon(url: item.url, size: 30)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(item.name).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                    Text(item.parentPath).font(.system(size: 11)).foregroundStyle(.tertiary).lineLimit(1)
                }
                if let blurb = item.blurb, !blurb.isEmpty {
                    Text(blurb).font(.system(size: 12)).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer()
            if hover { ItemActions(item: item) }
            else if isFirst { Text("↩").font(.system(size: 12)).foregroundStyle(.tertiary) }
        }
        .padding(.horizontal, 10).padding(.vertical, 7)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous)
            .fill(Color.white.opacity(hover || isFirst ? 0.09 : 0)))
        .contentShape(Rectangle())
        .onTapGesture(perform: activate)
        .onHover { hover = $0 }
        .contextMenu { ItemMenu(item: item) }
    }
}
