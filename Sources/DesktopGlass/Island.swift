import AppKit
import ApplicationServices
import SwiftUI

// MARK: - Preferences

enum IslandPrefs {
    private static let d = UserDefaults.standard

    /// The pill at the top of the desktop.
    static var showIsland: Bool {
        get { d.object(forKey: "island.show") as? Bool ?? true }
        set { d.set(newValue, forKey: "island.show") }
    }
    /// ⌘D opens the island from any app (needs Accessibility).
    static var commandD: Bool {
        get { d.object(forKey: "island.commandD") as? Bool ?? true }
        set { d.set(newValue, forKey: "island.commandD") }
    }
    /// Two-finger swipe down at the top edge of the screen opens it (needs Accessibility, except over the pill).
    static var swipeDown: Bool {
        get { d.object(forKey: "island.swipe") as? Bool ?? true }
        set { d.set(newValue, forKey: "island.swipe") }
    }
    /// DesktopGlass mode: Finder stops drawing icons (files only show up in the island) and the custom wallpaper shows.
    static var hideDesktopIcons: Bool {
        get { d.object(forKey: "island.hideDesktopIcons") as? Bool ?? true }
        set { d.set(newValue, forKey: "island.hideDesktopIcons") }
    }
    static var size: IslandSize {
        get { IslandSize(rawValue: d.integer(forKey: "island.size")) ?? .standard }
        set { d.set(newValue.rawValue, forKey: "island.size") }
    }
}

// MARK: - Sizes (⌘= / ⌘-)

/// Standard → Medium → Large → Full screen. Steps that don't fit the screen fold into Full screen.
enum IslandSize: Int, CaseIterable {
    case standard, medium, large, full

    var label: String { ["Standard", "Medium", "Large", "Full Screen"][rawValue] }

    private var nominal: NSSize? {
        switch self {
        case .standard: return NSSize(width: 1180, height: 720)
        case .medium: return NSSize(width: 1400, height: 860)
        case .large: return NSSize(width: 1640, height: 1000)
        case .full: return nil
        }
    }

    /// The panel hangs from the top of the screen, centered under the island.
    func frame(on screen: NSScreen) -> NSRect {
        let visible = screen.visibleFrame
        let full = visible.insetBy(dx: 6, dy: 6)
        guard let size = fittingSize(on: screen) else { return full }
        return NSRect(x: visible.midX - size.width / 2, y: visible.maxY - 8 - size.height,
                      width: size.width, height: size.height)
    }

    /// nil when this step is (or doesn't fit, so becomes) full screen.
    private func fittingSize(on screen: NSScreen) -> NSSize? {
        guard let n = nominal else { return nil }
        let visible = screen.visibleFrame
        if n.width > visible.width - 60 || n.height > visible.height - 40 { return nil }
        return n
    }

    /// The next step that actually looks different on this screen.
    func stepped(_ delta: Int, on screen: NSScreen) -> IslandSize {
        var s = self
        let start = frame(on: screen)
        while let next = IslandSize(rawValue: s.rawValue + delta) {
            s = next
            if next.frame(on: screen) != start { return next }
        }
        return self
    }
}

// MARK: - The pill

/// The always-there island: a small glass pill at the top center of the desktop. It sits just above the desktop
/// layer, so app windows cover it and it shows whenever you're looking at the desktop.
final class IslandPillWindow: NSPanel {
    static let pillSize = NSSize(width: 322, height: 38)
    /// Transparent room around the pill so its hover grow and the interactive glass aren't clipped by the window.
    static let margin = NSSize(width: 16, height: 8)
    static var windowSize: NSSize {
        NSSize(width: pillSize.width + margin.width * 2, height: pillSize.height + margin.height * 2)
    }

