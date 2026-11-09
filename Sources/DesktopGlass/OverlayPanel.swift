import AppKit
import SwiftUI

/// Borderless floating panel with a real behind-window blur (Liquid Glass on macOS 26).
final class OverlayPanel: NSPanel {
    init<V: View>(rootView: V) {
        super.init(contentRect: NSRect(x: 0, y: 0, width: 1060, height: 680),
                   styleMask: [.nonactivatingPanel, .borderless, .fullSizeContentView, .resizable],
                   backing: .buffered, defer: false)
        isFloatingPanel = true
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false     // the window shadow brings a hairline outline with it
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = false
        isMovableByWindowBackground = true    // GlassTerminalView opts out, so clicks still reach the terminal
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        minSize = NSSize(width: 820, height: 520)

        let host = NSHostingView(rootView: rootView)
        host.translatesAutoresizingMaskIntoConstraints = false

        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            let glass = NSGlassEffectView()
            glass.cornerRadius = 26
            contentView = glass
            host.translatesAutoresizingMaskIntoConstraints = true
            host.autoresizingMask = [.width, .height]
            host.frame = glass.bounds
            glass.contentView = host
            return
        }
        #endif

        let blur = NSVisualEffectView()
        blur.material = .hudWindow
        blur.blendingMode = .behindWindow
        blur.state = .active
        // Rounded with a mask image: a layer corner mask leaves a faint rim around the glass.
        let r: CGFloat = 26
        let mask = NSImage(size: NSSize(width: r * 2 + 1, height: r * 2 + 1), flipped: false) { rect in
            NSColor.black.setFill()
            NSBezierPath(roundedRect: rect, xRadius: r, yRadius: r).fill()
            return true
        }
        mask.capInsets = NSEdgeInsets(top: r, left: r, bottom: r, right: r)
        mask.resizingMode = .stretch
        blur.maskImage = mask
        blur.addSubview(host)
        NSLayoutConstraint.activate([
            host.leadingAnchor.constraint(equalTo: blur.leadingAnchor),
            host.trailingAnchor.constraint(equalTo: blur.trailingAnchor),
            host.topAnchor.constraint(equalTo: blur.topAnchor),
            host.bottomAnchor.constraint(equalTo: blur.bottomAnchor),
        ])
        contentView = blur
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
    /// Esc folds the panel back into the island.
    var onCancel: (() -> Void)?
    override func cancelOperation(_ sender: Any?) { if let onCancel { onCancel() } else { orderOut(nil) } }

    func showCentered() {
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { NSMouseInRect(mouse, $0.frame, false) } ?? NSScreen.main
        if let visible = screen?.visibleFrame {
            let size = frame.size
            setFrameOrigin(NSPoint(x: visible.midX - size.width / 2, y: visible.midY - size.height / 2 + visible.height * 0.04))
        }
        makeKeyAndOrderFront(nil)
    }
}
