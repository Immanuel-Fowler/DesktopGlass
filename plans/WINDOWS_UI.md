# Plan: a native Windows UI for DesktopGlass

**Goal:** a Windows app that feels like it was made for Windows 11 and is not a port of the Mac look. It is a thin
client of the Rust engine described in [README.md](README.md), the same way the Mac app will be.

**Prerequisite:** engine Phases 1–5 (the `dg engine` protocol) and Phase 6 (the Windows platform layer). The UI shell
(W0–W1 below) can start as soon as the Phase 5 protocol types are defined, using a mock engine.

---

## 1. Stack

| Choice | Why |
|---|---|
| **WinUI 3** (Windows App SDK 1.6+) | Microsoft's current native UI, with real Mica/Acrylic, Fluent controls and the Windows 11 look |
| **C# / .NET 8 LTS** | the best-supported WinUI language; good async and process APIs |
| **CommunityToolkit.Mvvm** | view-models with source generators and little boilerplate |
| **CommunityToolkit.WinUI** controls | `SettingsCard`/`SettingsExpander` for a Windows-11-style Settings page |
| **StreamJsonRpc** | Microsoft's JSON-RPC library, which fits the engine's stdio protocol |
| **H.NotifyIcon.WinUI** | tray icon and menu, which WinUI lacks built in |
| **WebView2** | artifact viewer, visual HTML editor, Monaco code editor, xterm.js terminal |
| **MSIX** packaging | clean install/uninstall, `desktopglass://` protocol, start-at-login task, auto-update via App Installer |

**Target:** Windows 11 22H2+ for the full look. Windows 10 (19041+) runs with an Acrylic-or-solid fallback instead of
Mica, with no other differences.

## 2. Project layout

```
windows/
├── DesktopGlass.sln
├── DesktopGlass/                      WinUI 3 app (packaged)
│   ├── App.xaml(.cs)                  single instance, protocol activation, tray, hotkey, engine startup
│   ├── Engine/
│   │   ├── EngineClient.cs            spawns dg.exe engine, restarts with backoff, StreamJsonRpc proxy
│   │   ├── Generated/Protocol.cs      generated from `dg schema` at build time (never hand-edited)
│   │   └── UiHandlers.cs              UI-bound calls the engine forwards: browser_*, html_*, show_*, play_game
│   ├── Shell/
│   │   ├── OverlayWindow.xaml         the full-screen glass overlay
│   │   ├── HotKeyService.cs           RegisterHotKey on a message-only window
│   │   ├── TrayService.cs
│   │   └── Backdrop.cs                Mica / Acrylic / fallback selection
│   ├── Views/                         Overlay, Cards, Sidebar, Search, Settings, Editor, Artifacts, Games
│   ├── ViewModels/
│   ├── Web/                           WebView2 hosting + bridge (shared JS comes from ../../shared/web)
│   └── Package.appxmanifest
├── DesktopGlass.Tests/                view-model + EngineClient tests against a mock engine
└── build.ps1                          cargo build --release (x64 + arm64) → copy dg.exe → msbuild → MSIX
```

`shared/web/` at the repo root holds the JavaScript, CSS and HTML that both platforms load into their WebViews: the
visual HTML editor's injected script, Markdown preview rendering, and the Monaco/xterm.js host pages.

## 3. Mac → Windows mapping