    init(onOpen: @escaping () -> Void) {
        super.init(contentRect: NSRect(origin: .zero, size: Self.windowSize),
                   styleMask: [.nonactivatingPanel, .borderless, .fullSizeContentView],
                   backing: .buffered, defer: false)
        level = Self.desktopLevel
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false     // no hairline outline
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = true
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        let host = PillHostingView(rootView: IslandPillView(onOpen: onOpen, onWallpaper: {
            if let screen = IslandPillWindow.homeScreen { WallpaperPopup.shared.toggle(below: IslandPillWindow.pillRect(on: screen)) }
        }))
        host.onSwipeDown = onOpen
        contentView = host
        reposition()
    }

    override var canBecomeKey: Bool { false }

    /// A live, non-interactive copy of the pill (window-sized, pill centered) for the overlay ⇄ island morph.
    static func previewView() -> NSView {
        NSHostingView(rootView: IslandPillView(onOpen: {}, onWallpaper: {}))
    }

    /// Normally just above the desktop; raised above apps when the shortcut calls the island up off the desktop.
    static let desktopLevel = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.desktopIconWindow)) + 1)

    /// The screen with the menu bar.
    static var homeScreen: NSScreen? { NSScreen.screens.first ?? NSScreen.main }

    static func frame(on screen: NSScreen) -> NSRect {
        let v = screen.visibleFrame
        // The pill itself stays 6pt below the menu bar; the window extends past it by `margin`.
        return NSRect(x: v.midX - windowSize.width / 2, y: v.maxY - pillSize.height - 6 - margin.height,
                      width: windowSize.width, height: windowSize.height)
    }

    /// Where the visible pill sits (the window minus its margin) — what the panel morphs from and back into.
    static func pillRect(on screen: NSScreen) -> NSRect {
        frame(on: screen).insetBy(dx: margin.width, dy: margin.height)
    }

    func reposition() {
        if let screen = Self.homeScreen { setFrame(Self.frame(on: screen), display: true) }
    }
}

/// Lets a two-finger swipe down over the pill open the island without any permission.
private final class PillHostingView<Content: View>: NSHostingView<Content> {
    var onSwipeDown: () -> Void = {}
    private var swipe = SwipeTracker()

    override func scrollWheel(with event: NSEvent) {
        if swipe.feed(event, threshold: 18) == .down { onSwipeDown() }
    }
}

private struct IslandPillView: View {
    let onOpen: () -> Void
    let onWallpaper: () -> Void
    @ObservedObject private var reindexer = IslandStatus.shared
    @AppStorage("island.hideDesktopIcons") private var hideDesktopIcons = true
    @State private var hover = false
    /// On hover everything but the "Desktop" label nudges down together, the switch included.
    private var nudge: CGFloat { hover ? 2 : 0 }

