#!/bin/bash
# Builds build/DesktopGlass.app from the Swift package (needs Xcode or the Command Line Tools).
set -euo pipefail
cd "$(dirname "$0")/.."
source ./app.conf
swift build -c release
APP="build/DesktopGlass.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp .build/release/DesktopGlass "$APP/Contents/MacOS/DesktopGlass"

# dg, the Rust engine (engine/). Universal (arm64 + x86_64) when both Rust targets are installed
# (`rustup target add aarch64-apple-darwin x86_64-apple-darwin`); otherwise just this Mac's architecture.
export PATH="$PATH:$HOME/.cargo/bin"
if command -v cargo >/dev/null 2>&1; then
  ENGINE="$PWD/engine"
  TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
  INSTALLED="$(rustup target list --installed 2>/dev/null || true)"
  HAVE_ALL=1
  for t in "${TARGETS[@]}"; do grep -qx "$t" <<<"$INSTALLED" || HAVE_ALL=0; done
  if [ "$HAVE_ALL" = 1 ]; then
    echo "→ Building dg (universal)"
    SLICES=()
    for t in "${TARGETS[@]}"; do
      cargo build --release --locked --manifest-path "$ENGINE/Cargo.toml" --target "$t" --bin dg
      SLICES+=("$ENGINE/target/$t/release/dg")
    done
    lipo -create -output "$APP/Contents/MacOS/dg" "${SLICES[@]}"
  else
    echo "→ Building dg ($(uname -m) only: install both Rust targets for a universal binary)"
    cargo build --release --locked --manifest-path "$ENGINE/Cargo.toml" --bin dg
    cp "$ENGINE/target/release/dg" "$APP/Contents/MacOS/dg"
  fi
else
  echo "  (cargo not found: the app is built without the dg engine — install Rust from https://rustup.rs)"
fi

# JavaScript shared with other UIs (shared/web/*.js): the visual editor, browser tools, Markdown visuals.
cp -R shared/web "$APP/Contents/Resources/web"

# Mermaid (MIT) for diagrams in Markdown Preview: bundled so diagrams work offline. Without npm, the app
# falls back to loading the same pinned version from a CDN.
MERMAID_VERSION=11.4.1
MERMAID_DIR=.build/mermaid-$MERMAID_VERSION
if [ ! -f "$MERMAID_DIR/mermaid.min.js" ] && command -v npm >/dev/null 2>&1; then
  echo "→ Fetching Mermaid $MERMAID_VERSION"
  mkdir -p "$MERMAID_DIR"
  ( cd "$MERMAID_DIR" && npm pack "mermaid@$MERMAID_VERSION" --silent >/dev/null 2>&1 \
      && tar -xzf mermaid-*.tgz package/dist/mermaid.min.js package/LICENSE \
      && mv package/dist/mermaid.min.js package/LICENSE . && rm -rf package mermaid-*.tgz ) \
    || echo "  (couldn't fetch Mermaid — diagrams will load it from the web)"
fi
if [ -f "$MERMAID_DIR/mermaid.min.js" ]; then
  cp "$MERMAID_DIR/mermaid.min.js" "$APP/Contents/Resources/"
  cp "$MERMAID_DIR/LICENSE" "$APP/Contents/Resources/mermaid-LICENSE.txt" 2>/dev/null || true
fi
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>DesktopGlass</string>
  <key>CFBundleDisplayName</key><string>DesktopGlass</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>DesktopGlass</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSDesktopFolderUsageDescription</key><string>DesktopGlass shows your Desktop folders and their descriptions.</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
      <key>CFBundleURLSchemes</key><array><string>desktopglass</string></array>
    </dict>
  </array>
  <key>NSAppleEventsUsageDescription</key><string>DesktopGlass opens Terminal so Claude Code can ask you its indexing questions.</string>
</dict>
</plist>
PLIST
# Nested executables are signed before the bundle, or the bundle's signature doesn't verify.
[ -f "$APP/Contents/MacOS/dg" ] && { codesign --force --sign - "$APP/Contents/MacOS/dg" >/dev/null 2>&1 || true; }
codesign --force --sign - "$APP" >/dev/null 2>&1 || true
echo "Built $APP ($VERSION)"
