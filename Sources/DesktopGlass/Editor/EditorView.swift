import AppKit
import SwiftUI

struct EditorView: View {
    @ObservedObject var model: EditorModel
    @ObservedObject var terminal: TerminalSession
    @ObservedObject var detector: HarnessDetector
    let backToDesktop: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            EditorToolbar(model: model, backToDesktop: backToDesktop)
            Divider().opacity(0.25)
            HSplitView {
                if model.showSidebar {
                    FileTreeView(model: model)
                        .frame(minWidth: 180, idealWidth: 250, maxWidth: 460)
                }
                VStack(spacing: 0) {
                    TabStrip(model: model)
                    Divider().opacity(0.2)
                    ZStack {
                        Color(nsColor: .textBackgroundColor).opacity(0.16)
                        if let page = model.activeWeb {
                            EditorWebPane(tab: page).id(page.id)
                        } else if let doc = model.active {
                            ActiveDocumentView(document: doc, model: model).id(ObjectIdentifier(doc))
                        } else {
                            EmptyEditorView(model: model)
                        }
                    }
                    Divider().opacity(0.2)
                    if model.activeWeb == nil, let doc = model.active { StatusBar(document: doc, model: model) } else { StatusBarEmpty(model: model) }
                }
                .frame(minWidth: 320)
                .padding(.trailing, model.showTerminal ? 0 : 29)
                if model.showTerminal {
                    TerminalPanel(terminal: terminal, model: model, detector: detector)
                        .frame(minWidth: 260, idealWidth: 420, maxWidth: 900)
                }
            }
            .overlay(alignment: .trailing) {
                if !model.showTerminal {
                    HStack(spacing: 0) {
                        Divider().opacity(0.25)
                        CollapsedTerminalRail(terminal: terminal, model: model)
                    }
                }
            }
        }
        .background(GlassSheen())
        .ignoresSafeArea()
        .overlay {
            if model.showQuickOpen { QuickOpenView(model: model).transition(.opacity) }
        }
        .animation(.easeOut(duration: 0.15), value: model.showQuickOpen)
    }
}

/// A soft top-down highlight and a hairline edge over the window's blur, so the editor reads as a pane of glass.
private struct GlassSheen: View {
    var body: some View {
        ZStack {
            LinearGradient(colors: [Color.white.opacity(0.10), Color.white.opacity(0.02), .clear],
                           startPoint: .top, endPoint: .bottom)
            LinearGradient(colors: [Color.white.opacity(0.05), .clear], startPoint: .topLeading, endPoint: .center)
        }
        .overlay(Rectangle().strokeBorder(Color.white.opacity(0.12), lineWidth: 1))
        .allowsHitTesting(false)
    }
}

// MARK: Toolbar

struct EditorToolbar: View {
    @ObservedObject var model: EditorModel
    let backToDesktop: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            Color.clear.frame(width: 70, height: 1)          // room for the traffic lights
            Button(action: backToDesktop) {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.left").font(.system(size: 11, weight: .bold))
                    Image(systemName: "square.grid.2x2")
                    Text("Desktop")
                }
                .font(.system(size: 12, weight: .medium))
                .padding(.horizontal, 10).padding(.vertical, 5)
            }
            .buttonStyle(.plain).glassPill()
            .help("Shrink back to the Desktop overlay")

            Button { model.showSidebar.toggle() } label: {
                Image(systemName: "sidebar.left").frame(width: 26, height: 22)
            }
            .buttonStyle(.plain).glassPill().help("Toggle sidebar (⌘B)")

            WindowDragArea().frame(maxWidth: .infinity, maxHeight: .infinity)
            VStack(spacing: 1) {
                Text(model.active?.name ?? model.rootName).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                if let doc = model.active {
                    Text(model.relativePath(doc.url)).font(.system(size: 10.5)).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            WindowDragArea().frame(maxWidth: .infinity, maxHeight: .infinity)

            Button { model.showQuickOpen = true } label: {
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass")
                    Text("Go to File").foregroundStyle(.secondary)
                    Text("⌘P").foregroundStyle(.tertiary)
                }
                .font(.system(size: 12))
                .padding(.horizontal, 10).padding(.vertical, 5)
            }
            .buttonStyle(.plain).glassPill()
            Button { model.chooseFolder() } label: {
                Image(systemName: "folder.badge.plus").frame(width: 26, height: 22)
            }
            .buttonStyle(.plain).glassPill().help("Open Folder… (⌘O)")

            Menu {
                if let doc = model.active {
                    Button("Open in \(Editor.external.rawValue)") { Editor.external.open(doc.url) }
                    Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([doc.url]) }
                    Divider()
                }
                if let root = model.root {
                    Button("Open Folder in \(Editor.external.rawValue)") { Editor.external.open(root.url) }
                    Button("Open Folder in Terminal") { openTerminal(at: root.url) }
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .menuStyle(.borderlessButton)
            .frame(width: 34)
        }
        .padding(.trailing, 12)
        .frame(height: 44)
    }

    private func openTerminal(at url: URL) {
        if let terminal = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal") {
            NSWorkspace.shared.open([url], withApplicationAt: terminal, configuration: NSWorkspace.OpenConfiguration())
        }
    }
}

