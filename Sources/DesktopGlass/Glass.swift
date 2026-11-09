import AppKit
import SwiftUI

extension View {
    /// Liquid Glass on macOS 26+, frosted material before that (an edge only when highlighted).
    @ViewBuilder
    func glassCard(cornerRadius: CGFloat = 16, highlighted: Bool = false) -> some View {
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            self.glassEffect(.regular.interactive(), in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
        } else {
            legacyGlass(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous), highlighted: highlighted)
        }
        #else
        legacyGlass(RoundedRectangle(cornerRadius: cornerRadius, style: .continuous), highlighted: highlighted)
        #endif
    }

    @ViewBuilder
    func glassPill() -> some View {
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            self.glassEffect(.regular.interactive(), in: Capsule())
        } else {
            legacyGlass(Capsule(), highlighted: false)
        }
        #else
        legacyGlass(Capsule(), highlighted: false)
        #endif
    }

    func legacyGlass<S: InsettableShape>(_ shape: S, highlighted: Bool) -> some View {
        self.background(.ultraThinMaterial, in: shape)
            .overlay(shape.strokeBorder(Color.white.opacity(highlighted ? 0.35 : 0), lineWidth: 1))   // edge only when highlighted
    }
}

enum Symbols {
    static func folder(_ name: String) -> String {
        let n = name.lowercased()
        if n.contains("project") { return "hammer" }
        if n.contains("client") { return "briefcase" }
        if n.contains("school") || n.contains("class") { return "graduationcap" }
        if n.contains("game") { return "gamecontroller" }
        if n.contains("media") || n.contains("photo") || n.contains("image") { return "photo.on.rectangle" }
        if n.contains("video") { return "film" }
        if n.contains("music") { return "music.note" }
        if n.contains("document") || n.contains("doc") { return "doc.text" }
        if n.contains("archive") || n.contains("zip") { return "archivebox" }
        if n.contains("church") { return "building.columns" }
        return "folder"
    }
}

enum IconCache {
    private static var cache: [String: NSImage] = [:]
    static func icon(for url: URL) -> NSImage {
        if let hit = cache[url.path] { return hit }
        let img = NSWorkspace.shared.icon(forFile: url.path)
        cache[url.path] = img
        return img
    }
}

struct FileIcon: View {
    let url: URL
    var size: CGFloat = 36
    var body: some View {
        Image(nsImage: IconCache.icon(for: url))
            .resizable()
            .interpolation(.high)
            .frame(width: size, height: size)
    }
}