| Mac (today) | Windows | Notes |
|---|---|---|
| `OverlayPanel.swift`: borderless floating `NSPanel` | `OverlayWindow`: `AppWindow` with `OverlappedPresenter` (no border/title bar, always on top), sized to the **current monitor's work area**, `WS_EX_TOOLWINDOW` so it stays out of Alt+Tab | Shows on the monitor under the cursor. Esc or a focus loss hides it |
| `Glass.swift`: Liquid Glass / `.ultraThinMaterial` | `DesktopAcrylicBackdrop` for the overlay (it must see through to the desktop); **Mica** for normal windows (Settings, Editor); `AcrylicBrush` on cards | Fall back to a solid theme color when *Transparency effects* is off or on battery saver, as Windows itself does |
| `OverlayView.swift`: header, sidebar, cards, search, footer, problem chip | `NavigationView` (left, compact) for the sidebar; `ItemsRepeater` + `UniformGridLayout` for cards; `AutoSuggestBox` for search; `InfoBar` for problem chips | Cards: 8px corners, Fluent elevation, `Segoe UI Variable` type ramp |
| Morph animation (`AppDelegate`) | `ConnectedAnimationService` (card → editor) + Composition implicit animations | Respect *Animation effects* off |
| `HotKey.swift`: Carbon, ⌥⌘D | `RegisterHotKey`; default **Ctrl+Alt+D**, configurable | Many Win+ combos are reserved by the shell. Show a clear error when a hotkey is taken |
| Menu-bar icon + menu | Tray icon + `MenuFlyout` | Left-click toggles the overlay, right-click opens the menu |
| `Settings.swift`, `IndexPreferences.swift`, `MCPSection.swift` | Settings window: `NavigationView` with pages (General, Re-indexing, Indexing Preferences, MCP, Games) built from `SettingsCard`s | All values read and written through the engine (`settings.json`) |
| `SMAppService` launch at login | MSIX `StartupTask` | Toggle in Settings → General |
| `Editor/`: TextKit editor, `Syntax.swift`, `MarkdownStyler.swift` | **Monaco** in WebView2 | Gives highlighting, find/replace and minimap for free, so `Syntax.swift` isn't ported |
| `MarkdownVisuals.swift`: Markdown preview | WebView2 rendering via the shared `shared/web/markdown` page | Same output on both platforms |
| `Terminal.swift`: SwiftTerm | **xterm.js** in WebView2 + **ConPTY** in C# | PTY stays in the UI process as on Mac, so keystrokes don't go through the engine |
| `Conversations.swift` | port as-is (view + view-model) | |
| `ArtifactBrowser.swift`: `WKWebView` | WebView2: `NavigationStarting` enforces the engine's allow-list, `SetVirtualHostNameToFolderMapping` for local files, `CapturePreviewAsync` for `browser_screenshot`, `ExecuteScriptAsync` for `browser_eval` | `ArtifactAccounts.swift` → WebView2 profiles per account |
| `HTML/HTMLVisualSession.swift`, `HTMLVisualEditor.swift` | WebView2 + the shared injected script (`AddScriptToExecuteOnDocumentCreatedAsync`, `WebMessageReceived`) | Edit logic (`HTMLOps`) is in the engine, so only the session glue is new |
| `desktopglass://open` URL scheme | MSIX `windows.protocol` extension → `AppInstance` redirection to the running instance | Used by `dg view` and the autoshow hook |
| `NSWorkspace` open/reveal | `Launcher.LaunchFileAsync`; `explorer.exe /select,` to reveal | |
| `GamepadNavigator.swift` | built-in XAML gamepad focus navigation (`XYFocusKeyboardNavigation`) | Little code needed |
| `Games/`: `GameController`, native games library | `Windows.Gaming.Input`; installed games from the engine's Windows catalogue | **Optional for v1** |
| `FoundationModelsCLI.swift`, MLX | not offered | Hidden from the backend picker |

## 4. Engine integration

- `EngineClient` starts `dg.exe engine` from the package folder at app launch, connects StreamJsonRpc over its
  stdin/stdout, and restarts it with exponential backoff. The UI shows an `InfoBar` if the engine keeps crashing.
- All protocol types come from `Generated/Protocol.cs`, which is regenerated from `dg schema` in `build.ps1`. The
  build fails if the generated file is out of date.
- Engine notifications (`desktop/changed`, `index/progress`, `index/problem`) update view-models on the UI thread
  through `DispatcherQueue`.
- `UiHandlers` implements the methods the engine forwards from MCP clients: `browser_tabs/read/screenshot/click/type/
  eval/navigate`, `html_*` (live session side), `show_artifact`, `open_in_editor`, `show_overlay`, `play_game`. The
  Mac app implements the same list, so one integration test suite (run against each UI) covers both.

## 5. Windows design rules

