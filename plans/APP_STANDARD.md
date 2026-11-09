# Plan: the DesktopGlass app standard

**Goal:** let developers build apps that run *inside* DesktopGlass. An app can put icons in the island's top bar,
open terminal panes next to the agents, show HTML/CSS interfaces, and draw its own graphics — up to full 3D games —
in a custom overlay. The standard is defined by the Rust engine (see [README.md](README.md)), so the same app
package runs on the Mac app and the future Windows app.

**Depends on:** Rust plan Phase 0 (the `dg` binary) and Phase 5 (`dg engine` daemon + `dg-proto`). Nothing here
starts before the engine owns the control channel.

---

## 1. What an app is

An app is **its own process** plus a **package folder**. DesktopGlass (the engine) launches it, gives it a private
connection, and draws whatever it asks for. It never runs inside the DesktopGlass process.

```
                    ┌──────────── DesktopGlass UI (Swift / WinUI) ─────────────┐
                    │ island top bar · terminal dock · web views · surfaces    │
                    └───────────────▲──────────────────────────▲───────────────┘
                     JSON-RPC (UI)  │                          │ shared GPU textures
                    ┌───────────────┴──────────┐               │ (IOSurface / DXGI)
                    │ dg engine — app host     │               │
                    │ manifest, permissions,   │               │
                    │ lifecycle, routing       │               │
                    └───────────────▲──────────┘               │
                    dgapp/1 socket  │ (one per app)            │
                    ┌───────────────┴──────────────────────────┴───────────────┐
                    │ the app: any language. Rust SDK first, Swift + C# later  │
                    │ logic · state · its own renderer (wgpu, Bevy, Metal…)    │
                    └──────────────────────────────────────────────────────────┘
```

**Why a separate process:** a crashing or hung app can't take DesktopGlass down; apps can be written in any
language; and the OS sandbox can be applied per app (§7). It is also exactly how the engine already treats agent CLIs.

## 2. Package format

```
Weather.dgapp/
├── dgapp.toml            manifest (below)
├── icon.svg              app icon (also 512×512 icon.png)
├── bin/
│   ├── macos-universal/weather
│   └── windows-x64/weather.exe
├── ui/                   optional: HTML/CSS/JS served at dgapp://<id>/
│   └── index.html
└── assets/               anything else the app reads
```

```toml
[app]
id       = "com.example.weather"     # reverse-DNS, unique
name     = "Weather"
version  = "1.2.0"
kind     = "app"                     # "app" | "game" (games also appear in the Games library)
standard = 1                         # dgapp protocol major version

[run]
mac      = "bin/macos-universal/weather"
windows  = "bin/windows-x64/weather.exe"
args     = []
keep_alive = false                   # true: stays running with the island closed (top-bar items, background work)

[ui]
entry    = "ui/index.html"           # optional; omit for terminal-only or surface-only apps

[capabilities]                       # what the app may ask the host for (see §3)
topbar   = true
terminal = true
web      = true
surface  = "gpu"                     # "none" | "gpu"
overlay  = false                     # draw above the desktop outside the island

[permissions]                        # shown to the user at install; see §7
network  = ["api.weather.gov"]
files    = ["pick"]                  # "pick" (user-chosen files only) | "desktop:read" | "desktop:write"
desktop_index = false                # read the Desktop index / descriptions
```

