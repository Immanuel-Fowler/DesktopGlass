import AppKit
import AVFoundation
import SwiftUI
import UniformTypeIdentifiers
import WebKit

// MARK: - Library

/// The custom wallpaper: an image or a looping, muted video. Files (picked or downloaded from a URL) are copied into
/// ~/Library/Application Support/DesktopGlass/Wallpapers so they keep working offline and after the original moves.
/// YouTube links can't be downloaded; they're saved as a tiny `<video id>.youtube` file and played through YouTube's
/// embedded player (muted, looping), so they need a connection.
/// It shows only in DesktopGlass mode (the pill switch), together with Finder's desktop icons being hidden.
@MainActor
final class WallpaperStore: ObservableObject {
    static let shared = WallpaperStore()

    static let dir = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/DesktopGlass/Wallpapers", isDirectory: true)
    static let maxRecents = 8

    @Published private(set) var current: URL?
    @Published private(set) var recents: [URL] = []
    /// Shown while a YouTube wallpaper can't play (offline, loading, embedding off) or if the wallpaper file is gone.
    @Published private(set) var fallback: URL?
    @Published private(set) var downloading = false
    @Published var error: String?

    private let d = UserDefaults.standard

    private init() {
        current = d.string(forKey: "wallpaper.current").map { URL(fileURLWithPath: $0) }
            .flatMap { FileManager.default.fileExists(atPath: $0.path) ? $0 : nil }
        fallback = d.string(forKey: "wallpaper.fallback").map { URL(fileURLWithPath: $0) }
            .flatMap { FileManager.default.fileExists(atPath: $0.path) ? $0 : nil }
        recents = (d.stringArray(forKey: "wallpaper.recents") ?? []).map { URL(fileURLWithPath: $0) }
            .filter { FileManager.default.fileExists(atPath: $0.path) }
    }

    nonisolated static func isVideo(_ url: URL) -> Bool {
        guard let t = UTType(filenameExtension: url.pathExtension.lowercased()) else { return false }
        return t.conforms(to: .movie) || t.conforms(to: .video)
    }

    /// A saved YouTube wallpaper (`<id>.youtube`).
    nonisolated static func isYouTube(_ url: URL) -> Bool { url.pathExtension.lowercased() == "youtube" }
    nonisolated static func youTubeID(_ url: URL) -> String { url.deletingPathExtension().lastPathComponent }

    /// The 11-character video id from a youtube.com/watch, /shorts, /embed, /live or youtu.be link.
    nonisolated static func youTubeID(fromLink url: URL) -> String? {
        guard let host = url.host?.lowercased() else { return nil }
        let parts = url.path.split(separator: "/").map(String.init)
        var id: String?
        if host == "youtu.be" || host.hasSuffix(".youtu.be") {
            id = parts.first
        } else if host == "youtube.com" || host.hasSuffix(".youtube.com") || host.hasSuffix("youtube-nocookie.com") {
            if parts.first == "watch" || parts.isEmpty {
                id = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "v" }?.value
            } else if ["shorts", "embed", "live", "v"].contains(parts[0]), parts.count > 1 {
                id = parts[1]
            }
        }
        guard let id, id.count == 11, id.allSatisfy({ $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" }) else { return nil }
        return id
    }

    nonisolated static func isSupported(_ url: URL) -> Bool {
        guard let t = UTType(filenameExtension: url.pathExtension.lowercased()) else { return false }
        return t.conforms(to: .image) || t.conforms(to: .movie) || t.conforms(to: .video)
    }

    func use(_ url: URL) {
        error = nil
        current = url
        d.set(url.path, forKey: "wallpaper.current")
        recents.removeAll { $0 == url }
        recents.insert(url, at: 0)
        recents = Array(recents.prefix(Self.maxRecents))
        d.set(recents.map(\.path), forKey: "wallpaper.recents")
        WallpaperController.shared.refresh()
    }

    func clear() {
        current = nil
        d.removeObject(forKey: "wallpaper.current")
        WallpaperController.shared.refresh()
    }

    func forget(_ url: URL) {
        recents.removeAll { $0 == url }
        d.set(recents.map(\.path), forKey: "wallpaper.recents")
        if current == url { clear() }
        if url.path.hasPrefix(Self.dir.path), url != fallback { try? FileManager.default.removeItem(at: url) }
    }

    /// Opens a file picker for an image or video and makes the pick the wallpaper.
    func chooseFile() {
        guard let picked = Self.pick("Choose an image or video for your desktop") else { return }
        importFile(picked)
    }

    /// Picks the image or video shown while a YouTube wallpaper can't play.
    func chooseFallback() {
        guard let picked = Self.pick("Choose a fallback image or video (shown when YouTube can't play)") else { return }
        guard Self.isSupported(picked) else { error = "That isn't an image or video file."; return }
        do {
            let dst = try Self.uniqueDestination(named: picked.lastPathComponent)
            try FileManager.default.copyItem(at: picked, to: dst)
            if let old = fallback, old.path.hasPrefix(Self.dir.path), !recents.contains(old) {
                try? FileManager.default.removeItem(at: old)
            }
            fallback = dst
            d.set(dst.path, forKey: "wallpaper.fallback")
            error = nil
            WallpaperController.shared.reload()
        } catch {
            self.error = "Couldn't copy the file: \(error.localizedDescription)"
        }
    }

    func clearFallback() {
        if let old = fallback, old.path.hasPrefix(Self.dir.path), !recents.contains(old) {
            try? FileManager.default.removeItem(at: old)
        }
        fallback = nil
        d.removeObject(forKey: "wallpaper.fallback")
        WallpaperController.shared.reload()
    }

    private static func pick(_ message: String) -> URL? {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.image, .movie, .video]
        panel.allowsMultipleSelection = false
        panel.message = message
        NSApp.activate(ignoringOtherApps: true)
        return panel.runModal() == .OK ? panel.url : nil
    }