    var body: some View {
        HStack(spacing: 0) {
            Button(action: onOpen) {
                HStack(spacing: 9) {
                    Image(systemName: "square.grid.3x3.square.fill").font(.system(size: 14, weight: .semibold))
                        .offset(y: nudge)
                    Text("Desktop").font(.system(size: 13, weight: .semibold))   // stays put on hover
                    if reindexer.busy { ProgressView().controlSize(.mini) }
                    Spacer(minLength: 4)
                    Group {
                        Text(IslandPrefs.commandD ? "⌘D" : HotKeyChoice.current.rawValue)
                            .font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
                        Image(systemName: "chevron.down").font(.system(size: 10, weight: .bold)).foregroundStyle(.secondary)
                    }
                    .offset(y: nudge)
                }
                .padding(.leading, 16).padding(.trailing, 8)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Button(action: onWallpaper) {
                Image(systemName: "photo.on.rectangle.angled").font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .frame(width: 24, height: 24)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .offset(y: nudge)
            .help("Wallpaper")
            .padding(.trailing, 6)
            // DesktopGlass mode: Finder's icons hidden + the custom wallpaper shown.
            Toggle("", isOn: $hideDesktopIcons)
                .labelsHidden().toggleStyle(.switch).controlSize(.mini)
                .offset(y: nudge)
                .help(hideDesktopIcons ? "DesktopGlass mode is on — turn off to bring back desktop icons, your normal wallpaper and your Dock setting"
                                       : "Turn on DesktopGlass mode: hide desktop icons, show your DesktopGlass wallpaper and auto-hide the Dock")
                .padding(.trailing, 12)
        }
        .contentShape(Capsule())
        .contextMenu {
            Toggle("DesktopGlass Mode", isOn: $hideDesktopIcons)
            Button("Wallpaper…", action: onWallpaper)
        }
        .glassPill()
        .scaleEffect(hover ? 1.03 : 1)
        .animation(.spring(response: 0.25, dampingFraction: 0.7), value: hover)
        .onHover { hover = $0 }
        .help("Open DesktopGlass — click, swipe down, or ⌘D")
        .frame(width: IslandPillWindow.pillSize.width, height: IslandPillWindow.pillSize.height)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// Small bits of state the pill shows.
@MainActor
final class IslandStatus: ObservableObject {
    static let shared = IslandStatus()
    @Published var busy = false
}

// MARK: - Swipe detection

/// Turns trackpad scroll events into "fingers moved down / up" swipes, once per gesture.
struct SwipeTracker {
    enum Direction { case down, up }
    private var total: CGFloat = 0
    private var fired = false

    /// Feed every scroll event; returns a direction once per gesture when the fingers travel past `threshold`.
    mutating func feed(_ e: NSEvent, threshold: CGFloat = 28) -> Direction? {
        guard e.hasPreciseScrollingDeltas, e.momentumPhase.isEmpty else { return nil }   // trackpad, fingers down
        if e.phase.contains(.began) || e.phase.contains(.mayBegin) { total = 0; fired = false }
        if e.phase.contains(.ended) || e.phase.contains(.cancelled) { total = 0; fired = false; return nil }
        guard !fired else { return nil }
        // With natural scrolling, fingers moving down give a positive delta; otherwise negative.
        let fingersDown = e.isDirectionInvertedFromDevice ? e.scrollingDeltaY : -e.scrollingDeltaY
        guard abs(e.scrollingDeltaY) > abs(e.scrollingDeltaX) * 0.8 else { return nil }   // mostly vertical
        total += fingersDown
        if total > threshold { fired = true; return .down }
        if total < -threshold { fired = true; return .up }
        return nil
    }
}

// MARK: - System-wide input (⌘D anywhere, swipe at the top edge)

/// One Accessibility event tap for the island's shortcut and gesture. Needs Accessibility permission; without it
/// the configurable hotkey, clicking the pill and swiping over the pill still work.
@MainActor
final class DesktopInput {
    /// Return true to swallow the key.
    var onKeyDown: (_ keyCode: Int64, _ flags: CGEventFlags) -> Bool = { _, _ in false }
    var onScroll: (_ event: NSEvent) -> Void = { _ in }

    private var tap: CFMachPort?
    private var source: CFRunLoopSource?
    private var retry: Timer?

    static var isTrusted: Bool { AXIsProcessTrusted() }

    /// Shows the system prompt that sends the user to Privacy & Security → Accessibility.
    static func requestAccess() {
        let key = kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String
        _ = AXIsProcessTrustedWithOptions([key: true] as CFDictionary)
    }

    static func openAccessibilitySettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(url)
        }
    }

    var isRunning: Bool { tap != nil }

    /// Starts the tap now, or as soon as permission is granted.
    func start() {
        guard tap == nil else { return }
        if install() { retry?.invalidate(); retry = nil; return }
        guard retry == nil else { return }
        retry = Timer.scheduledTimer(withTimeInterval: 3, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.install() else { return }
                self.retry?.invalidate()
                self.retry = nil
            }
        }
    }

    func stop() {
        retry?.invalidate(); retry = nil
        if let tap { CGEvent.tapEnable(tap: tap, enable: false) }
        if let source { CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes) }
        tap = nil; source = nil
    }

