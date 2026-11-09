#!/bin/bash
# Builds the generated fixture Desktops (edge cases that are awkward to keep as files: symlinks, odd names,
# big files, invalid UTF-8, 40+ entries) into $1/<name>/. The static ones live in ../golden/desktops/.
set -euo pipefail
out="$1"
mkdir -p "$out"

# ---- edge: caps, odd names, symlinks, depth, encodings ----
d="$out/edge"; rm -rf "$d"; mkdir -p "$d"
cd "$d"
mkdir -p Many "Deep/A/B/C/D" "Deep/A/B/pkg.bundle" Weird "Links" "_Organize Log"
for i in $(seq 1 25); do printf 'x%.0s' $(seq 1 "$i") > "Many/file$i.jpg"; done
for i in $(seq 1 9); do printf 'p' > "Many/doc$i.pdf"; done
for i in $(seq 1 6); do mkdir -p "Many/sub$i"; done
for i in $(seq 1 4); do printf 'n' > "Many/noext$i"; done
for i in 1 2 3; do printf z > "Many/zz$i.png"; done; printf m > Many/zz.mov; printf m > Many/zz.PNG.png
printf 'k' > Many/a.KEY; printf 'z' > Many/b.tar.gz; printf 'w' > "Many/.hidden"; printf 'l' > 'Many/~$lock.docx'
printf '# Many\n> Lots of things\n\n## Contents\n- **file1.jpg** — one\n- **sub1/** — first sub\n' > Many/description.md
printf 'deep' > Deep/A/B/C/D/leaf.txt
printf 'deep' > Deep/A/B/C/c.txt
printf '# C\n> The deepest group\n' > Deep/A/B/C/description.md
printf '# B\n> Middle\n' > Deep/A/B/description.md
printf 'é' > "Weird/café.txt"                                  # NFC
printf 'e\xcc\x81' > "Weird/cafe$(printf '\xcc\x81') nfd.md"    # NFD name
printf '\xf0\x9f\x98\x80 emoji\n' > "Weird/😀 smile.txt"
printf 'q' > 'Weird/quote"back\slash.txt'
printf 't' > "Weird/tab	name.txt"
printf 'n' > "Weird/new
line.txt"
printf 'i' > "Weird/Icon"$'\r'
printf 'Ünïcödé first line here\n' > "Weird/UPPER.TXT"
printf '\xff\xfe\x00bad bytes then text\nreal line\n' > "Weird/binary.txt"
printf '\r\n\r\nCRLF title line\r\nsecond\r\n' > "Weird/crlf.md"
printf '{\n"a":1\n}\nimport x\n<div>\n---\n| t |\n### Real heading here\n' > "Weird/markup.md"
head -c 2100000 /dev/zero | tr '\0' 'a' > "Weird/big.txt"
printf 'abc' > "Weird/tiny.md"
printf 'x' > "Weird/dot."
printf '# Weird\r\n> Odd names\r\nTags: a, B ,, c\r\n\r\n## Contents\r\n- **café.txt** — accent\r\n* **tiny.md**: tiny\r\n- **dot.** - dotted\r\n\r\n## Notes\r\nKeep these.\r\n> quoted note\r\n' > Weird/description.md
mkdir -p "$out/outside/Secret"; printf 's' > "$out/outside/Secret/s.txt"
ln -s "$out/outside/Secret" "Links/SecretLink"
ln -s "../Many/file1.jpg" "Links/file-link.jpg"
ln -s "/nonexistent/dg-broken" "Links/broken"
ln -s "$out/outside/Secret" "TopLink"
mkdir -p "Links/Real"; printf 'r' > Links/Real/r.txt
mkdir -p "Proj/badjson"; printf '{nope' > Proj/badjson/package.json
mkdir -p "Proj/pyproj"; printf '[project]\n' > Proj/pyproj/pyproject.toml
mkdir -p "Proj/rust/.git"; printf '[package]\n' > Proj/rust/Cargo.toml; printf '\n\n## \n**Rust** thing: fast\nsecond\n' > Proj/rust/README.md
mkdir -p "Proj/dotnet"; printf 'x' > Proj/dotnet/App.sln; printf 'y' > Proj/dotnet/README.txt
mkdir -p "Proj/static"; printf '<html>' > Proj/static/index.html; printf 'z' > Proj/static/style.css
mkdir -p "Proj/go"; printf 'module x' > Proj/go/go.mod; printf 'readme' > Proj/go/readme.md
mkdir -p "Proj/manydeps"; printf '{"dependencies":{"zod":"1","three":"1","stripe":"1","openai":"1","next":"1","react":"1","vue":"1","express":"1"},"description":5}' > Proj/manydeps/package.json
printf '%s' "$(printf 'Long description %.0s' $(seq 1 20))" > /dev/null
mkdir -p "Proj/longdesc"; printf '{"description":"%s"}' "$(printf 'word%.0s ' $(seq 1 60))" > Proj/longdesc/package.json
printf 'Grp' > "Proj/plain.txt"
printf 'unicode\n' > "Proj/Ärger.txt"; printf 'ascii\n' > "Proj/apple.txt"; printf 'Z\n' > "Proj/Zeta.txt"; printf 'b\n' > "Proj/_under.txt"
printf 'Σ' > "Proj/ΣΑΣ.txt"
mkdir -p "Empty"
printf '   \n' > Empty/description.md
mkdir -p "NoSummary"; printf 'x' > NoSummary/f.txt; printf '# Only title\n\n## Contents\n- **f.txt**\n' > NoSummary/description.md
printf 'loose' > "aaa.txt"; printf 'L' > "Zed.md"; mkdir -p "Thing.app"
printf '{"style":"terse","private":"Proj","skip":["Deep/A/B/C/"],"notes":42}' > "_Organize Log/preferences.json"
printf -- '- [ ] one\n- [ ] two\n- [ ] three\n' > "_Organize Log/questions.md"
# a stale manifest: a folder that no longer exists, a non-string sig, and an entry that is gone
printf '{"version":1,"updated":"x","containers":{"Gone/Folder":{"entries":{}},"Proj":{"entries":{"plain.txt":5,"removed.txt":"f1:1"}},"Many":{"entries":{"zzz-removed":"f1:1","aaa-removed":"f1:1"}}}}' > "_Organize Log/.manifest.json"
touch -h -t 202001020304.05 Links/broken 2>/dev/null || true
touch -t 201901010000 Weird/tiny.md