Installed apps live in `~/Library/Application Support/DesktopGlass/Apps/` (`%APPDATA%\DesktopGlass\Apps\`). They
show up in the sidebar's **Apps** dropdown — added in this release, which lists Mac apps today — in a
"DesktopGlass" group above the system apps. `kind = "game"` apps are also listed in the Games library.

## 3. The protocol: `dgapp/1`

JSON-RPC 2.0, newline-delimited, over a private socket the engine creates for each launch (a Unix domain socket on
macOS, a named pipe on Windows). Its path and a one-time token arrive in `DG_APP_SOCKET` / `DG_APP_TOKEN`. Types live
in `dg-proto` and are generated for every SDK, so a renamed field breaks the build instead of a running app.

Large or fast data never goes over JSON: terminal bytes use binary frames on the same socket, and graphics use shared
GPU memory (§5).

### 3.1 Lifecycle
| Message | Direction | Purpose |
|---|---|---|
| `app/hello` | app → host | protocol version, SDK name; host replies with granted capabilities and theme |
| `app/activate`, `app/deactivate` | host → app | the user opened or left the app |
| `app/quit` | host → app | asked to quit; the host kills it after 3 s |
| `app/theme` | host → app | light/dark, accent, glass vs. solid, so apps can match the island |

### 3.2 Top bar items (`capabilities.topbar`)
The island's header row (search · Re-index · terminal · settings) gets an **app area**.

| Call | Notes |
|---|---|
| `topbar/set { id, icon, title?, tooltip?, badge?, menu? }` | `icon` is an SF Symbol name, or a package SVG/PNG path (template-rendered so it matches the glass). Upsert by `id`. |
| `topbar/remove { id }` | |
| `topbar/clicked { id }`, `topbar/menuSelected { id, item }` | events to the app |

Limits: 3 items per app; the user can hide any item or pin it to the pill so it shows with the island folded up.

### 3.3 Terminal panes (`capabilities.terminal`)
Panes open as tabs in the docked terminal, next to the agents — the same panel the IDE uses.

| Call | Notes |
|---|---|
| `terminal/open { title, mode, cwd?, env?, command? }` → `{ pane }` | `mode: "command"` — the host runs `command` in a PTY (like the agent CLIs). `mode: "stream"` — the app *is* the program: it writes ANSI bytes and reads keystrokes. |
| binary frame `terminal/output { pane, bytes }` | app → host (stream mode) |
| binary frame `terminal/input { pane, bytes }` | host → app (stream mode) |
| `terminal/resize { pane, cols, rows }` | event to the app |
| `terminal/close { pane }`, `terminal/exited { pane, code }` | |

Stream mode is how a Rust TUI (ratatui, crossterm) runs inside DesktopGlass without a real shell.

### 3.4 HTML/CSS views (`capabilities.web`)
| Call | Notes |
|---|---|
| `view/open { entry, placement, size? }` → `{ view }` | `placement`: `"island"` (fills the island's content area, sidebar stays), `"window"` (its own glass window), `"overlay"` (see §5.3) |
| `view/close { view }` | |

Pages load from `dgapp://<app id>/…` (package files only) under a strict CSP. Inside the page,
`window.dg.invoke(method, params)` and `window.dg.on(event, fn)` talk to **the app's own process** (relayed by the
host); the page never gets host APIs directly. WebGL and WebGPU are enabled, so three.js, Babylon.js and PlayCanvas
games work in this layer with no native code at all.

### 3.5 Other services
`storage/dir` (per-app data folder) · `files/pick`, `files/save` (user-chosen, permission `files: pick`) ·
`desktop/search`, `desktop/describe` (permission `desktop_index`) · `notify/post` · `clipboard/read|write` ·
`input/gamepad` (forwarded from `GamepadNavigator`'s GameController stream) · `mcp/register` (expose tools to agents in
the terminal, opt-in).

## 4. Languages

| Layer | Now | Later |
|---|---|---|
| App logic | **Rust** — `dg-app` crate (async + blocking APIs), published with the engine | **Swift** package (`DesktopGlassKit`), **C#** NuGet (`DesktopGlass.Apps`); any language can speak the protocol directly |
| UI | **HTML + CSS + JS** in the host's WebView | A declarative native UI tree (`ui/render { tree }` → SwiftUI / WinUI) if web views prove too heavy |
| Graphics | **wgpu** via `dg-app`'s `Surface` | Bevy plugin, raw Metal/D3D12 handles, Swift `MTKView`-style helper |

All three SDKs are thin and generated from `dg-proto`, so features land in all of them at once.

## 5. Graphics: the custom overlay

### 5.1 Would people be able to make 3D games with this easily?

**Yes, if they bring an existing engine.** That is the design choice that makes it easy: DesktopGlass hands the app a
GPU texture the host composites with zero copies, and the app draws into it however it likes. Concretely:

- **Bevy (Rust):** a `bevy_desktopglass` plugin swaps Bevy's window for a DesktopGlass surface and feeds it input.
  An existing Bevy game becomes a DesktopGlass app with ~10 lines of setup.
- **wgpu / raw Metal / D3D12:** `dg-app` gives you a `wgpu::Surface`-like target; you write a normal render loop.
- **Web:** three.js / Babylon.js in an HTML view (§3.4) — no native code, slightly lower ceiling.
- **Godot / Unity:** possible later through a small native plugin that targets the same shared texture; not in v1.

What we deliberately **don't** do is write our own 3D engine or scene graph. That would take years and still trail
Bevy, Godot or three.js. The standard's job is to give apps a fast, glass-integrated place to draw, plus input,
audio passthrough and gamepads.