    private func install() -> Bool {
        guard Self.isTrusted else { return false }
        let mask = (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.scrollWheel.rawValue)
        let me = Unmanaged.passUnretained(self).toOpaque()
        guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap,
                                          eventsOfInterest: CGEventMask(mask), callback: desktopInputCallback,
                                          userInfo: me) else { return false }
        let source = CFMachPortCreateRunLoopSource(nil, tap, 0)
        CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)   // callbacks arrive on the main thread
        CGEvent.tapEnable(tap: tap, enable: true)
        self.tap = tap
        self.source = source
        return true
    }

    fileprivate func handle(_ type: CGEventType, _ event: CGEvent) -> Bool {
        switch type {
        case .tapDisabledByTimeout, .tapDisabledByUserInput:
            if let tap { CGEvent.tapEnable(tap: tap, enable: true) }
            return false
        case .keyDown:
            guard event.getIntegerValueField(.keyboardEventAutorepeat) == 0 else { return false }
            return onKeyDown(event.getIntegerValueField(.keyboardEventKeycode), event.flags)
        case .scrollWheel:
            if let e = NSEvent(cgEvent: event) { onScroll(e) }
            return false
        default:
            return false
        }
    }
}

private func desktopInputCallback(proxy: CGEventTapProxy, type: CGEventType, event: CGEvent,
                                  refcon: UnsafeMutableRawPointer?) -> Unmanaged<CGEvent>? {
    guard let refcon else { return Unmanaged.passUnretained(event) }
    let input = Unmanaged<DesktopInput>.fromOpaque(refcon).takeUnretainedValue()
    let swallow = MainActor.assumeIsolated { input.handle(type, event) }
    return swallow ? nil : Unmanaged.passUnretained(event)
}

// MARK: - Dock auto-hide

/// In DesktopGlass mode the Dock hides and slides up only when the pointer reaches the bottom edge. The user's own
/// setting is remembered when the mode turns on and put back when it turns off.
enum DockAutoHide {
    private static let savedKey = "dock.autohideBeforeGlassMode"

    static func apply(glassMode: Bool) {
        DispatchQueue.global(qos: .utility).async {
            let d = UserDefaults.standard
            let current = isOn
            let want: Bool
            if glassMode {
                if d.object(forKey: savedKey) == nil { d.set(current, forKey: savedKey) }
                want = true
            } else {
                guard let before = d.object(forKey: savedKey) as? Bool else { return }   // we never changed it
                d.removeObject(forKey: savedKey)
                want = before
            }
            guard want != current else { return }
            _ = Shell.run("/usr/bin/defaults", ["write", "com.apple.dock", "autohide", "-bool", want ? "true" : "false"])
            _ = Shell.run("/usr/bin/killall", ["Dock"])
        }
    }

    static var isOn: Bool {
        let out = Shell.run("/usr/bin/defaults", ["read", "com.apple.dock", "autohide"]).lowercased()
        return out.hasPrefix("1") || out.hasPrefix("true") || out.hasPrefix("yes")
    }
}

// MARK: - Finder desktop icons

enum FinderDesktopIcons {
    /// Whether Finder currently draws the desktop (icons). Missing key = the default, on.
    static var finderShowsIcons: Bool {
        let out = run("/usr/bin/defaults", ["read", "com.apple.finder", "CreateDesktop"]).lowercased()
        return !(out.hasPrefix("0") || out.hasPrefix("false") || out.hasPrefix("no"))
    }

    /// Applies the preference, restarting Finder only when it actually changes.
    static func apply(hidden: Bool) {
        DispatchQueue.global(qos: .utility).async {
            guard finderShowsIcons == hidden else { return }
            _ = run("/usr/bin/defaults", ["write", "com.apple.finder", "CreateDesktop", "-bool", hidden ? "false" : "true"])
            _ = run("/usr/bin/killall", ["Finder"])
        }
    }

    @discardableResult
    private static func run(_ tool: String, _ args: [String]) -> String { Shell.run(tool, args) }
}

enum Shell {
    /// Runs a tool and returns its trimmed stdout ("" if it couldn't start).
    @discardableResult
    static func run(_ tool: String, _ args: [String]) -> String {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: tool)
        p.arguments = args
        let pipe = Pipe()
        p.standardOutput = pipe
        p.standardError = FileHandle.nullDevice
        do { try p.run() } catch { return "" }
        p.waitUntilExit()
        return String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
    }
}
