#!/bin/bash
# MCP parity: `python3 mcp_server.py` vs `dg mcp` on the same fixture Desktop, settings and fake app; diffs every
# response, every control-channel request and the resulting files. Also runs mcp_install.py's self-test against
# `dg mcp`. Everything happens under a temporary HOME; see mcp_parity.py for details.
#
#   mcp-parity.sh [path/to/dg]        (default: engine/target/release/dg)
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
PY="${PYTHON:-python3}"
command -v "$PY" >/dev/null || { echo "mcp-parity: $PY not found; skipping"; exit 0; }
exec "$PY" -I "$here/mcp_parity.py" "${1:-$here/../../target/release/dg}"
