import AppKit
import SwiftUI

/// The editor's window: a normal, resizable, full-screen-capable macOS window with a glass background.
@MainActor
final class EditorWindowController: NSObject, NSWindowDelegate {
    let model = EditorModel()
    let terminal = TerminalSession()
    let detector: HarnessDetector
    let window: NSWindow
    var onBackToDesktop: (() -> Void)?
    var onClosed: (() -> Void)?

    init(detector: HarnessDetector) {
        self.detector = detector
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 800),
                          styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                          backing: .buffered, defer: false)
        super.init()
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.title = "DesktopGlass Editor"
        window.isOpaque = false
        window.backgroundColor = .clear
        window.isMovableByWindowBackground = false   // the toolbar is the drag handle
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: 760, height: 440)
        window.collectionBehavior = [.fullScreenPrimary]
        window.delegate = self

        let host = NSHostingView(rootView: EditorView(model: model, terminal: terminal, detector: detector) { [weak self] in
            self?.onBackToDesktop?()
        })
        window.contentView = Self.glassBackdrop(around: host)
    }

    /// Liquid Glass on macOS 26; a light, see-through HUD blur before that (the old under-window material read as flat grey).
    private static func glassBackdrop(around host: NSView) -> NSView {
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            let glass = NSGlassEffectView()
            glass.style = .clear
            host.autoresizingMask = [.width, .height]
            host.frame = glass.bounds
            glass.contentView = host
            return glass
        }
        #endif
        host.translatesAutoresizingMaskIntoConstraints = false
        let blur = NSVisualEffectView()
        blur.material = .hudWindow
        blur.blendingMode = .behindWindow
        blur.state = .active
        blur.addSubview(host)
        NSLayoutConstraint.activate([
            host.leadingAnchor.constraint(equalTo: blur.leadingAnchor),
            host.trailingAnchor.constraint(equalTo: blur.trailingAnchor),
            host.topAnchor.constraint(equalTo: blur.topAnchor),
            host.bottomAnchor.constraint(equalTo: blur.bottomAnchor),
        ])
        return blur
    }

    var isVisible: Bool { window.isVisible }

    func windowShouldClose(_ sender: NSWindow) -> Bool { model.confirmCloseAll() }

    func windowWillClose(_ notification: Notification) {
        terminal.stop()
        onClosed?()
    }

    func windowDidBecomeKey(_ notification: Notification) { model.reloadChangedFiles() }
}

/// Transparent strip that drags the window (SwiftUI content covers the hidden title bar).
struct WindowDragArea: NSViewRepresentable {
    final class DragView: NSView {
        override var mouseDownCanMoveWindow: Bool { true }
        override func mouseDown(with event: NSEvent) {
            if event.clickCount == 2 { window?.performZoom(nil) } else { window?.performDrag(with: event) }
        }
    }
    func makeNSView(context: Context) -> NSView { DragView() }
    func updateNSView(_ nsView: NSView, context: Context) {}
}

/// Lets any view ask the app to show something in the artifact viewer.
enum ArtifactRouter {
    static var handler: ((URL, String?) -> Void)?
    static func open(_ url: URL, title: String? = nil) { handler?(url, title) }
}

/// Lets overlay views ask the app to open something in the built-in editor.
enum EditorRouter {
    static var handler: ((URL) -> Void)?
    static func open(_ url: URL) { handler?(url) }
}