### 5.2 How surfaces work
| Step | macOS | Windows |
|---|---|---|
| App asks | `surface/create { width, height, placement, transparent, vsync }` | same |
| Host allocates | a ring of 3 `IOSurface`s; sends their Mach ports (`IOSurfaceCreateMachPort`) over an XPC side-channel | 3 shared D3D12/DXGI textures; sends NT handles (`DuplicateHandle` into the app process) |
| App draws | imports them as `MTLTexture`s (wgpu `Texture` via `wgpu-hal`), renders | imports as `ID3D12Resource` |
| App presents | `surface/present { buffer, damage? }` | same |
| Host shows | sets the buffer as a `CALayer`'s contents inside the island / window / overlay | composition `SpriteVisual` |
| Pacing | `surface/frame { time, interval }` events from `CADisplayLink` / DWM vblank | same |
| Input | `input/pointer`, `input/key`, `input/scroll`, `input/gamepad` events with host timestamps; pointer lock via `surface/captureCursor` | same |

Resizing hands out a new ring; the app keeps drawing into the old ring until it acks. Lost-device and app-crash
paths show a "the app stopped" card in place of the surface.

### 5.3 Placements
- **island** — inside the island's content area (a game or tool in the panel).
- **window** — a separate glass window; can go full screen.
- **overlay** — a transparent, click-through layer above the desktop (widgets, HUDs, live wallpapers). It sits at the
  same level as the island pill, so app windows cover it. Clicks pass through except on regions the app declares with
  `surface/hitRegions`.

### 5.4 Desktop widgets

Widgets are ordinary `.dgapp` packages whose manifest declares `"widgets"`; there is no separate format. The point is
that anyone can ship a desktop widget — clocks, system monitors, calendars, now-playing, feeds, a tiny game — without
being limited to what WidgetKit allows (no timeline-only refresh, no snapshot rendering, real interactivity, any
language).

```jsonc
"widgets": [{
  "id": "clock",
  "name": "Glass Clock",
  "kind": "web",                 // "web" (ui/ HTML/CSS/JS) or "surface" (the app's own GPU renderer, §5.2)
  "entry": "ui/clock.html",
  "sizes": ["small", "medium", "large"],   // 160×160, 340×160, 340×340 pt; or "free" with min/max
  "refresh": "live",             // "live" (always running) or { "every": "15m" } (woken, rendered, paused)
  "interactive": true            // false = fully click-through
}]
```

- **Placement:** widgets live on the overlay layer (§5.3) and only show in **DesktopGlass mode** (icons hidden), above
  the custom wallpaper and below the pill and app windows. Turning the mode off hides every widget (and pauses them);
  turning it back on restores them exactly where they were.
