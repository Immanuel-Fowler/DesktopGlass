# To do

## Artifacts open as a browser page in dgview, not as an HTML file

**Requested 2026-11-08 (not the first time it's come up).** When an agent makes an artifact (a chart, diagram,
status page, anything visual), it should open as a page in the DesktopGlass browser (dgview / the artifact viewer)
with no `.html` file written to the project or the Desktop just to show it.

Today the flow is "write a file, then point the viewer at it":
- Agents write `something.html` into the project (e.g. `visualizations/desktopglass-status.html`) and then call
  `show_artifact` / `dg view` with that path.
- `Sources/DesktopGlass/Artifacts/CodexVisualizations.swift` turns Codex visualizations into page files under
  its pages dir (`writePage(for:in:)`) before opening them.

Want:
- `show_artifact` (MCP, `dg view`, the `desktopglass://` link) accepts the page content itself (HTML, or SVG/Markdown
  to wrap) and the viewer opens it as a tab, served from the engine/app in memory (or a private cache the user never
  sees), instead of from a file the agent had to create.
- The agent guide and the session hook tell agents to send content to the viewer instead of writing an HTML file.
- Saving a page as a file stays possible, but only when the user asks ("Save As…" in the viewer).
- Keep the existing rule: the viewer only opens on its own inside DesktopGlass's terminal (TERM_PROGRAM=DesktopGlass);
  elsewhere only when the user asks.

Not started. Don't change it until the user says to.
