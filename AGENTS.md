# AGENTS.md

This file provides guidance for coding agents working in this repository.
It applies to the entire `loopdiff-rs` project.

## Project Overview

Loopdiff is a Rust terminal UI for reviewing Git diffs in a human-to-AI
feedback loop. It presents a GitHub-inspired diff, lets a reviewer attach
comments to lines or ranges, and exports the review as structured Markdown.

The application is intentionally small, fast, keyboard-friendly, and visually
quiet. Preserve those qualities when making changes.

## Repository Structure

- `src/main.rs`: CLI parsing, terminal lifecycle, event loop, exit behavior.
- `src/git.rs`: Git command execution and diff loading.
- `src/model.rs`: unified-diff parsing, file/line models, syntax highlighting.
- `src/review.rs`: review annotations and Markdown parsing/serialization.
- `src/app.rs`: application state, layout, rendering, input, and UI tests.
- `README.md`: user-facing installation, usage, and key bindings.
- `dist-workspace.toml` and `.github/workflows/release.yml`: cargo-dist release
  and Homebrew publication configuration.
- `RELEASING.md`: maintainer release checklist and required GitHub secret.

## Development Commands

Run these commands from the repository root:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

Use `cargo fmt` after editing Rust files. Before handing work back, run all four
checks above. Add focused regression tests for behavioral or rendering fixes.

Useful manual runs:

```bash
cargo run --release
cargo run --release -- main
cargo run --release -- main..HEAD
cargo run --release -- old.py new.py
cargo run --release -- --staged
git diff | cargo run --release -- --stdin
cargo run --release -- -o review.md
cargo run --release -- --validate-review review.md
```

## Architecture and State

Keep parsing and persistence out of the UI layer:

- Diff semantics belong in `model.rs`.
- Markdown compatibility belongs in `review.rs`.
- Git process behavior belongs in `git.rs`.
- Interaction and rendering state belong in `app.rs`.

`App` owns independent state for the diff cursor, range selection, per-file
cursor positions, sidebar selection and scrolling, and the inline editor. Do
not infer one of these states solely from another when that would make focus or
selection ambiguous.

CLI input is intentionally unambiguous: `FROM..TO` means a Git revision range,
while two space-separated positional arguments mean two file paths compared via
`git diff --no-index`. Do not reinterpret two positional arguments as commits.

## UX Invariants

Treat the following behavior as intentional unless the task explicitly changes
it:

- `-` switches focus between the file tree and diff (`Tab` remains an alias).
  The active panel must have a visible focus accent.
- `?` opens modal keyboard help. The popup consumes input until `?` or `Esc`
  closes it.
- The sidebar shows comments as selectable children of their file.
- Sidebar scrolling works vertically and horizontally, but scrollbars are not
  rendered.
- Moving through sidebar entries skips folder-only rows and keeps the selected
  entry visible.
- File cursor positions survive file switches.
- `/` opens a Vim-style search prompt in the shared statusline and live-filters
  the file sidebar; do not add a separate search field or dialog.
- Accepting search keeps focus in the file explorer. Cancelling restores both
  the previous filter and the panel that was focused before search.
- `gg` jumps to the start and `{number}gg` jumps to the exact or nearest visible
  old/new line in the current diff.
- `c` starts or clears the diff-line range used for a comment; arrows or `j/k`
  extend it.
- `v` starts characterwise visual selection, `Shift+V` starts linewise visual
  selection, and `y` copies that visual selection without diff markers.
- A new range comment is anchored and rendered below the visually lowest line,
  regardless of selection direction.
- Comments may overlap or be nested. Pressing Enter on an exact comment anchor
  edits that comment; pressing Enter elsewhere inside its range creates a new
  comment.
- Inline comments remain visually distinct from diff text and begin at the code
  column.
- In the comment editor, `Enter` saves, `Shift+Enter` inserts a newline, and
  `Esc` cancels.
- On the target terminal, `Shift+Enter` arrives as terminal line feed
  (`Ctrl+J`). Handle that event directly; do not enable keyboard enhancement
  protocols solely for this shortcut.
- Editor cursor movement and editing must remain safe at UTF-8 boundaries.
- Add/remove backgrounds extend to the right edge of the diff viewport.
- Avoid decorative headers, redundant badges, and visible scrollbar chrome.

Mouse and keyboard behavior should agree. A mouse-selected sidebar item must
leave the sidebar in a state where arrow navigation continues naturally.

## Review Markdown Contract

The Markdown output is both human-readable and machine-readable. AI agents are
expected to consume it directly. Preserve the format produced by
`review::format_review` and accepted by `review::parse_review`.

Important rules:

- The document starts with mandatory YAML front matter, followed by a
  `# Review: ...` heading. The front matter contains the format version, diff
  identity, canonical agent instructions, validation command, and response
  template.
- Treat the front matter as a self-contained agent handoff contract. Preserve
  it exactly; `review::parse_review` rejects a missing or modified contract.
- The only supported format is v1. Unversioned and legacy documents are
  intentionally rejected.
- Each thread includes its path, old/new range, fenced `diff` excerpt, status,
  and flat chronological message list.
- Messages carry stable IDs and a `human` or `assistant` role. Either role may
  start a thread. They may also carry an author name; human names come from Git
  configuration and assistant agents must choose a short name as instructed by
  the front matter. Preserve existing authors.
- Anchor metadata preserves exact inline placement.
- Front matter stores resolved endpoints and a SHA-256 of the reviewed patch.
- Existing `-o` files are loaded as sessions and saved back on exit.
- `--validate-review` must reject unsupported or malformed review files with a
  useful error.
- Exit code `10` means the review contains feedback; exit code `0` means it is
  empty.

Do not silently change this contract. Any incompatible evolution must increment
the format version and receive its own parser; do not add heuristic legacy
parsing.

## Rendering Guidelines

- Use Ratatui styles and layout primitives; do not emit terminal escape codes
  from widgets.
- Account for Unicode display width with `unicode-width` when cropping or
  padding content.
- Keep syntax foreground colors independent from add/remove background colors.
- Prefer subtle separators and background changes over extra labels or icons.
- Rendering must tolerate narrow and short terminal sizes without panicking.
- When changing coordinates, widths, or panel heights, update the TestBackend
  assertions in `src/app.rs`.

## Input and Terminal Safety

- Restore raw mode, the alternate screen, mouse capture, and cursor visibility
  on every normal error path.
- Do not add terminal-protocol workarounds unless they are necessary and tested
  in the supported terminal environment.
- Modified key events must not accidentally insert their character into the
  editor. In particular, control-key events need explicit handling or must be
  ignored.

## Change Discipline

- Keep changes scoped and preserve the existing compact design.
- Avoid unrelated dependency additions. Prefer the standard library and current
  dependencies for small features.
- Never discard user changes in a dirty worktree.
- Update `README.md` whenever CLI behavior, key bindings, or user-visible
  workflows change.
- Do not commit generated review Markdown, build output, or `target/`.

For a bug fix, write a regression test that fails for the reported behavior and
passes after the fix. For layout work, prefer a TestBackend assertion over a
snapshot that is sensitive to unrelated styling.