- **Locked by default.** Outside edit mode a widget can't be moved, resized or removed — dragging on it does nothing
  (or goes to the widget, if it's `interactive`), so a stray click never rearranges the desktop.
- **Edit mode — right-click the desktop.** In DesktopGlass mode the desktop is DesktopGlass's own surface (Finder's
  desktop is off), so a right-click on empty desktop opens a glass context menu: **Edit Widgets**, Wallpaper…, and
  DesktopGlass Mode (off). Right-clicking a widget offers **Edit Widgets** too, plus that widget's own
  settings. (Also reachable from the pill's context menu.)
  - In edit mode every widget gets an outline, a remove (×) button and a resize handle, and stops receiving input, so
    dragging anywhere on it moves it. Widgets snap to a grid and to each other's edges; holding ⌥ drags freely.
    Resizing steps through the sizes the manifest declares (or a `free` range).
  - **Edit mode is also where you add widgets.** While editing, a glass **widget tray** slides up from the bottom of
    the screen listing every installed widget (from installed `.dgapp` packages), with a live preview in each declared
    size and a search field. Drag one out of the tray onto the desktop to place it, or click it to drop it where you
    right-clicked. The same widget can be placed more than once (each placement is its own instance with its own
    settings). Removing a widget (×) only takes it off the desktop; the extension stays installed and the widget goes
    back to the tray.
  - A small glass **Done** bar shows at the top while editing. Leaving edit mode — Done, Esc, right-click → **Done
    Editing**, or turning the mode off — **locks** every widget in place.
  - Positions are saved per screen (by display UUID), in points relative to that screen's frame, by the engine
    (`widgets/layout` in app support — not the extension's own storage), so they survive relaunches, display
    rearrangements and the extension being updated. If a screen is missing, its widgets wait for it rather than piling
    onto another display.
- **Getting widgets:** installing a `.dgapp` that declares `widgets` just makes them available — nothing appears on the
  desktop until you place it from the tray in edit mode. The island's Apps/extension list shows which installed
  packages provide widgets, with an "Add to Desktop…" shortcut that turns on DesktopGlass mode if needed and opens edit
  mode with the tray.
- **Bridge:** web widgets get `window.dg.widget` — `size`, `onResize`, `settings` (per-instance JSON edited from a
  form the manifest describes), `storage`, and `openIsland(path)`. Everything else goes through the normal
  permissions (network hosts, files, system stats).
- **Budget:** a `{ every }` widget is suspended between refreshes; a `live` one is throttled to 30 fps when not
  hovered and paused when its screen sleeps or a full-screen app covers it. The Widgets sheet shows each one's
  energy impact.
- **Wallpapers** stay built-in (image or looping video from a file or a link, `Wallpaper.swift`). A future
  `"wallpaper"` entry could let a package supply a live (rendered) wallpaper through the same overlay surface.

## 6. Developer tools

| Command | Does |
|---|---|
| `dg app new <name> --template web|terminal|wgpu|bevy|three` | scaffolds a package with a working example |
| `dg app run [path]` | installs as a dev app, launches it, live-reloads `ui/`, streams its logs to your terminal |
| `dg app check` | validates the manifest, permissions and binaries for both OSes |
| `dg app pack` | produces `Name.dgapp.zip` with a content hash |
| `dg app install <zip|url>` | shows the permission sheet, then installs |

A **Developer** section in Settings lists dev apps, their sockets and logs, and has a "Reload" button.

## 7. Security

- **Permissions** come from the manifest and are shown at install time. Changing them in an update asks again.
  Anything not granted is refused by the host, never by the SDK.
- **Process sandbox:** macOS — each app runs under a Seatbelt profile generated from its permissions (the engine
  already does this for agent CLIs): no network except declared hosts, file access limited to its package, data dir
  and user-picked files. Windows — AppContainer with matching capabilities.
- **Web views** load only `dgapp://<own id>/`; no remote scripts unless `network` allows the host.
- **Surfaces** are textures only: an app can't read the screen or other apps' surfaces. Overlay surfaces can't take
  key focus without a click.
- **Integrity:** packages carry a hash. Signing (developer keys, then an optional directory) comes after v1.

## 8. Phases

| Phase | What | Done when | Estimate |
|---|---|---|---|
| **A0 Spec** | `dgapp/1` types in `dg-proto`, manifest schema, this doc → reference docs | schema + docs generated from code | 2–3 days |
| **A1 Host + lifecycle** | engine launches/kills apps, per-app sockets, `app/*`, dev mode, Apps dropdown lists `.dgapp` | a hello-world Rust app launches from the sidebar and quits cleanly | 4–5 days |
| **A2 Top bar + terminal** | `topbar/*`, `terminal/*` (command + stream), tabs in the docked terminal | a ratatui app runs in a pane; an icon in the top bar opens it | 4–5 days |
| **A3 Web views** | `view/*`, `dgapp://` scheme, `window.dg` bridge, CSP | a three.js demo runs in the island | 3–4 days |
| **A4 GPU surfaces (macOS)** | IOSurface ring, present/frame/input, island + window placements | a wgpu spinning-cube app at 120 Hz with < 1 frame added latency | 6–8 days |
| **A5 Overlay + Bevy** | overlay placement, hit regions, `bevy_desktopglass`, gamepad | a Bevy 3D sample game playable from the Games library | 5–6 days |
| **A5w Widgets** | `widgets` manifest entry, overlay placement in DesktopGlass mode, edit mode, Widgets sheet, `window.dg.widget` | a third-party clock and a CPU-graph widget install from zip and survive a relaunch | 4–5 days |
| **A6 Sandbox + install** | Seatbelt profiles from permissions, permission sheet, `dg app pack/install` | an app with no `network` permission cannot open a socket | 4–5 days |
| **A7 Swift + C# SDKs** | generated bindings + samples | the A2 sample ported to both | 4–5 days |
| **A8 Windows** | DXGI surfaces, named pipes, AppContainer, WinUI rendering | the A5 Bevy game runs on Windows unchanged | follows WINDOWS_UI.md |

**Total for macOS (A0–A6): ~5–6 weeks** for one developer, after Rust Phase 5.

## 9. Open questions

1. **Distribution:** sideload only at first, or a curated directory inside the Apps dropdown?
2. **Agents + apps:** should apps be able to start an agent in their own terminal pane (with the user's CLI login)?
3. **Background apps:** how many `keep_alive` apps can run, and do they get CPU/energy limits?
4. **Native UI tree:** do we need it, or are HTML views + surfaces enough? Decide after A3 based on real apps.
5. **Godot/Unity:** worth an official plugin, or leave to the community once the surface API is stable?
/b