    func importFile(_ src: URL) {
        guard Self.isSupported(src) else { error = "That isn't an image or video file."; return }
        do {
            let dst = try Self.uniqueDestination(named: src.lastPathComponent)
            try FileManager.default.copyItem(at: src, to: dst)
            use(dst)
        } catch {
            self.error = "Couldn't copy the file: \(error.localizedDescription)"
        }
    }

    /// Downloads a direct image/video link (https), caches it, and makes it the wallpaper.
    func download(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed), let scheme = url.scheme?.lowercased(), scheme == "https" || scheme == "http",
              url.host != nil else { error = "Paste a full link that starts with https://"; return }
        error = nil
        if let host = url.host?.lowercased(), host.contains("youtu") {
            guard let id = Self.youTubeID(fromLink: url) else { error = "Couldn't find the video in that YouTube link."; return }
            do {
                try FileManager.default.createDirectory(at: Self.dir, withIntermediateDirectories: true)
                let file = Self.dir.appendingPathComponent("\(id).youtube")
                try Data(trimmed.utf8).write(to: file)
                use(file)
            } catch {
                self.error = "Couldn't save the link: \(error.localizedDescription)"
            }
            return
        }
        downloading = true
        Task {
            defer { downloading = false }
            do {
                let (tmp, response) = try await URLSession.shared.download(from: url)
                if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                    error = "The server answered \(http.statusCode)."; return
                }
                let name = Self.fileName(for: url, response: response)
                guard Self.isSupported(URL(fileURLWithPath: name)) else {
                    try? FileManager.default.removeItem(at: tmp)
                    error = "That link isn't a direct image or video file (\(response.mimeType ?? "unknown type"))."
                    return
                }
                let dst = try Self.uniqueDestination(named: name)
                try FileManager.default.moveItem(at: tmp, to: dst)
                use(dst)
            } catch {
                self.error = "Download failed: \(error.localizedDescription)"
            }
        }
    }

    /// The link's file name, or one made from the response's MIME type when the path has no usable extension.
    private static func fileName(for url: URL, response: URLResponse) -> String {
        let base = url.lastPathComponent.isEmpty ? "wallpaper" : url.lastPathComponent
        if isSupported(URL(fileURLWithPath: base)) { return base }
        if let mime = response.mimeType, let ext = UTType(mimeType: mime)?.preferredFilenameExtension {
            return (URL(fileURLWithPath: base).deletingPathExtension().lastPathComponent) + "." + ext
        }
        return base
    }

    private static func uniqueDestination(named name: String) throws -> URL {
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let stem = URL(fileURLWithPath: name).deletingPathExtension().lastPathComponent
        let ext = URL(fileURLWithPath: name).pathExtension
        var candidate = dir.appendingPathComponent(name)
        var n = 2
        while FileManager.default.fileExists(atPath: candidate.path) {
            candidate = dir.appendingPathComponent("\(stem) \(n)" + (ext.isEmpty ? "" : ".\(ext)"))
            n += 1
        }
        return candidate
    }
}

