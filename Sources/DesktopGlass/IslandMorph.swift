import AppKit

/// The overlay ⇄ island morph: one live glass shape that goes from the overlay's rounded rectangle to the pill's
/// capsule. The real overlay dissolves into it at the start, and a live copy of the pill fades in at the end. Nothing
/// is snapshotted (snapshots paint glass as solid gray/black), so the morph stays see-through the whole way.
@MainActor
final class IslandMorph {
    static let panelRadius: CGFloat = 26

    private let window: NSWindow
    private let shape = NSVisualEffectView()
    private let pillCopy: NSView
    private weak var panel: NSWindow?
    private let from: NSRect, to: NSRect
    private var timer: Timer?
    /// 0 = the overlay, 1 = the pill.
    private(set) var progress: CGFloat = 0

    init(panel: NSWindow, pillRect: NSRect) {
        self.panel = panel
        from = panel.frame
        to = pillRect
        let area = from.union(to).insetBy(dx: -40, dy: -40)
        window = NSWindow(contentRect: area, styleMask: [.borderless], backing: .buffered, defer: false)
        window.level = panel.level
        window.backgroundColor = .clear
        window.isOpaque = false
        window.hasShadow = false
        window.ignoresMouseEvents = true
        window.isReleasedWhenClosed = false
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient, .ignoresCycle]

        let root = NSView(frame: NSRect(origin: .zero, size: area.size))
        root.wantsLayer = true
        window.contentView = root

        // Same glass as the overlay; rounded with a mask image rather than a layer mask (no dark rim at the edge).
        shape.material = .hudWindow
        shape.blendingMode = .behindWindow
        shape.state = .active
        shape.wantsLayer = true
        root.addSubview(shape)

        pillCopy = IslandPillWindow.previewView()
        pillCopy.wantsLayer = true
        pillCopy.frame = NSRect(origin: .zero, size: IslandPillWindow.windowSize)
        root.addSubview(pillCopy)
    }

    func show(at progress: CGFloat) {
        set(progress)
        if let panel, panel.isVisible { window.order(.below, relativeTo: panel.windowNumber) }
        else { window.orderFrontRegardless() }
    }

    func close() {
        timer?.invalidate()
        timer = nil
        window.orderOut(nil)
    }

    /// Puts the morph at `p` (0…1) immediately — used while the fingers drive it.
    func set(_ p: CGFloat) {
        progress = max(0, min(1, p))
        let t = progress
        func mix(_ a: CGFloat, _ b: CGFloat) -> CGFloat { a + (b - a) * t }
        func ramp(_ a: CGFloat, _ b: CGFloat) -> CGFloat { max(0, min(1, (t - a) / (b - a))) }
        // The top edge travels up to the pill's top, so it reads as being pulled into the island.
        let w = mix(from.width, to.width), h = mix(from.height, to.height)
        let top = mix(from.maxY, to.maxY), midX = mix(from.midX, to.midX)
        let origin = window.frame.origin
        let r = NSRect(x: midX - w / 2 - origin.x, y: top - h - origin.y, width: w, height: h)
        let radius = min(mix(Self.panelRadius, to.height / 2), h / 2)

        CATransaction.begin()
        CATransaction.setDisableActions(true)
        shape.frame = r
        shape.maskImage = Self.mask(r.size, radius)
        // The pill copy brings its own glass, so the shape hands over to it at the end.
        shape.alphaValue = 1 - ramp(0.6, 0.95)
        let pw = IslandPillWindow.windowSize
        pillCopy.frame = NSRect(x: r.midX - pw.width / 2, y: r.maxY - IslandPillWindow.margin.height - IslandPillWindow.pillSize.height
                                    - IslandPillWindow.margin.height, width: pw.width, height: pw.height)
        pillCopy.alphaValue = ramp(0.55, 0.95)
        CATransaction.commit()
        // The real overlay (live, see-through) dissolves into the shape over the first part.
        panel?.alphaValue = 1 - ramp(0, 0.3)
    }

    /// Runs the morph from where it is to `target` (0 or 1), easing out, then calls `done`.
    func animate(to target: CGFloat, duration: TimeInterval, done: @escaping () -> Void) {
        timer?.invalidate()
        let start = progress, began = CACurrentMediaTime()
        let span = max(0.12, duration * Double(abs(target - start)))
        let timer = Timer(timeInterval: 1 / 120, repeats: true) { [weak self] timer in
            MainActor.assumeIsolated {
                guard let self else { timer.invalidate(); return }
                let x = min(1, (CACurrentMediaTime() - began) / span)
                let eased = 1 - pow(1 - x, 3)           // ease-out cubic
                self.set(start + (target - start) * CGFloat(eased))
                if x >= 1 { timer.invalidate(); self.timer = nil; done() }
            }
        }
        self.timer = timer
        RunLoop.main.add(timer, forMode: .common)
    }

    private static func mask(_ size: NSSize, _ radius: CGFloat) -> NSImage {
        NSImage(size: size, flipped: false) { rect in
            NSColor.black.setFill()
            NSBezierPath(roundedRect: rect, xRadius: radius, yRadius: radius).fill()
            return true
        }
    }
}
