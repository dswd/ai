# AGENTS.md

Guidance for AI agents working in this repository.

## Project

`ai` is a single-binary Rust CLI agent: it talks to LLM providers and lets the model
use tools (filesystem, shell, web, memory) gated by a policy engine. Rust edition
2024, async via Tokio, LLM layer via the `rig` facade (`rig-core` + `rig-agent`). Default build enables the `browser`
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
| `main.rs` | Entry point; subcommand dispatch (default = interactive session, `run`, `session`, `setup`, `dream`, `memory`, `completions`, `probe-web`), then wires config, policy, agent, streaming loops. Also runs the end-of-session dream prompt/auto-dream (`maybe_dream_at_exit`, shared `run_dream`). |
| `cli.rs` | clap definitions: `Cli` (globals + `command`) flattens `AgentArgs` (agent options, also flattened into `run`); value args accept space or `=`; subcommand enums `Command`/`SessionCommand`/`MemoryCommand`. Interactive is the no-subcommand default; `-s/--session-name NAME` only names the session |
| `config.rs` | YAML config, path resolution (incl. the `dream:` section: `auto`, `jobs`) |
| `providers.rs` | Provider registry: two flavors (OpenAi, Anthropic) + OpenAI-compatible endpoints |
| `policy.rs` | Allow/deny rules, first-match-wins, glob matching, CLI overrides, ask mode, session approvals |
| `sandbox.rs` | The single checked filesystem layer: resolve-then-authorize-then-operate for every tool path |
| `container.rs` | Session-scoped container for external commands (Docker/Podman): policy→bind mounts, network none unless web |
| `context.rs` | Deterministic context editing: prune stale tool outputs from the history sent to the model |
| `skills.rs` | Skill discovery/loading (markdown front-matter files) |
| `session.rs` | Session persistence (JSON, schema v2: full chat log + provider/model binding); newest-session selection, the 60-minute resume window, and date-aware name lookup (`NAME` matches `YYYY-MM-DD_NAME`) used to continue one-off runs, an unnamed `ai` session, and explicit names; `tuples()` extracts completed user/agent exchanges for the transcript index |
| `memory.rs` | SQLite store (`memory.db`): `memory` (facts + tags + provenance) and `transcripts` (derived user+agent tuples) tables, one-time migration from `memory.json`, session backfill, and semantic retrieval over sqlite-vec KNN with `last_used` tracking |
| `embed.rs` | Local embedding (`fastembed`/ONNX): `Embedder` trait, `FastembedEmbedder` (lazy model load, E5 query/passage prefixes), model-id resolution, and vector serialization |
| `dream.rs` | `ai dream` maintenance over a parallel work pool: extract memories from unprocessed transcripts, prune processed transcripts, judge stale entries. Also holds `PROMPT_THRESHOLD`/`should_prompt`/`is_affirmative` for the end-of-session prompt. |
| `tools/` | One file per tool (see below) |
| `format.rs` | Streaming markdown-to-ANSI console formatting for assistant output |
| `output.rs` | stdout/stderr stream routing (TTY-aware, `NO_COLOR`-aware) |
| `setup_cmd.rs` | Two-phase `ai setup` flow: deterministic provider/model wizard, then an AI conversation that edits the config |
| `catalog.rs` | models.dev provider/model catalog: fetch + cache, flavor mapping, compatibility filter |
| `io.rs` | Line editor / stdin handling |
| `util.rs` | Small helpers (formatting bars, byte sizes) |
| `agent.rs` | `AgentContext`, tool registration (`build_agent`), streaming and oneshot dispatch |
| `prompt.rs` | System prompt assembly, permission availability, missing-permission guidance |
| `setup.rs` | Config/policy/session/provider resolution and CLI override application |
| `clients.rs` | OpenAI/Anthropic client construction, `x-opencode-session` header |
| `commands.rs` | Non-agent subcommands: `probe-web`, `session list`, `session delete` |
| `tool.rs` | MCP tool-server connection (`--tool`) |
| `logging.rs` | Console logger, log-level setup |
| `interactive.rs` | Interactive session loop, `/` commands, transcript indexing on save, usage reporting |

## Tool architecture

Each tool in `src/tools/` is a `rig::tool::PortableTool` with: serde + schemars args struct,
`new(policy)` constructor, and `call` implementing the action. Tools are registered in
`build_agent` (`agent.rs`) **only when policy allows**: read tools need a `Read` allow rule,
write tools a `Write` rule, web tools `WebFetch`/`WebSearch`, and so on. Every user-supplied
path an action touches must go through `src/sandbox.rs` (never `std::fs` directly), so the
resolve-then-authorize-then-operate contract holds for every caller.

Key tools:
- `execute.rs` — runs shell commands through bashkit's virtual bash. In-process builtins
run without `-x`; external commands fork-exec and require an `Action::Execute` allow rule.
When a container is configured, the whole command string is instead sent to the container's
shell (see `container.rs`). Filesystem ops are policy-checked via `policy_fs.rs` (`PolicyFsBackend`).
- `policy_fs.rs` — `FsBackend` shim that checks Read/Write policy on every file op.
  Metadata-only `stat`/`exists` are intentionally not gated so bashkit's `command`/`type`/
  `which`/PATH resolution does not prompt for a Read grant per `PATH` entry.
- `shared.rs` — shared helpers: `BASHKIT_BUILTINS` list, `is_bashkit_builtin`, output
limit/offset helpers, search/walk utilities.
- `browser_state.rs` + `browser_*.rs` — Obscura headless browser state and one tool per
file (feature-gated). `search_browser.rs` holds the browser-driven search-engine fallback.
- `search_api.rs` — configurable search-API providers: `SearchResult` + renderer, JSON
request/parse pairs for Brave/Tavily/Exa/Serper/SearXNG, and status→`EngineError` mapping.
- `search_html.rs` / `search_probe.rs` — search-result HTML/markdown/quality helpers and the
`--probe-web` diagnostics.
- `file_view.rs` — extracts text from PDF/DOCX/XLSX/etc via `anydoc`.
- `write_config.rs` — setup-only `write_config` tool: strictly parses the AI's YAML, restores
the real provider/credentials (including search keys), and saves through the sandbox (write
approval). It never takes or reports the config path, so the setup AI cannot read the secret file.
- `exit_program.rs` — tool the model calls to end interactive sessions and `ai setup` when the
user asks; signals the loop via a shared `Arc<AtomicBool>` in `AgentContext`. `stream_response`
checks the flag per stream item and aborts generation once it is set. Not `Action`-gated,
and not registered for one-off runs.

## Conventions

- No code comments unless asked; keep changes idiomatic and minimal.
- Always run `cargo fmt` (rustfmt) after any code change and before committing.
- Track all changes in `CHANGELOG.md` — add/update an entry for every feature, fix, or
  behavioral change, grouped under the current or next version.
- Add `Action`-gated tools to `build_agent` in `agent.rs`, following existing registration order.
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
- `.gitignore` excludes `/target`, `PLAN.md`, `web`.