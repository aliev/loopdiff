# loopdiff

A fast Rust/Ratatui terminal UI for reviewing Git diffs in a human-to-AI
feedback loop.

Loopdiff keeps review comments beside the diff, exports them as compact
self-describing Markdown, and watches that file for replies from an AI agent.

![Loopdiff showing an inline human and AI review thread](https://raw.githubusercontent.com/aliev/loopdiff/main/docs/assets/loopdiff-demo.png)

> Loopdiff is currently pre-1.0. The review format is versioned, but CLI and UI
> details may still evolve.

## Install

### Homebrew

```bash
brew install aliev/tap/loopdiff
```

### Installer

macOS and Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/aliev/loopdiff/releases/latest/download/loopdiff-installer.sh | sh
```

Windows PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/aliev/loopdiff/releases/latest/download/loopdiff-installer.ps1 | iex"
```

### Build from source

Loopdiff requires Git and Rust 1.85 or newer. Install it from a local clone:

```bash
cargo install --path . --locked
```

To build without installing:

```bash
cargo build --release --locked
./target/release/loopdiff --help
```

## Quick start

```bash
# HEAD → working tree
loopdiff

# HEAD → index
loopdiff --staged

# HEAD^ → working tree (same semantics as git diff HEAD^)
loopdiff 'HEAD^'

# Exact commit-to-commit diff
loopdiff e7f53..39fb

# Review exactly the latest commit
loopdiff 'HEAD^..HEAD'

# Diff two files outside Git history
loopdiff old.py new.py

# External unified diff
git diff | loopdiff --stdin

# Persistent review session
loopdiff -o review.md

# Strict format validation
loopdiff --validate-review review.md
```

Git inputs follow `git diff` semantics: no revision compares `HEAD` with the
working tree, one revision compares that commit with the working tree, and a
`FROM..TO` argument compares the two commit snapshots. Two space-separated
arguments are treated as file paths and compared with `git diff --no-index`.
Exit code `10` means the review contains feedback; exit code `0` means it is
empty.

Give the resulting file to an agent with a short prompt such as:

> Review comments are in `review.md`. Address them and reply in the same file.

The file contains the response protocol and validation command. While Loopdiff
is open, valid external replies are loaded automatically without overwriting
local edits.

## UI

- GitHub-style folder/file sidebar with fuzzy filtering.
- Separate old/new gutters and per-hunk syntax highlighting.
- Full-row add/remove backgrounds, cursor, and visual ranges.
- Inline, multiline review threads.
- Human messages and AI replies use distinct visual cards.
- Comments appear as navigable children beneath their files.
- Markdown session import/export with strict v1 validation.
- Mouse support plus Vim-style navigation.
- VS Code-style sticky hunk headers that appear only after their original row scrolls away.

Clipboard yanking uses OSC 52 and therefore requires a terminal that permits
OSC 52 clipboard access.

## Keys

| Key | Action |
|---|---|
| `-` | toggle focus between sidebar and diff |
| `Tab` | alternate alias for pane focus |
| `?` | open keyboard help popup |
| `j/k`, arrows | move |
| `G`, `gg`, `42gg` | end/start/jump to line |
| `c`, then `j/k` or arrows | select diff lines for a comment |
| `Space` | mark the current file viewed/unviewed and advance |
| `v`, then arrows or `h/j/k/l` | characterwise visual selection |
| `Shift+V`, then `j/k` or arrows | linewise visual selection |
| `y` | yank visual selection to the system clipboard |
| `Shift+Y` | copy open review comments as compact plain text for an LLM |
| `Enter`, double click | comment/edit |
| `r` | reply to the thread under the cursor |
| `Enter`, `Esc` | save/cancel editor |
| `Shift+Enter` | newline in comment |
| `[` / `]` | previous/next thread |
| `Ctrl+U` / `Ctrl+D` | half-page up/down |
| `/` | search files through the Vim-style statusline |
| `h/l`, left/right | horizontally scroll the file sidebar |
| `d` / `u` | delete thread / undo deletion |
| `q` | finish and hand off Markdown |

Search uses live filtering in the file tree. `Enter` accepts the query and
keeps focus in the file explorer, `Esc` restores the previous filter and panel,
and `Ctrl+U` clears the current prompt. Submit an empty search to clear an
accepted filter.

Viewed-file progress is stored in the v1 review YAML when `-o` is used and is
restored when that review session is opened again.

## Review format v1

Review Markdown is intentionally readable by humans and structured for LLMs.
No unversioned or legacy format is accepted.

````markdown
---
loopdiff:
  format_version: 1
  document: review
  response_protocol: loopdiff-response/v1
  diff:
    from: { kind: commit, label: HEAD^, oid: "..." }
    to: { kind: commit, label: HEAD, oid: "..." }
    patch_sha256: "..."
  agent:
    instructions:
      - Read every review thread.
      - Address the feedback in the working tree.
      - Append an assistant message to each addressed thread.
      - Choose a short name for yourself and use it as the author of every assistant message.
      - Do not modify thread metadata, selected diffs, or existing messages.
      - Explain if a request was not implemented.
      - Validate this file before finishing.
    validation:
      command: loopdiff --validate-review <this-file>
    response:
      role: assistant
      insert_before: "<!-- /loopdiff:thread -->"
      template: |-
        <!-- loopdiff:message {"id":"m-NEW","role":"assistant","author":"AI_NAME"} -->
        **AI_NAME**

        Describe what was changed or explain why it was not changed.
        <!-- /loopdiff:message -->
---

# Review: `HEAD^ → HEAD`

## `src/auth.rs` · new L42–47

<!-- loopdiff:thread {"id":"t-001","path":"src/auth.rs","old":null,"new":[42,47],"anchor":{"old":null,"new":47},"status":"open"} -->

```diff
-validate(token)
+validate(token)?
```

<!-- loopdiff:message {"id":"m-001","role":"human","author":"Alice"} -->
**Alice**

This error should not be silently ignored.
<!-- /loopdiff:message -->

<!-- loopdiff:message {"id":"m-002","role":"assistant","author":"Nova"} -->
**Nova**

Agreed. I will propagate the error.
<!-- /loopdiff:message -->

<!-- /loopdiff:thread -->
````

The YAML front matter makes the review a self-contained handoff contract. For
example, telling an agent “Review comments are in `review.md`; address them and
reply in the same file” is sufficient: the file itself defines the workflow,
response template, and validation command. The contract is canonical and is
validated along with the review, so it must not be edited or removed.

The machine-readable metadata records:

- format version;
- resolved Git commit IDs or worktree/index/stdin endpoints;
- SHA-256 of the reviewed patch;
- stable thread and message IDs;
- exact old/new ranges and inline anchor;
- thread status, message role, and author name.

Threads contain a flat chronological message list. Both humans and AI may start
a thread or reply. Human messages use `git config user.name` when available;
agents choose a name according to the embedded contract. `Reviewer` and `AI`
remain the fallbacks. `loopdiff --validate-review review.md` checks the front
matter and embedded agent contract in addition to structure, version, IDs,
roles, locations, metadata, and the patch hash shape. Opening an `-o` session
against a different patch is rejected.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Loopdiff is available under the
[MIT License](LICENSE). Release notes are maintained in
[CHANGELOG.md](CHANGELOG.md), and the maintainer release process is documented
in [RELEASING.md](RELEASING.md).