// MARK: - Desktop windows

/// One borderless window per screen at the desktop-picture level (below the pill and every app window).
@MainActor
final class WallpaperController {
    static let shared = WallpaperController()
    private var windows: [WallpaperWindow] = []
    private var shown: URL?
    private var builtFor: [NSRect] = []

    private init() {
        // This also fires when the Dock or menu bar hides/shows (full-screen apps, Spaces). Rebuilding reloads the
        // wallpaper from the start, so only do it when a screen was really added, removed, moved or resized.
        NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification,
                                               object: nil, queue: .main) { _ in
            MainActor.assumeIsolated {
                let c = WallpaperController.shared
                if NSScreen.screens.map(\.frame) != c.builtFor { c.rebuild() }
            }
        }
        let ws = NSWorkspace.shared.notificationCenter
        for (name, playing) in [(NSWorkspace.screensDidSleepNotification, false), (NSWorkspace.screensDidWakeNotification, true),
                                (NSWorkspace.sessionDidResignActiveNotification, false),
                                (NSWorkspace.sessionDidBecomeActiveNotification, true)] {
            ws.addObserver(forName: name, object: nil, queue: .main) { _ in
                MainActor.assumeIsolated {
                    WallpaperController.shared.windows.forEach { $0.setPlaying(playing) }
                    if playing { WallpaperController.shared.ensureVisible(reason: "wake") }
                }
            }
        }
        ws.addObserver(forName: NSWorkspace.activeSpaceDidChangeNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { WallpaperController.shared.ensureVisible(reason: "space change") }
        }
        // Watchdog: whatever took the wallpaper off screen (a Space/display glitch, an app that hid our windows),
        // put it back within a few seconds.
        Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { _ in
            MainActor.assumeIsolated { WallpaperController.shared.ensureVisible(reason: "watchdog") }
        }
    }

    /// Re-shows wallpaper windows that should be up but aren't on screen.
    func ensureVisible(reason: String) {
        let want = IslandPrefs.hideDesktopIcons ? WallpaperStore.shared.current : nil
        guard let want else { return }
        if want != shown || windows.count != NSScreen.screens.count {
            NSLog("DesktopGlass wallpaper: rebuilding (\(reason)): windows=\(windows.count) screens=\(NSScreen.screens.count)")
            shown = want
            rebuild()
            return
        }
        for w in windows where !w.isVisible {
            NSLog("DesktopGlass wallpaper: window was off screen; showing it again (\(reason))")
            w.orderFrontRegardless()
        }
    }

    /// Shows the wallpaper when DesktopGlass mode is on and one is set; otherwise removes it.
    func refresh() {
        let want = IslandPrefs.hideDesktopIcons ? WallpaperStore.shared.current : nil
        guard want != shown || windows.count != NSScreen.screens.count else { return }
        shown = want
        rebuild()
    }

    /// Rebuilds the windows even if the wallpaper itself didn't change (e.g. a new fallback).
    func reload() { refresh(); rebuild() }

    private func rebuild() {
        windows.forEach { $0.teardown(); $0.orderOut(nil) }
        windows = []
        builtFor = NSScreen.screens.map(\.frame)
        guard let url = shown else { return }
        let fallback = WallpaperStore.shared.fallback
        windows = NSScreen.screens.map { WallpaperWindow(screen: $0, url: url, fallback: fallback) }
        windows.forEach { $0.orderFrontRegardless() }
    }
}

private final class WallpaperWindow: NSWindow, WKNavigationDelegate {
    private var players: [(AVQueuePlayer, AVPlayerLooper)] = []
    private var web: WKWebView?
    private var youTubeID: String?

