#!/bin/bash
# Installs the desktopglass-index skill, builds DesktopGlass, and launches it. Safe to re-run.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
source "$HERE/app.conf"
SKILL="$HOME/.claude/skills/desktopglass-index"
OLD_SKILL="$HOME/.claude/skills/desktop-index"     # name used before 1.0

echo "→ DesktopGlass $VERSION"
echo "→ Installing skill → $SKILL"
mkdir -p "$SKILL"
cp "$HERE/skill/desktopglass-index/SKILL.md" "$SKILL/SKILL.md"
# Everything the old Python/zsh scripts did is now in the engine (`dg`, inside the app); remove the old copies.
rm -rf "$SKILL/scripts"
mkdir -p "$HOME/.local/bin"

# Earlier builds installed the skill as "desktop-index". Remove it only if it's DesktopGlass's own copy.
if [[ -f "$OLD_SKILL/SKILL.md" ]] && grep -q "DesktopGlass" "$OLD_SKILL/SKILL.md" && [[ -f "$OLD_SKILL/scripts/reindex.sh" ]]; then
  echo "→ Removing the old desktop-index skill (renamed to desktopglass-index)"
  rm -rf "$OLD_SKILL"
fi

echo "→ Building DesktopGlass"
"$HERE/scripts/build-app.sh"

echo "→ Installing to ~/Applications"
mkdir -p "$HOME/Applications"
pkill -x DesktopGlass 2>/dev/null || true
for _ in $(seq 1 50); do pgrep -x DesktopGlass >/dev/null || break; sleep 0.1; done   # let it quit before relaunching
rm -rf "$HOME/Applications/DesktopGlass.app"
cp -R "$HERE/build/DesktopGlass.app" "$HOME/Applications/"
ln -sf "$HOME/Applications/DesktopGlass.app/Contents/MacOS/dg" "$HOME/.local/bin/dg"   # the engine, for agents and scripts
# `dgview file.html` from any terminal (older instructions mention it); `dg view` does the work.
rm -f "$HOME/.local/bin/dgview"
printf '#!/bin/sh\nexec "$HOME/.local/bin/dg" view "$@"\n' > "$HOME/.local/bin/dgview" && chmod +x "$HOME/.local/bin/dgview"
# Point existing MCP registrations and hooks at the engine (only apps where DesktopGlass is already installed);
# this also migrates registrations made by the old Python installer.
"$HOME/.local/bin/dg" mcp-install refresh >/dev/null 2>&1 || true
open "$HOME/Applications/DesktopGlass.app"

cat <<MSG

Done. DesktopGlass $VERSION is in your menu bar.
Press ⌥⌘D, open ⚙️ Settings to choose a backend and preferences, then click Re-index.
MSG
