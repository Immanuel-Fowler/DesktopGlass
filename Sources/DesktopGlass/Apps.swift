import AppKit
import SwiftUI

/// An app shown in the sidebar's Apps dropdown.
struct AppEntry: Identifiable, Hashable {
    let url: URL
    let name: String
    var id: String { url.path }
}

/// Installed Mac apps, for the sidebar's Apps dropdown. Apps built on the DesktopGlass app standard
/// (see plans/APP_STANDARD.md) will be listed here too once the engine can run them.
@MainActor
final class AppCatalog: ObservableObject {
    static let shared = AppCatalog()

    @Published private(set) var apps: [AppEntry] = []
    @Published private(set) var loading = false
    private var loadedAt: Date?
    private var icons: [String: NSImage] = [:]

    /// Folders searched, one level of subfolders deep (e.g. /Applications/Utilities).
    nonisolated static var searchRoots: [URL] {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return ["/Applications", "/System/Applications", home.appendingPathComponent("Applications").path]
            .map { URL(fileURLWithPath: $0, isDirectory: true) }
    }

    /// Rescans at most every five minutes unless forced.
    func refresh(force: Bool = false) {
        if loading { return }
        if !force, let t = loadedAt, Date().timeIntervalSince(t) < 300 { return }
        loading = true
        DispatchQueue.global(qos: .utility).async {
            let found = Self.scan()
            DispatchQueue.main.async {
                self.apps = found
                self.loadedAt = Date()
                self.loading = false
            }
        }
    }

    func filtered(_ query: String) -> [AppEntry] {
        let terms = query.lowercased().split(separator: " ").map(String.init)
        guard !terms.isEmpty else { return apps }
        let hits = apps.filter { a in terms.allSatisfy { a.name.lowercased().contains($0) } }
        let first = terms[0]
        return hits.sorted { a, b in
            let ap = a.name.lowercased().hasPrefix(first), bp = b.name.lowercased().hasPrefix(first)
            if ap != bp { return ap }
            return a.name.localizedStandardCompare(b.name) == .orderedAscending
        }
    }

    func icon(for app: AppEntry) -> NSImage {
        if let i = icons[app.id] { return i }
        let i = NSWorkspace.shared.icon(forFile: app.url.path)
        i.size = NSSize(width: 32, height: 32)
        icons[app.id] = i
        return i
    }

    func open(_ app: AppEntry) {
        NSWorkspace.shared.openApplication(at: app.url, configuration: NSWorkspace.OpenConfiguration())
    }

    nonisolated static func scan() -> [AppEntry] {
        let fm = FileManager.default
        var seen = Set<String>()
        var out: [AppEntry] = []
        func add(_ url: URL) {
            let name = fm.displayName(atPath: url.path).replacingOccurrences(of: ".app", with: "")
            guard seen.insert(url.resolvingSymlinksInPath().path).inserted else { return }
            out.append(AppEntry(url: url, name: name))
        }
        for root in searchRoots {
            for url in (try? fm.contentsOfDirectory(at: root, includingPropertiesForKeys: [.isDirectoryKey],
                                                    options: [.skipsHiddenFiles])) ?? [] {
                if url.pathExtension == "app" { add(url); continue }
                guard (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true else { continue }
                for inner in (try? fm.contentsOfDirectory(at: url, includingPropertiesForKeys: nil,
                                                          options: [.skipsHiddenFiles])) ?? []
                where inner.pathExtension == "app" { add(inner) }
            }
        }
        return out.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }
}

/// Collapsible, searchable list of apps under the Desktop section of the sidebar.
struct AppsSection: View {
    @ObservedObject var store: DesktopStore
    @ObservedObject private var catalog = AppCatalog.shared

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Button { withAnimation(.easeOut(duration: 0.18)) { store.appsExpanded.toggle() } } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.system(size: 9, weight: .bold))
                        .rotationEffect(.degrees(store.appsExpanded ? 90 : 0))
                    Text("Apps").font(.system(size: 11, weight: .semibold))
                    Spacer()
                    if !catalog.apps.isEmpty { Text("\(catalog.apps.count)").font(.system(size: 10.5)) }
                }
                .foregroundStyle(.secondary)
                .padding(.horizontal, 10).padding(.vertical, 5)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(store.appsExpanded ? "Collapse Apps" : "Expand Apps")

            if store.appsExpanded {
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass").font(.system(size: 10.5)).foregroundStyle(.secondary)
                    TextField("Search apps", text: $store.appsQuery)
                        .textFieldStyle(.plain).font(.system(size: 12))
                        .onSubmit { if let first = catalog.filtered(store.appsQuery).first { catalog.open(first) } }
                    if !store.appsQuery.isEmpty {
                        Button { store.appsQuery = "" } label: { Image(systemName: "xmark.circle.fill") }
                            .buttonStyle(.plain).foregroundStyle(.secondary).font(.system(size: 11))
                    }
                }
                .padding(.horizontal, 9).padding(.vertical, 5)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color.white.opacity(0.07)))
                .padding(.horizontal, 4).padding(.bottom, 2)

                let shown = catalog.filtered(store.appsQuery)
                if catalog.loading && catalog.apps.isEmpty {
                    ProgressView().controlSize(.small).frame(maxWidth: .infinity).padding(.vertical, 6)
                } else if shown.isEmpty {
                    Text("No apps match").font(.system(size: 11.5)).foregroundStyle(.tertiary)
                        .padding(.horizontal, 10).padding(.vertical, 4)
                } else {
                    // Rows are cheap, but a few hundred icons are not: render lazily.
                    LazyVStack(alignment: .leading, spacing: 1) {
                        ForEach(shown) { app in AppRow(app: app, catalog: catalog) }
                    }
                }
            }
        }
        .onAppear { catalog.refresh() }
    }
}

private struct AppRow: View {
    let app: AppEntry
    let catalog: AppCatalog
    @State private var hover = false

    var body: some View {
        Button { catalog.open(app) } label: {
            HStack(spacing: 8) {
                Image(nsImage: catalog.icon(for: app)).resizable().frame(width: 18, height: 18)
                Text(app.name).lineLimit(1)
                Spacer(minLength: 0)
            }
            .font(.system(size: 12.5))
            .padding(.horizontal, 10).padding(.vertical, 4)
            .contentShape(Rectangle())
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color.white.opacity(hover ? 0.07 : 0)))
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .contextMenu {
            Button("Open") { catalog.open(app) }
            Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([app.url]) }
        }
        .help(app.url.path)
    }
}