// MARK: File tree

struct FileTreeView: View {
    @ObservedObject var model: EditorModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 4) {
                Text(model.rootName.uppercased())
                    .font(.system(size: 10.5, weight: .semibold)).foregroundStyle(.secondary).lineLimit(1)
                Spacer()
                if let root = model.root {
                    treeButton("doc.badge.plus", "New File") { model.newFile(in: root.url) }
                    treeButton("folder.badge.plus", "New Folder") { model.newFolder(in: root.url) }
                    treeButton("arrow.clockwise", "Refresh") { model.refresh() }
                    treeButton("arrow.down.right.and.arrow.up.left", "Collapse All") { model.collapseAll() }
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 8)
            if model.root == nil {
                VStack(spacing: 10) {
                    Text("No folder open").foregroundStyle(.secondary)
                    Button("Open Folder…") { model.chooseFolder() }
                        .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 5).glassPill()
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 0) {
                        ForEach(model.rows) { row in TreeRowView(row: row, model: model) }
                    }
                    .padding(.horizontal, 6).padding(.bottom, 10)
                }
            }
        }
        .background(Color.black.opacity(0.04))
    }

    private func treeButton(_ symbol: String, _ help: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: symbol).font(.system(size: 11)).frame(width: 20, height: 18) }
            .buttonStyle(.plain).foregroundStyle(.secondary).help(help)
    }
}

struct TreeRowView: View {
    let row: TreeRow
    @ObservedObject var model: EditorModel
    @State private var hover = false

    private var isActive: Bool { model.active?.url.standardizedFileURL == row.node.url.standardizedFileURL }
    private var isOpen: Bool { model.documents.contains { $0.url.standardizedFileURL == row.node.url.standardizedFileURL } }

    var body: some View {
        HStack(spacing: 5) {
            Color.clear.frame(width: CGFloat(row.depth) * 14, height: 1)
            if row.node.isDirectory {
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(.tertiary)
                    .rotationEffect(.degrees(model.isExpanded(row.node) ? 90 : 0))
                    .frame(width: 12)
            } else {
                Color.clear.frame(width: 12, height: 1)
            }
            FileIcon(url: row.node.url, size: 16)
            Text(row.node.name)
                .font(.system(size: 12.5, weight: isActive ? .semibold : .regular))
                .foregroundStyle(row.node.isDimmed ? HierarchicalShapeStyle.tertiary : HierarchicalShapeStyle.primary)
                .lineLimit(1)
            Spacer(minLength: 0)
            if isOpen && !isActive { Circle().fill(Color.secondary.opacity(0.5)).frame(width: 5, height: 5) }
        }
        .padding(.vertical, 3).padding(.horizontal, 6)
        .background(RoundedRectangle(cornerRadius: 6, style: .continuous)
            .fill(isActive ? Color.accentColor.opacity(0.28) : Color.white.opacity(hover ? 0.07 : 0)))
        .contentShape(Rectangle())
        .onTapGesture {
            if row.node.isDirectory { withAnimation(.easeOut(duration: 0.12)) { model.toggle(row.node) } }
            else { model.openFile(row.node.url) }
        }
        .onHover { hover = $0 }
        .contextMenu {
            let dir = row.node.isDirectory ? row.node.url : row.node.url.deletingLastPathComponent()
            if !row.node.isDirectory { Button("Open") { model.openFile(row.node.url) } }
            Button("New File…") { model.newFile(in: dir) }
            Button("New Folder…") { model.newFolder(in: dir) }
            Divider()
            Button("Open in \(Editor.external.rawValue)") { Editor.external.open(row.node.url) }
            Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([row.node.url]) }
            Button("Copy Path") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(row.node.url.path, forType: .string)
            }
            Divider()
            Button("Move to Trash…") { model.moveToTrash(row.node) }
        }
    }
}