    init(screen: NSScreen, url: URL, fallback: URL?) {
        super.init(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        setFrame(screen.frame, display: false)
        level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.desktopWindow)) + 1)
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenNone]
        ignoresMouseEvents = true
        isOpaque = true
        backgroundColor = .black
        hasShadow = false
        isReleasedWhenClosed = false

        let view = NSView(frame: NSRect(origin: .zero, size: screen.frame.size))
        view.wantsLayer = true
        view.layer?.backgroundColor = NSColor.black.cgColor
        if WallpaperStore.isYouTube(url) {
            // The fallback sits underneath and shows whenever YouTube isn't actually playing
            // (loading, offline, embedding disabled, removed video).
            if let fallback { addMedia(fallback, to: view) }
            let config = WKWebViewConfiguration()
            config.mediaTypesRequiringUserActionForPlayback = []
            // Covered windows are suspended by default; throttle instead so the player is still alive on return.
            if #available(macOS 14.0, *) { config.preferences.inactiveSchedulingPolicy = .throttle }
            let web = WKWebView(frame: view.bounds, configuration: config)
            web.autoresizingMask = [.width, .height]
            web.setValue(false, forKey: "drawsBackground")
            youTubeID = WallpaperStore.youTubeID(url)
            web.navigationDelegate = self
            loadYouTube(into: web)
            view.addSubview(web)
            self.web = web
        } else if FileManager.default.fileExists(atPath: url.path) {
            addMedia(url, to: view)
        } else if let fallback {
            addMedia(fallback, to: view)
        }
        contentView = view
    }

    /// An image or a looping, muted video filling the view.
    private func addMedia(_ url: URL, to view: NSView) {
        if WallpaperStore.isVideo(url) {
            let player = AVQueuePlayer()
            player.isMuted = true
            player.preventsDisplaySleepDuringVideoPlayback = false
            let looper = AVPlayerLooper(player: player, templateItem: AVPlayerItem(url: url))
            let layer = AVPlayerLayer(player: player)
            layer.videoGravity = .resizeAspectFill
            layer.frame = view.bounds
            layer.autoresizingMask = [.layerWidthSizable, .layerHeightSizable]
            view.layer?.addSublayer(layer)
            player.play()
            players.append((player, looper))
        } else if let image = NSImage(contentsOf: url) {
            let layer = CALayer()
            layer.contents = image
            layer.contentsGravity = .resizeAspectFill
            layer.frame = view.bounds
            layer.autoresizingMask = [.layerWidthSizable, .layerHeightSizable]
            view.layer?.addSublayer(layer)
        }
    }

    private func loadYouTube(into web: WKWebView) {
        guard let id = youTubeID else { return }
        // YouTube's player wants to know which site embeds it; this identifies the app (no referrer = error 153).
        web.loadHTMLString(Self.youTubeHTML(id: id), baseURL: URL(string: "https://desktopglass.app/"))
    }

    /// WebKit runs the page in a separate process, which macOS may stop (memory pressure, a crash). Without this the
    /// wallpaper silently goes blank; reload it instead.
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        NSLog("DesktopGlass wallpaper: YouTube player process stopped; reloading")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self, weak webView] in
            guard let self, let webView, self.web === webView else { return }
            self.loadYouTube(into: webView)
        }
    }

    func setPlaying(_ playing: Bool) {
        players.forEach { playing ? $0.0.play() : $0.0.pause() }
        if #available(macOS 12.0, *) { web?.setAllMediaPlaybackSuspended(!playing) }
    }

    /// YouTube's IFrame player, muted and looping, covering the screen. YouTube draws its title, play button, logo
    /// and end screen inside its own frame, which can't be styled, so instead:
    /// - the frame stays invisible until the video has really been playing for a moment (hides the start-up
    ///   play button, spinner and title card) and fades out again if it pauses, stalls or errors;
    /// - it's scaled up so the corner badges fall outside the screen;
    /// - it restarts a few seconds before the end, so the end screen never appears.
    private static func youTubeHTML(id: String) -> String {
        """
        <!doctype html><html><head><meta name="referrer" content="strict-origin-when-cross-origin"><style>
        html,body{margin:0;height:100%;overflow:hidden;background:transparent}
        #wrap{position:absolute;top:50%;left:50%;width:100vw;height:56.25vw;min-height:100vh;min-width:177.78vh;
          transform:translate(-50%,-50%) scale(1.18);opacity:0;transition:opacity .6s ease;pointer-events:none}
        #wrap.on{opacity:1}
        #wrap iframe{width:100%;height:100%;border:0}
        </style></head><body><div id="wrap"><div id="p"></div></div>
        <script>
        var wrap=document.getElementById('wrap'),player,shown=false,everShown=false,hideTimer,revealTimer,poll;
        function show(){clearTimeout(hideTimer);everShown=true;if(!shown){shown=true;wrap.classList.add('on');}}
        function hide(now){clearTimeout(revealTimer);clearTimeout(hideTimer);
          hideTimer=setTimeout(function(){shown=false;wrap.classList.remove('on');},now?0:500);}
        // Back on screen: resume at once (WebKit paused it while the desktop was covered).
        document.addEventListener('visibilitychange',function(){
          if(!document.hidden&&player&&player.playVideo){player.playVideo();}
        });
        function onYouTubeIframeAPIReady(){
          player=new YT.Player('p',{videoId:'\(id)',host:'https://www.youtube-nocookie.com',
            playerVars:{autoplay:1,mute:1,controls:0,disablekb:1,fs:0,iv_load_policy:3,modestbranding:1,
              playsinline:1,rel:0,cc_load_policy:0,showinfo:0,origin:'https://desktopglass.app'},
            events:{
              onReady:function(e){e.target.mute();e.target.playVideo();},
              onStateChange:function(e){
                if(e.data===YT.PlayerState.PLAYING){
                  // the title card only appears on the very first start; after that, show again right away
                  if(shown||everShown){show();}else{clearTimeout(revealTimer);revealTimer=setTimeout(show,2500);}
                }else if(e.data===YT.PlayerState.ENDED){player.seekTo(0,true);player.playVideo();}
                else if(document.hidden){}   // WebKit pauses video in covered windows; nobody can see it anyway
                else if(e.data===YT.PlayerState.PAUSED){hide(false);player.playVideo();}
                else{hide(false);}
              },
              onError:function(){hide(true);}
            }});
          poll=setInterval(function(){
            if(!player||!player.getDuration)return;
            var d=player.getDuration(),t=player.getCurrentTime();
            if(d>30&&t>d-20){player.seekTo(0,true);}       // before the end screen
            else if(d>0&&d<=30&&t>d-0.4){player.seekTo(0,true);}
          },250);
        }
        </script>
        <script src="https://www.youtube.com/iframe_api"></script>
        </body></html>
        """
    }

    func teardown() {
        web?.navigationDelegate = nil
        web?.loadHTMLString("", baseURL: nil)
        web = nil
        players.forEach { $0.0.pause(); $0.1.disableLooping() }
        players = []
    }
}

