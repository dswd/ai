# AGENTS.md

Guidance for AI agents working in this repository.

## Project

`ai` is a single-binary Rust CLI agent: it talks to LLM providers and lets the model
use tools (filesystem, shell, web, git, memory) gated by a policy engine. Rust edition
2024, async via Tokio, LLM layer via `rig-core`. Default build enables the `browser`
feature (Obscura headless browser for web tools); `--no-default-features` drops it.

## Commands

- Build: `cargo build` (add `--no-default-features` to skip the browser)
- Test: `cargo test`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: always run `cargo fmt` after every edit; verify with `cargo fmt --all --check`

CI enforces clippy with `-D warnings` and fmt on every push/PR, so keep the tree clean.
Never commit unformatted code — run `cargo fmt` before finishing any change.

## Module map (src/)

| File | Purpose |
| --- | --- |
| `main.rs` | Entry point; wires CLI, config, policy, agent, streaming loops |
| `cli.rs` | clap arg definitions (note: all value args use `require_equals`, e.g. `-r=.`) |
| `config.rs` | YAML config, path resolution |
| `providers.rs` | Provider registry: two flavors (OpenAi, Anthropic) + OpenAI-compatible endpoints |
| `policy.rs` | Allow/deny rules, first-match-wins, glob matching, CLI overrides, ask mode |
| `skills.rs` | Skill discovery/loading (markdown front-matter files) |
| `session.rs` | Session persistence (JSON) |
| `memory.rs` | Persistent agent memory |
| `tools/` | One file per tool (see below) |
| `format.rs` | Streaming markdown-to-ANSI console formatting for assistant output |
| `output.rs` | stdout/stderr stream routing (TTY-aware, `NO_COLOR`-aware) |
| `init.rs` | Interactive config wizard |
| `io.rs` | Line editor / stdin handling |
| `util.rs` | Small helpers (formatting bars, byte sizes) |

## Tool architecture

Each tool in `src/tools/` is a rig-core `Tool` with: serde + schemars args struct,
`new(policy)` constructor, and `call` implementing the action. Tools are registered in
`build_agent` (`main.rs`) **only when policy allows**: read tools need a `Read` allow rule,
write tools a `Write` rule, web tools `WebFetch`/`WebSearch`, and so on.

Key tools:
- `execute.rs` — runs shell commands through bashkit's virtual bash. In-process builtins
everywhere (no `-x` needed); external commands fork-exec and require an `Action::Execute`
allow rule. Filesystem ops are policy-checked via `policy_fs.rs` (`PolicyFsBackend`).
- `policy_fs.rs` — `FsBackend` shim that checks Read/Write policy on every file op.
- `shared.rs` — shared helpers: `BASHKIT_BUILTINS` list, `is_bashkit_builtin`, output
limit/offset helpers, search/walk utilities.
- `browser.rs` — Obscura headless browser tools (feature-gated).
- `file_view.rs` — extracts text from PDF/DOCX/XLSX/etc via `anydoc`.

## Conventions

- No code comments unless asked; keep changes idiomatic and minimal.
- Always run `cargo fmt` (rustfmt) after any code change and before committing.
- Track all changes in `CHANGELOG.md` — add/update an entry for every feature, fix, or
  behavioral change, grouped under the current or next version.
- Add `Action`-gated tools to `build_agent` in `main.rs`, following existing registration order.
- Tool output must respect the hard caps in `tools/mod.rs`: `MAX_OUTPUT_LINES` (200) and
`MAX_OUTPUT_CHARS` (~100 KB); use `process_output`/`truncate` for offset/limit handling.
- Use `ansi_color_constants` for terminal styling (logs/tool bars).
- Assistant markdown output is rendered by `format.rs`; don't emit raw ANSI to stdout.
- Tests live as inline `#[cfg(test)] mod tests` next to the code (see `policy.rs`, `format.rs`).

## Release process

Follow `.opencode/skills/release/SKILL.md`: bump version in `Cargo.toml`, update
`CHANGELOG.md` (concise, grouped, breaking changes flagged), commit, tag `vX.Y.Z`, push.
Prebuilt binaries are produced by the GitHub/Gitea release workflows on tags.

Keep `CHANGELOG.md` up to date continuously — don't leave entries to be written only at
release time.

## State of the working tree

- The repo tracks `src/providers.rs` and `src/skills.rs`; keep `mod providers;`/`mod skills;`
in `main.rs` — they are required for compilation.
- `.gitignore` excludes `/target`, `PLAN.md`, `web/`.