// MARK: Tabs

@MainActor
struct TabStrip: View {
    @ObservedObject var model: EditorModel
    @ObservedObject private var browser = ArtifactBrowserController.shared
    @State private var showServers = false

    var body: some View {
        HStack(spacing: 6) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 4) {
                    ForEach(model.documents, id: \.url) { doc in
                        TabItem(document: doc, model: model)
                    }
                    ForEach(browser.editorTabs) { page in
                        WebTabItem(tab: page, model: model, browser: browser)
                    }
                    newTabMenu
                }
                .padding(.horizontal, 8).padding(.vertical, 5)
            }
            if let page = model.activeWeb {
                WebTabControls(tab: page, browser: browser).padding(.trailing, 10)
            } else if let doc = model.active {
                DocumentModeButtons(document: doc).padding(.trailing, 10)
            }
        }
        .frame(height: 36)
        .popover(isPresented: $showServers, arrowEdge: .bottom) {
            ServerList(browser: browser, placement: .editor) { showServers = false }.frame(width: 420)
        }
    }

    /// "+": open a view next to the code — a preview of this file, a local server, or any artifact address.
    private var newTabMenu: some View {
        Menu {
            Button("New Tab") { browser.newTab(placement: .editor) }
            Divider()
            if let doc = model.active, doc.isWebPreviewable {
                Button("Preview \(doc.name)") {
                    if doc.isDirty { doc.save() }
                    browser.open(doc.url, title: doc.name, placement: .editor)
                }
            }
            Button("Local Server…") { showServers = true }
            Divider()
            Button("Viewer Window") { browser.showWindow() }
        } label: {
            Image(systemName: "plus").font(.system(size: 11, weight: .semibold)).frame(width: 22, height: 20)
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
        .help("New tab with an address bar, a preview of this file, or a local server")
    }
}

/// A page from the artifact viewer, as a tab beside the files.
@MainActor
struct WebTabItem: View {
    @ObservedObject var tab: ArtifactTab
    @ObservedObject var model: EditorModel
    let browser: ArtifactBrowserController
    @State private var hover = false

    private var isActive: Bool { model.activeWeb === tab }

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: tab.url.isFileURL ? "eye" : (tab.url.host?.contains("localhost") == true ? "server.rack" : "globe"))
                .font(.system(size: 10.5)).foregroundStyle(.secondary)
            Text(tab.title).font(.system(size: 12, weight: isActive ? .semibold : .regular)).lineLimit(1).frame(maxWidth: 180)
            if tab.isLoading { ProgressView().controlSize(.mini) }
            Button { browser.close(tab) } label: { Image(systemName: "xmark").font(.system(size: 8.5, weight: .bold)) }
                .buttonStyle(.plain).foregroundStyle(.secondary).opacity(hover || isActive ? 1 : 0)
                .frame(width: 14, height: 14)
        }
        .padding(.leading, 10).padding(.trailing, 6).padding(.vertical, 5)
        .background {
            if isActive { Capsule().fill(Color.white.opacity(0.14)) }
            else if hover { Capsule().fill(Color.white.opacity(0.06)) }
        }
        .contentShape(Capsule())
        .onTapGesture { model.activeWeb = tab; browser.active = tab }
        .onHover { hover = $0 }
        .contextMenu {
            Button("Open in Viewer Window") { browser.move(tab, toEditor: false) }
            Button("Close") { browser.close(tab) }
        }
        .help(tab.url.isFileURL ? tab.url.path : tab.url.absoluteString)
    }
}

/// Back/forward/reload and the page's actions, in the tab strip while a page is showing.
@MainActor
struct WebTabControls: View {
    @ObservedObject var tab: ArtifactTab
    let browser: ArtifactBrowserController

    var body: some View {
        HStack(spacing: 10) {
            NavButtons(tab: tab)
            AddressBar(tab: tab).frame(minWidth: 140, idealWidth: 260, maxWidth: 320)
            TabActions(tab: tab)
            Button { browser.move(tab, toEditor: false) } label: { Image(systemName: "macwindow") }
                .buttonStyle(.plain).foregroundStyle(.secondary).help("Open in its own window")
        }
    }
}

/// The page itself, in the code area.
@MainActor
struct EditorWebPane: View {
    @ObservedObject var tab: ArtifactTab

    var body: some View {
        WebPage(tab: tab, browser: ArtifactBrowserController.shared)
    }
}