// MARK: - The pop-up next to the island

/// A small glass panel that hangs just below the pill. Closes when it loses focus.
@MainActor
final class WallpaperPopup: NSPanel {
    static let shared = WallpaperPopup()
    static let size = NSSize(width: 340, height: 400)

    private init() {
        super.init(contentRect: NSRect(origin: .zero, size: Self.size),
                   styleMask: [.nonactivatingPanel, .borderless, .fullSizeContentView],
                   backing: .buffered, defer: false)
        level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.desktopIconWindow)) + 2)
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false     // no hairline outline
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        collectionBehavior = [.canJoinAllSpaces, .transient, .ignoresCycle]
        contentView = NSHostingView(rootView: WallpaperPopupView(close: { WallpaperPopup.shared.dismiss() }))
        NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: self, queue: .main) { _ in
            MainActor.assumeIsolated {
                // the file picker takes focus while it's open; stay up for it
                guard NSApp.modalWindow == nil else { return }
                WallpaperPopup.shared.dismiss()
            }
        }
    }

    override var canBecomeKey: Bool { true }

    override func cancelOperation(_ sender: Any?) { dismiss() }

    /// Shows it below `pillRect` (screen coordinates), or hides it if it's already up.
    func toggle(below pillRect: NSRect) {
        if isVisible { dismiss(); return }
        let origin = NSPoint(x: pillRect.midX - Self.size.width / 2, y: pillRect.minY - Self.size.height - 8)
        setFrame(NSRect(origin: origin, size: Self.size), display: true)
        alphaValue = 0
        makeKeyAndOrderFront(nil)
        NSAnimationContext.runAnimationGroup { $0.duration = 0.16; animator().alphaValue = 1 }
    }

    func dismiss() {
        guard isVisible else { return }
        NSAnimationContext.runAnimationGroup({ $0.duration = 0.12; animator().alphaValue = 0 },
                                             completionHandler: { MainActor.assumeIsolated { self.orderOut(nil) } })
    }
}

