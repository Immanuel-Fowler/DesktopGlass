---
name: desktopglass-index
description: DesktopGlass's indexer. Maintain the description.md file in each ~/Desktop folder and the _Organize Log/desktop-index.md summary that the DesktopGlass overlay reads. Use when asked to index, re-index, describe or summarize the Desktop, when DesktopGlass runs you headless, or when the user wants to answer the open questions in _Organize Log/questions.md.
---

# Desktop Index

You keep a short `description.md` in each Desktop folder up to date. The DesktopGlass overlay shows them, and the `dg inventory` command assembles them into `_Organize Log/desktop-index.md`. **Be token-frugal:** `dg inventory` has already gathered what you need, so don't explore on your own.

Paths below are relative to `~/Desktop`.

## Mode

- **Headless**: the prompt says "headless" (the schedule or the overlay's Re-index button). Never wait for an answer. Make your best call, end any blurb you're unsure of with ` (?)`, and append the question to `_Organize Log/questions.md`.
- **Interactive**: a person is in the session. When a folder's purpose is unclear, ask with AskUserQuestion (batch up to 4 questions per call) instead of guessing.

## Steps

1. **Interactive only:** if `_Organize Log/questions.md` has `- [ ]` items, ask them (up to 4 per call), apply the answers to the affected description.md files, and delete the resolved lines. Never ask setup or preference questions; those live in the DesktopGlass app.
2. Run `~/.local/bin/dg inventory ~/Desktop scan` (add `--all` only if the user asks for a full rewrite). It prints compact JSON listing **only folders that need work**:
   - `{"n":…, "keep":…}` → unchanged entry: copy that blurb exactly.
   - Other entries are new or changed. `k` is the kind (group = described subfolder, project, folder, package, file). Hints: `stack`, `readme`, `desc` (package.json), `head` (first line of a text file), `top` (first names inside), `summary` (the subfolder's own summary), `items`, `size`, `mod`, `old` (previous blurb).
   - `private: 1` → describe from names only; open nothing inside.
   - `prefs` → the user's preferences from DesktopGlass Settings (`_Organize Log/preferences.json`): `style` is `terse` (short fragments) or `sentence` (one full sentence per blurb), `flagStatus` says whether to mark projects active / backup / archived, `notes` is free-form context from the user. Follow them; don't read the JSON file yourself. Private and skipped folders are already applied by `dg inventory`.
   - `removed` → drop those bullets. `more` → mention the overflow in Notes. `removed_folders` → nothing to do.
   - If `containers` is empty, go to step 4.
3. For each listed folder, write `<path>/description.md` in the format below.
   - If the file exists and only a few entries changed, use Edit on just those bullets (and the summary, if it no longer fits) rather than rewriting the file.
   - Otherwise Write the whole file.
   - Open a file yourself only when an entry has no hints and its name says nothing. Read at most its first 40 lines, and do this no more than 5 times per run.
4. Run `~/.local/bin/dg inventory ~/Desktop commit`. It saves the change manifest and rebuilds `desktop-index.md`; never write that file yourself.
5. Finish with one line: how many folders you updated and how many questions you added.

Never run ls, find or Glob over the Desktop, and never read media or binaries. Don't move, rename or delete anything, and don't create description.md inside a project folder (the scan never lists those). Edit only description.md files and questions.md; never edit preferences.json (the app owns it).

## description.md format

```
# <Folder name>
> <one-sentence summary, ≤ 120 chars>
Tags: <3–6 lowercase tags, comma-separated>

## Contents
- **<subfolder or project>/** — <what it is, ≤ 90 chars>
- **<file>** — <what it is>

## Notes
<optional: overflow ("plus 40 more screenshots"), status, anything useful>
```

- Write one bullet per entry in the scan's order, and put `/` after directory names. The overlay parses `**name**` followed by ` — blurb`, so keep that exact shape.
- For projects, say what it does, then the stack, e.g. `Client portal for FFS · Next.js, Supabase`.
- If an entry looks like a backup, copy or duplicate of a sibling (`copy`, `backup`, `-2`, a matching zip), say so.
- Follow `prefs` for tone and length. Keep the user's own names for things.

## questions.md format

```
- [ ] 2026-11-04 · Projects/Other/bruh — What is this folder for? It only holds easy-rag-2.
```