/// Source text, or the visual HTML editor for HTML tabs in Visual mode.
struct ActiveDocumentView: View {
    @ObservedObject var document: EditorDocument
    @ObservedObject var model: EditorModel

    var body: some View {
        if document.isHTML && document.isVisual {
            HTMLVisualEditorView(session: model.visualSession(for: document))
        } else {
            CodeEditorHost(document: document)
        }
    }
}

/// Preview ⇄ Source for Markdown; "View" opens HTML/SVG in the artifact viewer (it live-reloads on save).
struct DocumentModeButtons: View {
    @ObservedObject var document: EditorDocument

    var body: some View {
        HStack(spacing: 6) {
            if document.isMarkdown {
                Picker("", selection: Binding(get: { document.isPreviewing }, set: { document.setPreview($0) })) {
                    Label("Source", systemImage: "chevron.left.forwardslash.chevron.right").tag(false)
                    Label("Preview", systemImage: "eye").tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 170)
                .help("Preview is still editable — ⇧⌘V toggles")
            }
            if document.isHTML {
                Picker("", selection: $document.isVisual) {
                    Label("Source", systemImage: "chevron.left.forwardslash.chevron.right").tag(false)
                    Label("Visual", systemImage: "paintbrush.pointed").tag(true)
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(width: 160)
                .help("Visual: click elements on the rendered page and edit their CSS — written to your stylesheet")
            }
            if document.isWebPreviewable {
                Button {
                    if document.isDirty { document.save() }
                    ArtifactRouter.open(document.url, title: document.name)
                } label: {
                    Label("View", systemImage: "sparkles.rectangle.stack").font(.system(size: 12))
                        .padding(.horizontal, 10).padding(.vertical, 4)
                }
                .buttonStyle(.plain).glassPill()
                .help("Show it in a tab next to the code — reloads every time you save")
            }
        }
    }
}

struct TabItem: View {
    @ObservedObject var document: EditorDocument
    @ObservedObject var model: EditorModel
    @State private var hover = false

    private var isActive: Bool { model.active === document }

    var body: some View {
        HStack(spacing: 6) {
            FileIcon(url: document.url, size: 14)
            Text(document.name).font(.system(size: 12, weight: isActive ? .semibold : .regular)).lineLimit(1)
            ZStack {
                if document.isDirty && !hover {
                    Circle().fill(Color.primary.opacity(0.6)).frame(width: 7, height: 7)
                } else {
                    Button { model.close(document) } label: {
                        Image(systemName: "xmark").font(.system(size: 8.5, weight: .bold))
                    }
                    .buttonStyle(.plain).foregroundStyle(.secondary)
                    .opacity(hover || isActive ? 1 : 0)
                }
            }
            .frame(width: 14, height: 14)
        }
        .padding(.leading, 10).padding(.trailing, 6).padding(.vertical, 5)
        .background {
            if isActive { Capsule().fill(Color.white.opacity(0.14)) }
            else if hover { Capsule().fill(Color.white.opacity(0.06)) }
        }
        .contentShape(Capsule())
        .onTapGesture { model.active = document; model.activeWeb = nil }
        .onHover { hover = $0 }
        .help(document.url.path)
    }
}

// MARK: Editor host

@MainActor
struct CodeEditorHost: NSViewRepresentable {
    let document: EditorDocument

    func makeNSView(context: Context) -> NSView {
        let container = NSView()
        embed(in: container)
        return container
    }

    func updateNSView(_ container: NSView, context: Context) {
        if container.subviews.first !== document.scrollView { embed(in: container) }
    }

    @MainActor
    private func embed(in container: NSView) {
        container.subviews.forEach { $0.removeFromSuperview() }
        let scroll = document.scrollView
        scroll.frame = container.bounds
        scroll.autoresizingMask = [.width, .height]
        container.addSubview(scroll)
        let tv = document.textView
        DispatchQueue.main.async { tv.window?.makeFirstResponder(tv) }
    }
}

struct EmptyEditorView: View {
    @ObservedObject var model: EditorModel

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "chevron.left.forwardslash.chevron.right")
                .font(.system(size: 34, weight: .light)).foregroundStyle(.tertiary)
            Text(model.root == nil ? "Open a folder to start" : "Pick a file from the sidebar")
                .font(.system(size: 15, weight: .medium)).foregroundStyle(.secondary)
            HStack(spacing: 10) {
                if model.root != nil {
                    Button("Go to File  ⌘P") { model.showQuickOpen = true }
                        .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 6).glassPill()
                }
                Button("Open Folder…  ⌘O") { model.chooseFolder() }
                    .buttonStyle(.plain).padding(.horizontal, 12).padding(.vertical, 6).glassPill()
            }
        }
    }
}

