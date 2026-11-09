#!/bin/bash
# Runs every test: the Rust engine (cargo test), the parity checks against the Python reference implementations
# (skipped if python3 is missing), and the app (swift test).
# Usage: scripts/test.sh [rust|parity|swift]
set -euo pipefail
cd "$(dirname "$0")/.."
status=0
want() { [[ ${1:-all} == all || ${1:-all} == "$2" ]]; }
if want "${1:-all}" rust; then
  echo "→ Rust engine"
  ( cd engine && cargo test --release --workspace ) || status=1
fi
if want "${1:-all}" parity; then
  echo "→ Parity with the Python reference (inventory, MCP)"
  ( cd engine && cargo build --release -p dg >/dev/null ) || status=1
  bash engine/tests/parity/parity.sh engine/target/release/dg || status=1
  bash engine/tests/parity/mcp-parity.sh engine/target/release/dg || status=1
fi
if want "${1:-all}" swift; then
  echo "→ Swift (HTML, descriptions, path security, game detection)"
  swift test || status=1
fi
exit $status
