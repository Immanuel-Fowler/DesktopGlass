import AppKit
import Carbon
import Combine
import ServiceManagement
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    private var statusItem: NSStatusItem!
    private var panel: OverlayPanel!
    private var hotKey: HotKey?
    private var registeredHotKey: HotKeyChoice?
    private let store = DesktopStore()
    private let reindexer = Reindexer()
    private let detector = HarnessDetector()
    private lazy var editor = EditorWindowController(detector: detector)
    private let artifacts = ArtifactBrowserController.shared
    private var control: AppControl?
    /// The Rust engine (`dg engine`): owns the control channel and shared settings.
    let engine = EngineClient.shared
    private var isMorphing = false
    private let overlayTerminal = TerminalSession()
    private var lastEditorFrame: NSRect?
    // The island: an always-there pill on the desktop that drops down into the overlay panel.
    private var pill: IslandPillWindow!
    private let input = DesktopInput()
    private var topSwipe = SwipeTracker()
    private var islandSize = IslandPrefs.size
    private var appliedIslandPrefs: [Bool] = []
    private var reindexWatch: AnyCancellable?

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem.button?.image = NSImage(systemSymbolName: "square.grid.3x3.square", accessibilityDescription: "DesktopGlass")
        let menu = NSMenu()
        menu.delegate = self
        statusItem.menu = menu

        detector.detect()
        if !FileManager.default.fileExists(atPath: IndexPreferences.fileURL.path) { IndexPreferences.shared.saveNow() }
        NSApp.mainMenu = buildMainMenu()
        installKeyMonitor()
        EditorRouter.handler = { [weak self] url in self?.openEditor(url) }
        editor.onBackToDesktop = { [weak self] in self?.morphBackToDesktop() }
        editor.onClosed = { [weak self] in self?.editorClosed() }
        ArtifactRouter.handler = { [weak self] url, title in self?.artifacts.open(url, title: title) }
        artifacts.onVisibilityChange = { [weak self] in self?.updateActivationPolicy() }
        // With the editor open, pages open as tabs next to the code instead of in a separate window.
        artifacts.preferEditor = { [weak self] in self?.editor.window.isVisible == true }
        artifacts.showInEditor = { [weak self] tab in
            guard let self else { return }
            self.editor.model.activeWeb = tab
            self.editor.window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
        artifacts.openMarkdownInEditor = { [weak self] url in
            guard let self else { return false }
            self.editor.model.openFile(url)
            guard let doc = self.editor.model.document(at: url) else { return false }
            doc.setPreview(true)
            self.editor.window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return true
        }
        CodexVisualizations.shared.projectRoots = { [weak self] in
            guard let self, self.editor.window.isVisible, let root = self.editor.model.root?.url else { return [] }
            return [root]
        }
        CodexVisualizations.shared.start()
        ArtifactAccounts.shared.refresh()
        artifacts.onTabClosed = { [weak self] tab in
            guard let self, self.editor.model.activeWeb === tab else { return }
            self.editor.model.activeWeb = nil
        }
        let control = AppControl(artifacts: artifacts, reindexer: reindexer, store: store,
                                 openEditor: { [weak self] url in self?.openEditor(url) },
                                 showOverlay: { [weak self] in self?.showPanel() })
        control.documents = { [weak self] in self?.editor.model.documents ?? [] }
        self.control = control
        // The engine owns the control channel; calls it can't answer itself (browser, open editor buffers, windows)
        // come back here and go to AppControl.
        engine.controlHandler = { method, params in try await control.handle(method, params) }
        engine.onReady = { [weak self] port in self?.artifacts.excludedPorts = [port] }
        engine.start()
        panel = OverlayPanel(rootView: OverlayView(store: store, reindexer: reindexer, detector: detector,
                                                   terminal: overlayTerminal) { [weak self] in self?.hidePanel() })
        panel.onCancel = { [weak self] in self?.hidePanel() }
        pill = IslandPillWindow { [weak self] in self?.showPanel() }
        reindexWatch = reindexer.$running.sink { IslandStatus.shared.busy = $0 }
        input.onKeyDown = { [weak self] code, flags in self?.handleGlobalKey(code, flags) ?? false }
        input.onScroll = { [weak self] event in self?.handleGlobalScroll(event) }
        NotificationCenter.default.addObserver(self, selector: #selector(screensChanged(_:)),
                                               name: NSApplication.didChangeScreenParametersNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(panelResignedKey(_:)),
                                               name: NSWindow.didResignKeyNotification, object: panel)

        registerHotKey()
        applyIslandPrefs()
        if IslandPrefs.commandD || IslandPrefs.swipeDown, !DesktopInput.isTrusted { DesktopInput.requestAccess() }
        GameLibrary.shared.onLaunched = { [weak self] in
            guard let self, self.panel.isVisible else { return }
            self.hidePanel()
        }
        let pad = GamepadNavigator.shared
        pad.isActive = { [weak self] in
            guard let self else { return false }
            return self.panel.isVisible && !self.store.showingSettings && !(self.panel.firstResponder is GlassTerminalView)
        }
        pad.move = { [weak self] dx, dy in self?.leaveSearchField(); self?.store.moveSelection(dx: dx, dy: dy) }
        pad.activate = { [weak self] in self?.store.activateSelection() }
        pad.back = { [weak self] in
            guard let self else { return }
            if !self.store.query.isEmpty { self.store.query = ""; self.leaveSearchField() } else { self.store.goBack() }
        }
        pad.search = { [weak self] in self?.focusSearch() }
        pad.start()
        NotificationCenter.default.addObserver(self, selector: #selector(defaultsChanged(_:)),
                                               name: UserDefaults.didChangeNotification, object: nil)

        reindexer.onFinish = { [weak self] in self?.store.reload() }
        // The engine runs the re-index schedule (and its launch catch-up) itself.
        store.reload()
    }

    /// desktopglass://open?url=<file path or URL>&title=<optional>  ·  desktopglass://edit?path=<file or folder>
    func application(_ application: NSApplication, open urls: [URL]) {
        for link in urls where link.scheme == "desktopglass" {
            let items = URLComponents(url: link, resolvingAgainstBaseURL: false)?.queryItems ?? []
            func value(_ name: String) -> String? { items.first { $0.name == name }?.value }
            switch link.host {
            case "edit":
                if let raw = value("path") ?? value("url") {
                    Task { if let target = await ArtifactPolicy.resolve(raw), target.isFileURL { openEditor(target) } }
                }
            default:   // "open", "artifact", "view"
                let title = value("title")
                if let raw = value("url") ?? value("path") {
                    Task {   // the engine resolves paths and URLs
                        if let target = await ArtifactPolicy.resolve(raw) {
                            artifacts.open(target, title: title, explicit: true)   // dgview / a link: any web page
                        } else {
                            artifacts.showWindow()
                        }
                    }
                } else {
                    artifacts.showWindow()
                }
            }
        }
    }

    /// In the Dock and ⌘-Tab only while a real window (editor or artifact viewer) is open.
    private func updateActivationPolicy() {
        let needsDock = editor.window.isVisible || artifacts.window.isVisible
        NSApp.setActivationPolicy(needsDock ? .regular : .accessory)
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard editor.model.confirmCloseAll() else { return .terminateCancel }
        engine.stop()
        return .terminateNow
    }

    // MARK: Panel

    @objc func togglePanel() {
        guard !isMorphing else { return }
        if panel.isVisible { hidePanel() } else { showPanel() }
    }

    /// The shortcut (⌘D or the chosen hotkey). Off the desktop the first press only calls the island up above
    /// your apps; pressing it again (or clicking the island) opens the overlay.
    private func shortcutPressed() {
        guard !isMorphing else { return }
        if panel.isVisible { hidePanel(); return }
        let islandOnScreen = pill.isVisible && pill.occlusionState.contains(.visible)
        if IslandPrefs.showIsland, !islandOnScreen, !pillRaised { raisePill() } else { showPanel() }
    }

    // MARK: Island called up over other apps

    private var pillRaised = false
    private var pillDismissMonitors: [Any] = []
    private var pillDismissObserver: NSObjectProtocol?

    private func raisePill() {
        pillRaised = true
        pill.reposition()
        let home = pill.frame
        pill.level = .floating
        pill.alphaValue = 0
        pill.setFrame(home.offsetBy(dx: 0, dy: 14), display: false)
        pill.orderFrontRegardless()
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.22
            ctx.timingFunction = Self.morphTiming
            pill.animator().alphaValue = 1
            pill.animator().setFrame(home, display: true)
        }
        // A click elsewhere, Esc or switching apps puts it back on the desktop.
        if let m = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown], handler: { [weak self] _ in
            MainActor.assumeIsolated { self?.lowerPill(animated: true) }
        }) { pillDismissMonitors.append(m) }
        if let m = NSEvent.addGlobalMonitorForEvents(matching: .keyDown, handler: { [weak self] e in
            if e.keyCode == 53 { MainActor.assumeIsolated { self?.lowerPill(animated: true) } }
        }) { pillDismissMonitors.append(m) }
        pillDismissObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.lowerPill(animated: true) }
        }
    }

    /// Back to its desktop level (where apps cover it). Not animated when the overlay is taking over from it.
    private func lowerPill(animated: Bool) {
        guard pillRaised else { return }
        pillRaised = false
        pillDismissMonitors.forEach(NSEvent.removeMonitor)
        pillDismissMonitors = []
        if let o = pillDismissObserver { NSWorkspace.shared.notificationCenter.removeObserver(o) }
        pillDismissObserver = nil
        guard animated, pill.isVisible else { pill.level = IslandPillWindow.desktopLevel; return }
        NSAnimationContext.runAnimationGroup({ ctx in
            ctx.duration = 0.16
            pill.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self, !self.pillRaised else { return }
                self.pill.level = IslandPillWindow.desktopLevel
                if !self.panel.isVisible, IslandPrefs.showIsland { self.pill.alphaValue = 1; self.pill.orderFrontRegardless() }
            }
        })
    }

    /// The island grows into the overlay, which hangs from the top of the screen at the current size.
    private func showPanel() {
        if panel.isVisible { if !isMorphing { panel.makeKeyAndOrderFront(nil) }; return }
        lowerPill(animated: false)
        store.reload()
        store.keyboardActive = false
        let screen = IslandPillWindow.homeScreen ?? NSScreen.main!
        let target = islandSize.frame(on: screen)
        isMorphing = true
        pill.reposition()
        panel.alphaValue = 0
        panel.setFrame(target, display: true)
        panel.makeKeyAndOrderFront(nil)
        let morph = IslandMorph(panel: panel, pillRect: IslandPillWindow.pillRect(on: screen))
        self.morph = morph
        morph.show(at: 1)
        pill.orderOut(nil)
        morph.animate(to: 0, duration: 0.36) { [weak self] in
            guard let self else { return }
            self.panel.alphaValue = 1
            morph.close()
            if self.morph === morph { self.morph = nil }
            self.panel.makeKey()
            self.isMorphing = false
        }
        leaveSearchField()          // open in browse mode so arrows/WASD work; typing a letter starts a search
    }

    /// ⌘= / ⌘-: Standard → Medium → Large → Full screen, and back.
    private func stepIslandSize(_ delta: Int) {
        guard panel.isVisible, !isMorphing, let screen = panel.screen ?? IslandPillWindow.homeScreen else { return }
        let next = islandSize.stepped(delta, on: screen)
        guard next != islandSize else {
            // ⌘- past the smallest size closes the island; ⌘= past full screen just beeps.
            if delta < 0 { hidePanel() } else { NSSound.beep() }
            return
        }
        islandSize = next
        IslandPrefs.size = next
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.28
            ctx.timingFunction = Self.morphTiming
            panel.animator().setFrame(next.frame(on: screen), display: true)
        }
    }

    // MARK: Island input (⌘D anywhere, two-finger swipe at the top edge)

    private func handleGlobalKey(_ code: Int64, _ flags: CGEventFlags) -> Bool {
        let mods = flags.intersection([.maskCommand, .maskAlternate, .maskControl, .maskShift])
        guard IslandPrefs.commandD, code == Int64(kVK_ANSI_D), mods == .maskCommand, !isMorphing else { return false }
        if panel.isVisible && panel.isKeyWindow {
            if panel.firstResponder is GlassTerminalView { return false }   // let the shell have it
            hidePanel()
            return true
        }
        shortcutPressed()
        return true
    }

    private func handleGlobalScroll(_ event: NSEvent) {
        guard IslandPrefs.swipeDown else { return }
        let mouse = NSEvent.mouseLocation
        guard let screen = NSScreen.screens.first(where: { NSMouseInRect(mouse, $0.frame, false) }) else { return }
        let menuBar = screen.frame.maxY - screen.visibleFrame.maxY
        // The menu bar strip, plus the pill below it while the island is folded up.
        let inZone = mouse.y >= screen.frame.maxY - max(menuBar, 4) - (panel.isVisible ? 6 : 56)
        guard inZone else { if event.phase.contains(.began) { topSwipe = SwipeTracker() }; return }
        switch topSwipe.feed(event) {
        case .down? where !panel.isVisible: showPanel()
        case .up? where panel.isVisible && !isMorphing: hidePanel()
        default: break
        }
    }

    @objc private func screensChanged(_ note: Notification) {
        pill.reposition()
        if panel.isVisible, !isMorphing, let screen = panel.screen { panel.setFrame(islandSize.frame(on: screen), display: true) }
    }

    /// Re-applies island settings when they change (UserDefaults notifications fire for every write, so diff first).
    private func applyIslandPrefs() {
        let now = [IslandPrefs.showIsland, IslandPrefs.commandD, IslandPrefs.swipeDown, IslandPrefs.hideDesktopIcons]
        guard now != appliedIslandPrefs else { return }
        let first = appliedIslandPrefs.isEmpty
        let iconsChanged = first || appliedIslandPrefs[3] != now[3]
        appliedIslandPrefs = now
        if IslandPrefs.showIsland, !panel.isVisible { pill.reposition(); pill.orderFrontRegardless() }
        if !IslandPrefs.showIsland { pill.orderOut(nil) }
        if IslandPrefs.commandD || IslandPrefs.swipeDown { input.start() } else { input.stop() }
        if iconsChanged {
            FinderDesktopIcons.apply(hidden: IslandPrefs.hideDesktopIcons)
            DockAutoHide.apply(glassMode: IslandPrefs.hideDesktopIcons)
        }
        WallpaperController.shared.refresh()
    }

    // MARK: Overlay keyboard

    private var keyMonitor: Any?

    private var pinchMonitor: Any?
    private var pinchTotal: CGFloat = 0
    private var pinchFired = false

    private var headerSwipeMonitor: Any?
    /// Fingers-up distance of a two-finger swipe that started on the search bar; nil when no such swipe is live.
    private var headerSwipe: CGFloat?
    /// The search bar strip at the top of the overlay.
    private static let headerHeight: CGFloat = 62
    /// Fingers travel (points) that folds the window all the way into the island.
    private static let headerSwipeTravel: CGFloat = 220

    private func installKeyMonitor() {
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            return self.handleOverlayKey(event)
        }
        pinchMonitor = NSEvent.addLocalMonitorForEvents(matching: .magnify) { [weak self] event in
            guard let self else { return event }
            return self.handleOverlayPinch(event)
        }
        headerSwipeMonitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
            guard let self else { return event }
            return self.handleHeaderSwipe(event)
        }
    }

    /// Two fingers up on the search bar: the window morphs toward the island as the fingers move, and folds into it
    /// on release once it's past a third of the way (or flicked up); otherwise it springs back open.
    private func handleHeaderSwipe(_ e: NSEvent) -> NSEvent? {
        guard panel.isVisible, e.window === panel || headerSwipe != nil, e.hasPreciseScrollingDeltas else { return e }
        if !e.momentumPhase.isEmpty { return headerSwipe == nil && !isMorphing ? e : nil }   // swallow the coast after a fold
        if e.phase.contains(.began) {
            headerSwipe = nil
            guard !isMorphing, let content = panel.contentView else { return e }
            guard e.locationInWindow.y >= content.bounds.height - Self.headerHeight else { return e }
            headerSwipe = 0
            return nil
        }
        guard var travel = headerSwipe else { return e }
        if e.phase.contains(.ended) || e.phase.contains(.cancelled) {
            headerSwipe = nil
            guard let morph else { return nil }          // never moved
            let progress = travel / Self.headerSwipeTravel
            if progress > 0.33 || (progress > 0.08 && e.phase.contains(.ended) && lastHeaderVelocity > 9) {
                isMorphing = true
                morph.animate(to: 1, duration: 0.3) { [weak self] in self?.finishFold(morph) }
            } else {
                morph.animate(to: 0, duration: 0.25) { [weak self] in
                    guard let self else { return }
                    self.panel.alphaValue = 1
                    morph.close()
                    if self.morph === morph { self.morph = nil }
                }
            }
            return nil
        }
        // With natural scrolling, fingers moving up give a negative delta; otherwise positive.
        let fingersUp = e.isDirectionInvertedFromDevice ? -e.scrollingDeltaY : e.scrollingDeltaY
        lastHeaderVelocity = fingersUp
        travel = max(0, travel + fingersUp)
        headerSwipe = travel
        guard travel > 2 || morph != nil else { return nil }
        let fold = morph ?? beginFold()
        let p = min(1, travel / Self.headerSwipeTravel)
        fold.set(1 - pow(1 - p, 2))
        return nil
    }

    private var lastHeaderVelocity: CGFloat = 0

    /// Pinching in (fingers together) anywhere over the overlay folds it back into the island, once per gesture.
    private func handleOverlayPinch(_ e: NSEvent) -> NSEvent? {
        guard panel.isVisible, e.window === panel else { return e }
        if e.phase.contains(.began) { pinchTotal = 0; pinchFired = false }
        if e.phase.contains(.ended) || e.phase.contains(.cancelled) { pinchTotal = 0; pinchFired = false; return e }
        guard !pinchFired else { return nil }
        pinchTotal += e.magnification
        if pinchTotal < -0.18, !isMorphing {
            pinchFired = true
            hidePanel()
            return nil
        }
        return e
    }

    private func leaveSearchField() {
        DispatchQueue.main.async { self.panel.makeFirstResponder(nil) }
    }

    private func focusSearch() {
        store.focusToken += 1
        // the field selects all its text when focused; put the caret at the end so typing continues the query
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) {
            if let tv = self.panel.firstResponder as? NSTextView, tv.isFieldEditor {
                tv.setSelectedRange(NSRange(location: (tv.string as NSString).length, length: 0))
            }
        }
    }

    /// ↑↓←→ / WASD move · Return opens (⌥ editor, ⌘ Finder) · Tab or Delete goes back · / or ⌘F searches.
    private func handleOverlayKey(_ e: NSEvent) -> NSEvent? {
        guard panel.isVisible, e.window === panel else { return e }
        // ⌘= / ⌘- resize the island from anywhere in it (⌘+ arrives as ⌘= with or without shift).
        let sizeMods = e.modifierFlags.intersection([.command, .control, .option])
        if sizeMods == .command, let c = e.charactersIgnoringModifiers {
            if c == "=" || c == "+" { stepIslandSize(1); return nil }
            if c == "-" || c == "_" { stepIslandSize(-1); return nil }
        }
        if sizeMods == .control, e.keyCode == 50 { store.showTerminal.toggle(); return nil }      // ⌃`
        guard !store.showingSettings else { return e }
        let responder = panel.firstResponder
        if responder is GlassTerminalView { return e }               // the terminal gets every key
        // Other text fields (the Apps search) keep their keys; only the main search field gets the shortcuts below.
        if let tv = responder as? NSTextView, tv.isFieldEditor,
           (tv.delegate as? NSTextField)?.placeholderString != OverlayView.searchPlaceholder { return e }
        let mods = e.modifierFlags.intersection([.command, .control, .option])
        let chars = e.charactersIgnoringModifiers?.lowercased() ?? ""
        let isReturn = e.keyCode == 36 || e.keyCode == 76

        // Typing in the search field
        if let tv = responder as? NSTextView, tv.isFieldEditor {
            switch e.keyCode {
            case 125:                                                    // ↓
                if store.isSearching { store.keyboardActive = true; store.moveSelection(dx: 0, dy: 1) }
                else { leaveSearchField(); store.moveSelection(dx: 0, dy: 0) }
                return nil
            case 126:                                                    // ↑
                if store.isSearching { store.keyboardActive = true; store.moveSelection(dx: 0, dy: -1); return nil }
                return e
            case 36, 76:
                if store.isSearching { store.activateSelection(); leaveSearchField() }
                return nil
            case 48:                                                     // Tab: clear, back to browsing
                store.query = ""
                leaveSearchField()
                return nil
            case 53:                                                     // Esc: clear first, close second
                if !store.query.isEmpty { store.query = ""; leaveSearchField(); return nil }
                return e
            default:
                return e
            }
        }

        if mods.contains(.command) || mods.contains(.control) {
            if mods.contains(.command), isReturn, let item = store.selectedItem { Actions.reveal(item); return nil }
            if mods.contains(.command), chars == "f" { focusSearch(); return nil }
            return e
        }

        switch e.keyCode {
        case 126: store.moveSelection(dx: 0, dy: -1); return nil
        case 125: store.moveSelection(dx: 0, dy: 1); return nil
        case 123: store.moveSelection(dx: -1, dy: 0); return nil
        case 124: store.moveSelection(dx: 1, dy: 0); return nil
        case 36, 76:
            if mods.contains(.option), let item = store.selectedItem {
                if item.canEditHere { Actions.editHere(item) } else { Actions.open(item) }
            } else {
                store.activateSelection()
            }
            return nil
        case 48, 51: store.goBack(); return nil                         // Tab, Delete
        case 53: return e                                               // Esc closes the overlay
        default: break
        }
        switch chars {
        case "w": store.moveSelection(dx: 0, dy: -1); return nil
        case "a": store.moveSelection(dx: -1, dy: 0); return nil
        case "s": store.moveSelection(dx: 0, dy: 1); return nil
        case "d": store.moveSelection(dx: 1, dy: 0); return nil
        case "/": focusSearch(); return nil
        default:
            // any other letter or number starts a search with it
            if let typed = e.characters, typed.count == 1, let scalar = typed.unicodeScalars.first,
               CharacterSet.alphanumerics.contains(scalar) || "._-".contains(typed) {
                store.query += typed
                focusSearch()
                return nil
            }
            return e
        }
    }

    /// The panel folds back up into the pill.
    private func hidePanel() {
        guard panel.isVisible, !isMorphing else { return }
        isMorphing = true
        let morph = self.morph ?? beginFold()
        morph.animate(to: 1, duration: 0.34) { [weak self] in self?.finishFold(morph) }
    }

    private var morph: IslandMorph?

    /// Swaps the overlay for the morph at the overlay's spot (progress 0).
    private func beginFold() -> IslandMorph {
        let screen = panel.screen ?? IslandPillWindow.homeScreen ?? NSScreen.main!
        pill.reposition()
        let morph = IslandMorph(panel: panel, pillRect: IslandPillWindow.pillRect(on: screen))
        self.morph = morph
        morph.show(at: 0)
        return morph
    }

    private func finishFold(_ morph: IslandMorph) {
        if IslandPrefs.showIsland { pill.alphaValue = 1; pill.orderFrontRegardless() }
        morph.close()
        if self.morph === morph { self.morph = nil }
        panel.orderOut(nil)
        panel.alphaValue = 1
        isMorphing = false
    }

    @objc private func panelResignedKey(_ note: Notification) { if !isMorphing { hidePanel() } }

    // MARK: Editor morph

    private static let morphTiming = CAMediaTimingFunction(controlPoints: 0.2, 0.9, 0.25, 1)

    private func defaultEditorFrame(on screen: NSScreen?) -> NSRect {
        if let last = lastEditorFrame, screen?.frame.intersects(last) ?? true { return last }
        let visible = (screen ?? NSScreen.main)?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        return visible.insetBy(dx: visible.width * 0.06, dy: visible.height * 0.06)
    }

    /// The overlay grows into the editor window: both animate to the editor's frame while they cross-fade.
    private func openEditor(_ url: URL) {
        guard editor.model.confirmSwitch(to: url) else { return }
        editor.model.open(url)
        let window = editor.window
        let overlayVisible = panel.isVisible
        NSApp.setActivationPolicy(.regular)

        guard overlayVisible else {
            if !window.isVisible { window.setFrame(defaultEditorFrame(on: NSScreen.main), display: false) }
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        let start = panel.frame
        let target = window.isVisible ? window.frame : defaultEditorFrame(on: panel.screen)
        isMorphing = true
        if !window.isVisible {
            window.setFrame(start, display: false)
            window.alphaValue = 0
        }
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        panel.orderFront(nil)   // keep the overlay above while it morphs away

        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.42
            ctx.timingFunction = Self.morphTiming
            ctx.allowsImplicitAnimation = true
            window.animator().setFrame(target, display: true)
            window.animator().alphaValue = 1
            panel.animator().setFrame(target, display: true)
            panel.animator().alphaValue = 0
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.46) {
            self.panel.orderOut(nil)
            self.panel.alphaValue = 1
            self.isMorphing = false
            self.editor.window.makeKey()
            if IslandPrefs.showIsland { self.pill.reposition(); self.pill.orderFrontRegardless() }
        }
    }

    /// The editor shrinks back into the overlay. Open tabs are kept for next time.
    private func morphBackToDesktop() {
        let window = editor.window
        lastEditorFrame = window.frame
        let end = islandSize.frame(on: window.screen ?? IslandPillWindow.homeScreen ?? NSScreen.main!)
        isMorphing = true
        pill.orderOut(nil)
        store.showingSettings = false
        store.reload()
        panel.setFrame(window.frame, display: false)
        panel.alphaValue = 0
        panel.makeKeyAndOrderFront(nil)

        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.38
            ctx.timingFunction = Self.morphTiming
            ctx.allowsImplicitAnimation = true
            panel.animator().setFrame(end, display: true)
            panel.animator().alphaValue = 1
            window.animator().setFrame(end, display: true)
            window.animator().alphaValue = 0
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.42) {
            window.orderOut(nil)
            window.alphaValue = 1
            if let frame = self.lastEditorFrame { window.setFrame(frame, display: false) }
            self.updateActivationPolicy()
            self.panel.makeKeyAndOrderFront(nil)
            self.leaveSearchField()
            // let the activation-policy switch settle before resign-key can hide the overlay again
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { self.isMorphing = false }
        }
    }

    private func editorClosed() {
        lastEditorFrame = editor.window.frame
        for tab in artifacts.editorTabs { artifacts.close(tab) }     // their home is gone
        if !isMorphing { DispatchQueue.main.async { self.updateActivationPolicy() } }
    }

    // MARK: Main menu (active while the editor is frontmost)

    private func buildMainMenu() -> NSMenu {
        let main = NSMenu()
        func menu(_ title: String, _ items: [NSMenuItem]) {
            let top = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            let sub = NSMenu(title: title)
            for i in items { sub.addItem(i) }
            top.submenu = sub
            main.addItem(top)
        }
        func item(_ title: String, _ action: Selector?, _ key: String, _ mods: NSEvent.ModifierFlags = [.command], target: AnyObject? = nil, tag: Int = 0) -> NSMenuItem {
            let i = NSMenuItem(title: title, action: action, keyEquivalent: key)
            i.keyEquivalentModifierMask = mods
            i.target = target
            i.tag = tag
            return i
        }
        let find = #selector(NSResponder.performTextFinderAction(_:))
        menu("DesktopGlass", [
            item("Settings…", #selector(openSettings), ",", target: self),
            .separator(),
            item("Hide DesktopGlass", #selector(NSApplication.hide(_:)), "h"),
            item("Quit DesktopGlass", #selector(NSApplication.terminate(_:)), "q"),
        ])
        menu("File", [
            item("Open Folder…", #selector(editorOpenFolder), "o", target: self),
            item("Go to File…", #selector(editorQuickOpen), "p", target: self),
            .separator(),
            item("Save", #selector(editorSave), "s", target: self),
            item("Save All", #selector(editorSaveAll), "s", [.command, .option], target: self),
            .separator(),
            item("Close Tab", #selector(editorCloseTab), "w", target: self),
            item("Close Window", #selector(NSWindow.performClose(_:)), "w", [.command, .shift]),
            .separator(),
            item("Back to Desktop", #selector(editorBack), "d", [.command, .shift], target: self),
        ])
        menu("Edit", [
            item("Undo", Selector(("undo:")), "z"),
            item("Redo", Selector(("redo:")), "z", [.command, .shift]),
            .separator(),
            item("Cut", #selector(NSText.cut(_:)), "x"),
            item("Copy", #selector(NSText.copy(_:)), "c"),
            item("Paste", #selector(NSText.paste(_:)), "v"),
            item("Select All", #selector(NSText.selectAll(_:)), "a"),
            .separator(),
            item("Find…", find, "f", tag: NSTextFinder.Action.showFindInterface.rawValue),
            item("Find and Replace…", find, "f", [.command, .option], tag: NSTextFinder.Action.showReplaceInterface.rawValue),
            item("Find Next", find, "g", tag: NSTextFinder.Action.nextMatch.rawValue),
            item("Find Previous", find, "g", [.command, .shift], tag: NSTextFinder.Action.previousMatch.rawValue),
            item("Use Selection for Find", find, "e", tag: NSTextFinder.Action.setSearchString.rawValue),
        ])
        menu("View", [
            item("Toggle Sidebar", #selector(editorToggleSidebar), "b", target: self),
            item("Toggle Terminal", #selector(editorToggleTerminal), "`", [.control], target: self),
            item("Toggle Preview / Visual Editor", #selector(editorTogglePreview), "v", [.command, .shift], target: self),
            .separator(),
            item("Artifact Viewer", #selector(showArtifacts), "a", [.command, .shift], target: self),
            .separator(),
            item("Bigger Text", #selector(editorBigger), "=", target: self),
            item("Smaller Text", #selector(editorSmaller), "-", target: self),
            item("Actual Size", #selector(editorActualSize), "0", target: self),
        ])
        menu("Window", [
            item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m"),
            item("Zoom", #selector(NSWindow.performZoom(_:)), ""),
        ])
        return main
    }

    @objc private func editorOpenFolder() { editor.model.chooseFolder() }
    @objc private func editorQuickOpen() { if editor.model.root != nil { editor.model.showQuickOpen = true } }
    @objc private func editorSave() { editor.model.saveActive() }
    @objc private func editorSaveAll() { editor.model.saveAll() }
    @objc private func editorCloseTab() { editor.model.closeActive() }
    @objc private func editorBack() { if editor.isVisible { morphBackToDesktop() } }
    @objc private func editorToggleSidebar() { editor.model.showSidebar.toggle() }
    @objc private func editorToggleTerminal() { editor.model.showTerminal.toggle() }
    @objc private func editorTogglePreview() {
        guard let doc = editor.model.active else { return }
        if doc.isMarkdown { doc.setPreview(!doc.isPreviewing) } else if doc.isHTML { doc.isVisual.toggle() }
    }
    @objc private func showArtifacts() { artifacts.showWindow() }
    @objc private func editorBigger() { editor.model.changeFontSize(by: 1) }
    @objc private func editorSmaller() { editor.model.changeFontSize(by: -1) }
    @objc private func editorActualSize() { editor.model.changeFontSize(by: nil) }
    @objc private func defaultsChanged(_ note: Notification) { registerHotKey(); applyIslandPrefs() }

    private func registerHotKey() {
        let choice = HotKeyChoice.current
        guard choice != registeredHotKey else { return }
        hotKey = nil   // unregister the old combo first
        hotKey = HotKey(keyCode: choice.keyCode, modifiers: choice.modifiers) { [weak self] in self?.shortcutPressed() }
        registeredHotKey = choice
    }

    // MARK: Menu

    func menuNeedsUpdate(_ menu: NSMenu) {
        menu.removeAllItems()
        menu.addItem(target(NSMenuItem(title: "Open Island   \(IslandPrefs.commandD ? "⌘D · " : "")\(HotKeyChoice.current.rawValue)", action: #selector(togglePanel), keyEquivalent: "")))
        menu.addItem(target(NSMenuItem(title: "Settings…", action: #selector(openSettings), keyEquivalent: ",")))
        menu.addItem(target(NSMenuItem(title: reindexer.running ? "Indexing…" : "Re-index Now", action: #selector(reindexNow), keyEquivalent: "")))
        menu.addItem(target(NSMenuItem(title: "Open Desktop Index", action: #selector(openIndex), keyEquivalent: "")))
        menu.addItem(target(NSMenuItem(title: "Artifact Viewer", action: #selector(showArtifacts), keyEquivalent: "")))
        if case .signedOut(let h)? = reindexer.problem {
            menu.addItem(target(NSMenuItem(title: "⚠︎ Sign In to \(h.label)…", action: #selector(signIn), keyEquivalent: "")))
        }
        if store.openQuestions > 0 {
            menu.addItem(target(NSMenuItem(title: "Answer \(store.openQuestions) Question(s) in \(Harness.current.label)", action: #selector(answerQuestions), keyEquivalent: "")))
        }
        menu.addItem(.separator())
        menu.addItem(submenu("Open Projects In", Editor.allCases.map(\.rawValue), current: Editor.current.rawValue, action: #selector(pickEditor(_:))))
        let scheduleItem = NSMenuItem(title: "Re-index Schedule", action: nil, keyEquivalent: "")
        let scheduleMenu = NSMenu()
        for s in Schedule.allCases {
            let item = target(NSMenuItem(title: s.label, action: #selector(pickSchedule(_:)), keyEquivalent: ""))
            item.representedObject = s.rawValue
            item.state = s == Schedule.current ? .on : .off
            scheduleMenu.addItem(item)
        }
        scheduleItem.submenu = scheduleMenu
        menu.addItem(scheduleItem)
        let with = NSMenuItem(title: "Re-index With", action: nil, keyEquivalent: "")
        let withMenu = NSMenu()
        for h in Harness.allCases {
            let installed = detector.installed[h] != nil
            let item = target(NSMenuItem(title: h.label + (installed ? "" : "  (not found)"), action: #selector(pickHarness(_:)), keyEquivalent: ""))
            item.representedObject = h.rawValue
            item.state = h == Harness.current ? .on : .off
            withMenu.addItem(item)
        }
        with.submenu = withMenu
        menu.addItem(with)

        let current = Harness.current
        ModelCatalog.shared.load(current, binary: detector.installed[current])
        let modelItem = NSMenuItem(title: "Re-index Model", action: nil, keyEquivalent: "")
        let modelMenu = NSMenu()
        let currentModel = current.model
        var options = [""] + ModelCatalog.shared.models(for: current)
        if !currentModel.isEmpty, !options.contains(currentModel) { options.append(currentModel) }
        for option in options {
            let item = target(NSMenuItem(title: option.isEmpty ? "Default" : option, action: #selector(pickModel(_:)), keyEquivalent: ""))
            item.representedObject = option
            item.state = option == currentModel ? .on : .off
            modelMenu.addItem(item)
            if option.isEmpty { modelMenu.addItem(.separator()) }
        }
        modelMenu.addItem(.separator())
        modelMenu.addItem(target(NSMenuItem(title: "Custom… (Settings)", action: #selector(openSettings), keyEquivalent: "")))
        modelItem.submenu = modelMenu
        menu.addItem(modelItem)
        let login = target(NSMenuItem(title: "Launch at Login", action: #selector(toggleLogin), keyEquivalent: ""))
        login.state = SMAppService.mainApp.status == .enabled ? .on : .off
        menu.addItem(login)
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: "Quit DesktopGlass", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))
    }

    private func target(_ item: NSMenuItem) -> NSMenuItem { item.target = self; return item }

    private func submenu(_ title: String, _ options: [String], current: String, action: Selector) -> NSMenuItem {
        let parent = NSMenuItem(title: title, action: nil, keyEquivalent: "")
        let sub = NSMenu()
        for option in options {
            let item = target(NSMenuItem(title: option, action: action, keyEquivalent: ""))
            item.representedObject = option
            item.state = option == current ? .on : .off
            sub.addItem(item)
        }
        parent.submenu = sub
        return parent
    }

    @objc private func reindexNow() { reindexer.run(reason: "menu") }
    @objc private func openSettings() {
        store.showingSettings = true
        if !panel.isVisible { showPanel() }
    }
    @objc private func openIndex() { Actions.openIndex() }
    @objc private func answerQuestions() { Actions.answerQuestions() }
    @objc private func signIn() { Actions.signIn() }

    @objc private func pickEditor(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String, let e = Editor(rawValue: raw) { Editor.current = e }
    }
    @objc private func pickSchedule(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String, let s = Schedule(rawValue: raw) { Schedule.current = s }
    }
    @objc private func pickModel(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String { Harness.current.model = Harness.sanitizeModel(raw) }
    }
    @objc private func pickHarness(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String, let h = Harness(rawValue: raw) { Harness.current = h }
    }
    @objc private func toggleLogin() {
        do {
            if SMAppService.mainApp.status == .enabled { try SMAppService.mainApp.unregister() }
            else { try SMAppService.mainApp.register() }
        } catch { NSSound.beep() }
    }
}

