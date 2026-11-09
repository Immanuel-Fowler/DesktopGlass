# Python reference implementations

The scripts DesktopGlass shipped before the Rust engine replaced them. They are **not installed**; they're kept only
so the parity harnesses (`../parity.sh`, `../mcp-parity.sh`) can keep proving that `dg inventory` and `dg mcp`
produce byte-identical output. Delete this folder (and the harnesses) once the Rust versions have had a release.
The shell scripts and other Python files (reindex.sh, common.sh, local_index.py, ask.sh, login.sh, autoshow.py,
session_context.py, dgview) were removed outright; their behaviour is covered by Rust tests in dg-index and dg-install.
