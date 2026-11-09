#!/bin/bash
# Removes DesktopGlass: the app, the desktopglass-index skill, dgview, MCP registrations and settings.
#   ./uninstall.sh           keep the description.md files and _Organize Log on your Desktop
#   ./uninstall.sh --purge   delete those too
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/app.conf"
SKILL="$HOME/.claude/skills/desktopglass-index"
OLD_SKILL="$HOME/.claude/skills/desktop-index"     # name used before 1.0
PURGE=0; [[ "${1:-}" == "--purge" ]] && PURGE=1

echo "This removes DesktopGlass, its skill, dg and dgview, its MCP entries and its settings."
[[ $PURGE == 1 ]] && echo "--purge: it also deletes every description.md it wrote and ~/Desktop/_Organize Log."
read -r -p "Continue? [y/N] " answer
[[ "$answer" =~ ^[Yy]$ ]] || { echo "Cancelled."; exit 0; }

pkill -x DesktopGlass 2>/dev/null && sleep 1
# The engine's uninstaller removes both its own registrations/hooks and ones left by the old Python installer.
DG=""
for c in "$HOME/Applications/DesktopGlass.app/Contents/MacOS/dg" "$HOME/.local/bin/dg"; do [[ -x "$c" ]] && { DG="$c"; break; }; done
echo "→ Removing the MCP server from AI apps"
if [[ -n $DG ]]; then
  "$DG" mcp-install uninstall >/dev/null 2>&1 || echo "  (couldn't run the MCP uninstaller — remove 'desktopglass' from your MCP configs by hand)"
elif [[ -f "$SKILL/scripts/mcp_install.py" ]]; then
  /usr/bin/python3 "$SKILL/scripts/mcp_install.py" uninstall >/dev/null 2>&1 || echo "  (couldn't run the MCP uninstaller — remove 'desktopglass' from your MCP configs by hand)"
else
  echo "  (no uninstaller found — remove 'desktopglass' from your MCP configs by hand)"
fi
echo "→ Removing the app, skill, dg and dgview"
rm -rf "$HOME/Applications/DesktopGlass.app" "$SKILL"
if [[ -f "$OLD_SKILL/SKILL.md" ]] && grep -q "DesktopGlass" "$OLD_SKILL/SKILL.md"; then rm -rf "$OLD_SKILL"; fi
[[ -L "$HOME/.local/bin/dgview" ]] && rm -f "$HOME/.local/bin/dgview"
[[ -f "$HOME/.local/bin/dgview" ]] && grep -q 'dg" view' "$HOME/.local/bin/dgview" && rm -f "$HOME/.local/bin/dgview"
[[ -L "$HOME/.local/bin/dg" ]] && rm -f "$HOME/.local/bin/dg"
echo "→ Removing settings"
defaults delete "$BUNDLE_ID" >/dev/null 2>&1 || true
rm -rf "$HOME/Library/Application Support/DesktopGlass"

if [[ $PURGE == 1 ]]; then
  echo "→ Removing description.md files and _Organize Log"
  # only description.md files that DesktopGlass's format produced (they start with "# " and contain "## Contents")
  while IFS= read -r -d '' f; do
    grep -q "^## Contents" "$f" && rm -f "$f"
  done < <(find "$HOME/Desktop" -maxdepth 4 -name description.md -print0 2>/dev/null)
  rm -rf "$HOME/Desktop/_Organize Log"
fi
echo "Done. Launch-at-login entries disappear with the app; if one remains, remove it in System Settings → General → Login Items."