// MARK: Status bar

struct StatusBar: View {
    @ObservedObject var document: EditorDocument
    @ObservedObject var model: EditorModel

    var body: some View {
        HStack(spacing: 14) {
            Text(document.language.name)
            Text("Ln \(document.line), Col \(document.column)")
            Text(document.indentUnit == "\t" ? "Tabs" : "Spaces: \(document.indentUnit.count)")
            Text("UTF-8")
            if document.isDirty { Text("Edited").foregroundStyle(.orange) }
            Spacer()
            FontSizeControl(model: model)
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 14)
        .frame(height: 26)
    }
}

struct StatusBarEmpty: View {
    @ObservedObject var model: EditorModel
    var body: some View {
        HStack {
            Text(model.root == nil ? "" : "\(model.fileIndex.count) files")
            Spacer()
            FontSizeControl(model: model)
        }
        .font(.system(size: 11)).foregroundStyle(.secondary)
        .padding(.horizontal, 14).frame(height: 26)
    }
}

struct FontSizeControl: View {
    @ObservedObject var model: EditorModel
    var body: some View {
        HStack(spacing: 6) {
            Button { model.changeFontSize(by: -1) } label: { Image(systemName: "textformat.size.smaller") }.buttonStyle(.plain)
            Text("\(Int(model.fontSize)) pt").monospacedDigit()
            Button { model.changeFontSize(by: 1) } label: { Image(systemName: "textformat.size.larger") }.buttonStyle(.plain)
        }
    }
}

// MARK: Quick Open (⌘P)

struct QuickOpenView: View {
    @ObservedObject var model: EditorModel
    @State private var query = ""
    @State private var selection = 0
    @State private var monitor: Any?
    @FocusState private var focused: Bool

    var body: some View {
        let results = model.quickOpenResults(query)
        ZStack(alignment: .top) {
            Color.black.opacity(0.18).onTapGesture { close() }
            VStack(spacing: 0) {
                HStack(spacing: 10) {
                    Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                    TextField("Go to file in \(model.rootName)", text: $query)
                        .textFieldStyle(.plain)
                        .font(.system(size: 16))
                        .focused($focused)
                        .onSubmit { open(results) }
                        .onChange(of: query) { _ in selection = 0 }
                }
                .padding(14)
                Divider().opacity(0.3)
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 2) {
                            ForEach(Array(results.enumerated()), id: \.element) { index, url in
                                HStack(spacing: 10) {
                                    FileIcon(url: url, size: 18)
                                    Text(url.lastPathComponent).font(.system(size: 13, weight: .medium)).lineLimit(1)
                                    Text(model.relativePath(url.deletingLastPathComponent()))
                                        .font(.system(size: 11)).foregroundStyle(.tertiary).lineLimit(1)
                                    Spacer()
                                }
                                .padding(.horizontal, 12).padding(.vertical, 6)
                                .background(RoundedRectangle(cornerRadius: 8, style: .continuous)
                                    .fill(index == selection ? Color.accentColor.opacity(0.3) : .clear))
                                .contentShape(Rectangle())
                                .onTapGesture { selection = index; open(results) }
                                .id(index)
                            }
                            if results.isEmpty {
                                Text(model.fileIndex.isEmpty ? "Indexing files…" : "No matching files")
                                    .foregroundStyle(.secondary).padding(20)
                            }
                        }
                        .padding(6)
                    }
                    .frame(maxHeight: 360)
                    .onChange(of: selection) { i in proxy.scrollTo(i) }
                }
            }
            .frame(width: 600)
            .glassCard(cornerRadius: 18)
            .shadow(color: .black.opacity(0.25), radius: 30, y: 12)
            .padding(.top, 70)
        }
        .onAppear {
            focused = true
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
                switch event.keyCode {
                case 125: selection = min(selection + 1, max(0, model.quickOpenResults(query).count - 1)); return nil
                case 126: selection = max(selection - 1, 0); return nil
                case 53: close(); return nil
                default: return event
                }
            }
        }
        .onDisappear {
            if let m = monitor { NSEvent.removeMonitor(m) }
            monitor = nil
        }
    }

    private func open(_ results: [URL]) {
        guard results.indices.contains(selection) else { return }
        let url = results[selection]
        close()
        model.openFile(url)
    }

    private func close() {
        query = ""
        model.showQuickOpen = false
    }
}