Follow Windows conventions instead of mimicking macOS:
- **Fluent everywhere:** `Segoe UI Variable`, `Segoe Fluent Icons`, the system accent color, standard control templates.
- **Themes:** light, dark and **high contrast** (test all three); follow the system setting by default.
- **Respect system settings:** *Transparency effects*, *Animation effects* and text scaling (layout must survive 150%
  text size).
- **Keyboard first:** every overlay action reachable by keyboard; arrow-key card navigation; access keys (Alt) in
  Settings.
- **Accessibility:** `AutomationProperties.Name` on cards (folder name + summary) and icons; test with Narrator.
- **Per-monitor DPI:** WinUI handles it, but test mixed-DPI multi-monitor setups for the overlay.
- **Window placement:** title-bar-less windows use the standard caption buttons via `AppWindowTitleBar` extension.

## 6. Phases

| Phase | Scope | Done when |
|---|---|---|
| **W0 Shell** (2–3 days) | WinUI project, MSIX manifest, single instance, tray icon, hotkey, empty Acrylic overlay, `EngineClient` against a mock engine | Ctrl+Alt+D toggles a glass overlay on the right monitor; the tray menu works |
| **W1 Overlay, read-only** (4–5 days) | header, sidebar, card grid from `desktop/overview`, search, folder drill-down, open/reveal in Explorer, keyboard navigation, themes | browsing a real (OneDrive-redirected) Desktop matches the Mac app's content |
| **W2 Settings + indexing** (4–5 days) | all Settings pages, backend picker with detection, schedule, preferences, Re-index, progress, `InfoBar` problem chips for exit codes 76–79/127, sign-in flow, unattended consent dialog (stating there is no sandbox on Windows), start at login | a schedule-driven re-index with Claude Code and with Ollama both work end to end |
| **W3 Artifact viewer + MCP** (4–5 days) | WebView2 viewer with allow-list, `desktopglass://` activation, `dg view`, `UiHandlers` for `browser_*`/`show_*`, MCP install page | an agent in Windows Terminal writes an HTML file and it appears in the viewer, and `browser_screenshot` works through MCP |
| **W4 Editor + terminal** (5–7 days) | Monaco editor (open/save/dirty state), Markdown preview, xterm.js + ConPTY terminal, conversations panel, card → editor connected animation | editing, previewing and running a CLI inside the app all work |
| **W5 Visual HTML editor** (3–4 days) | shared injected script, live session, `html_*` handlers | the shared `html_*` integration tests pass against Windows |
| **W6 Games** (optional, 3–4 days) | games library view, `Windows.Gaming.Input`, native game launch | installed games listed and launched |
| **W7 Ship** (2–3 days) | x64 + arm64 MSIX, code signing (self-signed for dev, Azure Trusted Signing for release), App Installer auto-update, `install.ps1` for developers, README section | clean install on a fresh Windows 11 VM with no dev tools |

**Total:** ~4–5 weeks for one developer (W6 excluded), after the engine work.

## 7. Testing

- **Unit:** view-models and `EngineClient` against a scripted mock engine (`DesktopGlass.Tests`, MSTest).
- **Shared integration suite:** the same protocol-level tests run against the Mac UI and the Windows UI for every
  UI-bound method.
- **UI automation:** a few WinAppDriver/Appium smoke tests: hotkey → overlay → search → open card.
- **Manual matrix:** Windows 10 and 11, light/dark/high contrast, 100%/150%/200% scaling, single and mixed-DPI dual
  monitors, OneDrive-redirected and local Desktop, Transparency off.

## 8. Open questions

1. **Default hotkey:** Ctrl+Alt+D, or something Win-based like Win+Shift+D (may collide with other apps)?
2. **Distribution:** sideloaded MSIX only, or also the Microsoft Store (stricter rules for launching agent CLIs)?
3. **Windows 10 support:** keep it with the Acrylic fallback, or require Windows 11?
4. **Games library:** ship in v1 or later?
5. **Sandbox:** is "no sandbox, the CLIs' own permissions only" acceptable for v1 on Windows, or is an
   AppContainer-based runner a release blocker?