private struct WallpaperPopupView: View {
    let close: () -> Void
    @ObservedObject private var store = WallpaperStore.shared
    @AppStorage("island.hideDesktopIcons") private var glassMode = true
    @State private var link = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Image(systemName: "photo.on.rectangle.angled").font(.system(size: 14, weight: .semibold))
                Text("Wallpaper").font(.system(size: 14, weight: .semibold))
                Spacer()
                Toggle("", isOn: $glassMode).labelsHidden().toggleStyle(.switch).controlSize(.mini)
                    .help("DesktopGlass mode: hide desktop icons and show this wallpaper")
            }

            preview
                .frame(height: 120)
                .frame(maxWidth: .infinity)
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))

            HStack(spacing: 8) {
                Button { store.chooseFile() } label: { Label("Choose File…", systemImage: "folder") }
                if store.current != nil {
                    Button { store.clear() } label: { Label("None", systemImage: "xmark") }
                }
            }
            .controlSize(.small)

            HStack(spacing: 6) {
                Image(systemName: "arrow.uturn.backward.circle").foregroundStyle(.secondary)
                Text(store.fallback.map { "Fallback: \($0.lastPathComponent)" } ?? "No fallback (shown if YouTube can't play)")
                    .font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                Spacer(minLength: 4)
                Button(store.fallback == nil ? "Choose…" : "Change…") { store.chooseFallback() }.controlSize(.mini)
                if store.fallback != nil {
                    Button { store.clearFallback() } label: { Image(systemName: "xmark") }.controlSize(.mini)
                }
            }

            HStack(spacing: 6) {
                TextField("Paste a YouTube, image or video link", text: $link)
                    .textFieldStyle(.roundedBorder).controlSize(.small)
                    .onSubmit(download)
                if store.downloading { ProgressView().controlSize(.small) }
                else { Button("Use", action: download).controlSize(.small).disabled(link.isEmpty) }
            }

            if let error = store.error {
                Text(error).font(.system(size: 11)).foregroundStyle(.red).lineLimit(2)
            } else if !glassMode, store.current != nil {
                Text("Turn the switch on to show it on your desktop.").font(.system(size: 11)).foregroundStyle(.secondary)
            }

            if !store.recents.isEmpty {
                Text("Recent").font(.system(size: 11, weight: .semibold)).foregroundStyle(.secondary)
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(store.recents, id: \.self) { url in
                            WallpaperThumb(url: url, selected: url == store.current)
                                .onTapGesture { store.use(url) }
                                .contextMenu { Button("Remove") { store.forget(url) } }
                        }
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .padding(14)
        .frame(width: WallpaperPopup.size.width, height: WallpaperPopup.size.height, alignment: .top)
        .glassCard(cornerRadius: 18)
    }

    @ViewBuilder private var preview: some View {
        if let url = store.current {
            WallpaperThumb(url: url, selected: false, size: nil)
        } else {
            ZStack {
                Color.primary.opacity(0.06)
                Text("No wallpaper yet").font(.system(size: 12)).foregroundStyle(.secondary)
            }
        }
    }

    private func download() {
        guard !link.isEmpty else { return }
        store.download(link)
        link = ""
    }
}

/// A still for an image or the first frame of a video.
private struct WallpaperThumb: View {
    let url: URL
    let selected: Bool
    var size: NSSize? = NSSize(width: 72, height: 44)
    @State private var image: NSImage?

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            if let image {
                Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
            } else {
                Color.primary.opacity(0.08)
            }
            if WallpaperStore.isVideo(url) || WallpaperStore.isYouTube(url) {
                Image(systemName: WallpaperStore.isYouTube(url) ? "play.rectangle.fill" : "play.circle.fill").font(.system(size: 11)).foregroundStyle(.white).padding(3)
                    .shadow(radius: 2)
            }
        }
        .frame(width: size?.width, height: size?.height)
        .frame(maxWidth: size == nil ? .infinity : nil, maxHeight: size == nil ? .infinity : nil)
        .clipped()
        .clipShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
            .strokeBorder(selected ? Color.accentColor : .clear, lineWidth: 2))
        .help(url.lastPathComponent)
        .task(id: url) { image = await Self.still(for: url) }
    }

    private static func still(for url: URL) async -> NSImage? {
        if WallpaperStore.isYouTube(url) {
            let id = WallpaperStore.youTubeID(url)
            guard let thumb = URL(string: "https://i.ytimg.com/vi/\(id)/hqdefault.jpg"),
                  let (data, _) = try? await URLSession.shared.data(from: thumb) else { return nil }
            return NSImage(data: data)
        }
        guard WallpaperStore.isVideo(url) else { return NSImage(contentsOf: url) }
        let gen = AVAssetImageGenerator(asset: AVURLAsset(url: url))
        gen.appliesPreferredTrackTransform = true
        gen.maximumSize = CGSize(width: 640, height: 360)
        return await withCheckedContinuation { cont in
            gen.generateCGImagesAsynchronously(forTimes: [NSValue(time: .zero)]) { _, cg, _, _, _ in
                cont.resume(returning: cg.map { NSImage(cgImage: $0, size: .zero) })
            }
        }
    }
}
