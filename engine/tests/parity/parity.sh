#!/bin/bash
# Parity: runs `python3 inventory.py` and `dg inventory` on the same fixture Desktops and diffs stdout, exit
# codes, .manifest.json and desktop-index.md. Every run works on a temporary copy; nothing real is written.
#
#   parity.sh [path/to/dg] [Desktop root to clone and compare read-only (scan/status/check) ...]
#
# Exit 0 when everything matches. DG_PARITY_KEEP=1 keeps the work folder.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
engine="$(cd "$here/../.." && pwd)"
repo="$(cd "$engine/.." && pwd)"
DG="${1:-$engine/target/release/dg}"; shift || true
PY="${PYTHON:-python3}"
INV="$here/reference/inventory.py"
DG="$(cd "$(dirname "$DG")" 2>/dev/null && pwd)/$(basename "$DG")"
[ -x "$DG" ] || { echo "parity: no dg binary at $DG (cargo build --release)"; exit 2; }
command -v "$PY" >/dev/null || { echo "parity: $PY not found; skipping"; exit 0; }

work="$(mktemp -d "${TMPDIR:-/tmp}/dg-parity.XXXXXX")"
[ "${DG_PARITY_KEEP:-}" = 1 ] || trap 'rm -rf "$work"' EXIT
fails=0; checks=0

# Normalise the two timestamps commit writes, so runs a second apart still compare.
norm() { sed -E -e 's/"updated":"[^"]*"/"updated":"T"/' -e 's/^_Updated [A-Z][a-z]{2} [0-9]{2}, [0-9]{4} [0-9]{2}:[0-9]{2} [AP]M · /_Updated T · /' "$1"; }

# compare LABEL — runs "$@" with both tools (the word INV stands for the tool) and diffs stdout + exit code.
compare() {
  local label="$1"; shift
  local a="$work/out.py" b="$work/out.dg" ea eb args_py=() args_dg=()
  for x in "$@"; do
    if [ "$x" = INV ]; then args_py+=("$PY" "$INV"); args_dg+=("$DG" inventory); else args_py+=("$x"); args_dg+=("$x"); fi
  done
  "${args_py[@]}" >"$a" 2>/dev/null; ea=$?
  "${args_dg[@]}" >"$b" 2>/dev/null; eb=$?
  checks=$((checks + 1))
  if [ "$ea" != "$eb" ] || ! cmp -s "$a" "$b"; then
    fails=$((fails + 1))
    echo "✗ $label (exit python=$ea dg=$eb)"
    diff -u "$a" "$b" | head -40
  else
    echo "✓ $label"
  fi
}

# commit with each tool and compare what it writes.
compare_commit() {
  local name="$1" root="$2" log="$2/_Organize Log"
  "$PY" "$INV" "$root" commit >"$work/c.py" 2>&1; local ea=$?
  for f in desktop-index.md .manifest.json; do [ -f "$log/$f" ] && norm "$log/$f" >"$work/py.$f" || : >"$work/py.$f"; done
  "$DG" inventory "$root" commit >"$work/c.dg" 2>&1; local eb=$?
  for f in desktop-index.md .manifest.json; do [ -f "$log/$f" ] && norm "$log/$f" >"$work/dg.$f" || : >"$work/dg.$f"; done
  for f in desktop-index.md .manifest.json; do
    checks=$((checks + 1))
    if cmp -s "$work/py.$f" "$work/dg.$f"; then echo "✓ $name commit → $f"; else
      fails=$((fails + 1)); echo "✗ $name commit → $f"; diff -u "$work/py.$f" "$work/dg.$f" | head -40; fi
  done
  checks=$((checks + 1))
  if [ "$ea" = "$eb" ] && cmp -s "$work/c.py" "$work/c.dg"; then echo "✓ $name commit stdout"; else
    fails=$((fails + 1)); echo "✗ $name commit stdout (exit python=$ea dg=$eb)"; diff -u "$work/c.py" "$work/c.dg" | head -20; fi
}

read_only() {
  local name="$1" root="$2"
  compare "$name status" INV "$root" status
  compare "$name scan" INV "$root" scan
  compare "$name scan --all" INV "$root" scan --all
  compare "$name check" INV "$root" check
}

full_cycle() {
  local name="$1" root="$2"
  read_only "$name" "$root"
  compare_commit "$name" "$root"
  read_only "$name after commit:" "$root"
  # Mutate: an edited file (new mtime + size), a new file, a removed entry, a removed folder.
  local f; f="$(find "$root" -mindepth 2 -maxdepth 2 -type f ! -name description.md ! -path '*/_Organize Log/*' | sort | head -1)"
  [ -n "$f" ] && { printf 'changed\n' >>"$f"; touch -t 203001010101 "$f"; }
  local g; g="$(find "$root" -mindepth 1 -maxdepth 1 -type d ! -name '_Organize Log' | sort | head -1)"
  [ -n "$g" ] && printf 'new\n' >"$g/zz-new-file.md"
  local h; h="$(find "$root" -mindepth 2 -maxdepth 2 -type d ! -path '*/_Organize Log*' | sort | tail -1)"
  [ -n "$h" ] && rm -rf "$h"
  read_only "$name after edits:" "$root"
  compare_commit "$name second" "$root"
}

echo "== usage"
compare "no arguments" INV
compare "one argument" INV "$work"
compare "unknown command" INV "$work" frobnicate
compare "missing Desktop status" INV "$work/does-not-exist" status
compare "missing Desktop scan" INV "$work/does-not-exist" scan
compare "missing Desktop check" INV "$work/does-not-exist" check

echo "== description fixtures (as Desktop folders)"
d="$work/descs"; mkdir -p "$d"
for md in "$engine"/tests/golden/descriptions/*.md; do
  n="$(basename "$md" .md)"; mkdir -p "$d/$n"; cp "$md" "$d/$n/description.md"; printf 'x' >"$d/$n/notes.txt"
done
full_cycle descriptions "$d"

echo "== golden Desktops"
for src in "$engine"/tests/golden/desktops/*/; do
  n="$(basename "$src")"; cp -R "$src" "$work/$n"; full_cycle "$n" "$work/$n"
done

echo "== generated Desktops"
"$here/make-fixtures.sh" "$work/gen" >/dev/null
for n in edge legacy empty; do full_cycle "$n" "$work/gen/$n"; done

echo "== known divergence: inputs inventory.py crashes on (dg must succeed)"
for cmd in scan check status; do
  checks=$((checks + 1)); "$DG" inventory "$work/gen/pycrash" $cmd >"$work/out.dg" 2>&1; e=$?
  if [ "$cmd" = check ] && [ $e = 1 ] || [ $e = 0 ]; then echo "✓ pycrash dg $cmd (exit $e)"; else fails=$((fails + 1)); echo "✗ pycrash dg $cmd (exit $e)"; fi
done
# Tilde expansion and a relative path for the root argument.
pushd "$work/gen" >/dev/null; compare "relative root" INV edge status; popd >/dev/null
HOME="$work/gen" compare "tilde root" INV "~/legacy" status

for extra in "$@"; do
  n="real-$(basename "$extra")"
  echo "== $n (read-only, on an APFS clone of the whole folder)"
  cp -cRp "$extra" "$work/$n" 2>/dev/null || cp -Rp "$extra" "$work/$n" 2>/dev/null
  read_only "$n" "$work/$n"
done

echo
echo "parity: $((checks - fails))/$checks matched"
[ "$fails" = 0 ]