# ---- legacy: preferences.md, skip by name, a 2-question log ----
d="$out/legacy"; rm -rf "$d"; mkdir -p "$d"
cd "$d"
mkdir -p "Keep/Sub" "Games/Switch" "GamesArchive" "Old/x" "Private Stuff/inner" "_Organize Log"
printf 'k' > Keep/k.txt; printf 's' > Keep/Sub/s.txt; printf 'g' > Games/Switch/z.nsp; printf 'a' > GamesArchive/a.zip
printf 'o' > Old/x/o.txt; printf 'p' > "Private Stuff/inner/p.txt"; printf 'pp' > "Private Stuff/top.txt"
printf '# Prefs\n- skip: `Games/`, Old\n* Private : Private Stuff/ ,\n  style: detailed\nnotes:   be brief   \n' > "_Organize Log/preferences.md"
printf -- '- [ ] only one\n' > "_Organize Log/questions.md"

# ---- empty: a Desktop with nothing on it ----
d="$out/empty"; rm -rf "$d"; mkdir -p "$d"

# ---- pycrash: inputs inventory.py crashes on (AttributeError/TypeError); dg treats them as empty ----
d="$out/pycrash"; rm -rf "$d"; mkdir -p "$d"
cd "$d"
mkdir -p "Proj/node" "Proj/deps"
printf '[1,2]' > Proj/node/package.json
printf '{"dependencies":["react"]}' > Proj/deps/package.json
