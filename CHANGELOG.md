# Changelog

## Unreleased

### Added

- **MCP servers in the config** — `mcp.servers` (`{ url, name? }`) declares external MCP servers alongside `--tool`; both sources are merged and deduplicated by URL. Config servers that fail to connect are skipped with a warning, while `--tool` failures still abort.
- **`load_skill` lists bundled files** — loading a skill now also returns the absolute paths of the other files in the skill's folder (recursive, hidden/build entries skipped, capped at 200) so the agent can open them with `read_file`.

### Changed

- **Breaking: `--skill` removed** — skills are discovered only from the configured `skills_dir` (default `<data-dir>/ai/skills`). When at least one skill is found, that folder is granted a default read permission, overridable by an explicit `deny read` in the policy file, so the agent can read skill files and their references with the standard read tools.

## v0.5.0 – Local Embeddings, Setup Wizard & CLI Overhaul

This release overhauls the CLI into subcommands, replaces keyword memory with local semantic search (ONNX embeddings + `sqlite-vec`), adds a guided `ai setup` wizard, and reworks session handling and maintenance.

### Added

- **Two-phase `ai setup`** — replaces the `--init` wizard. A deterministic phase picks a provider and model (reuse the existing config or detected credentials, browse the [models.dev](https://models.dev) catalog, or enter a custom URL) and proves the connection with a small call, falling back to the wizard on failure; an AI phase then edits the remaining config, saving through the sandboxed `write_config` tool with approval. The API key is never sent to the model and existing settings are preserved.
- **models.dev provider catalog** — fetches and caches `models.dev/api.json` (24 h TTL, stale-cache and built-in fallbacks) and maps any catalog provider to an OpenAI or Anthropic request flavor, surfacing model context windows and prices.
- **Local semantic memory** — memory facts and transcript excerpts are embedded locally with `fastembed` (ONNX) and searched with `sqlite-vec` (`vec0`, statically linked). Retrieval is cosine KNN with a distance cutoff (`memory_max_distance`, default `0.175` for E5 models), ranks memory above transcripts, and injects at most three hits per message. The model is configurable (`embedding_model`, default `multilingual-e5-small`; E5 `query:`/`passage:` prefixes applied automatically) and downloads to the user cache on first use.
- **Memory maintenance (`ai dream`)** — distills unprocessed transcripts into facts, prunes processed transcripts older than 7 days, and judges stale entries against newer neighbours, across a work pool (`dream.jobs`, default 4). It offers to run at the end of an interactive session when more than 50 tasks are pending, or automatically with `dream.auto: true`.
- **Memory CLI and tools** — `ai memory list` / `ai memory search QUERY`; agent tools `memory_add`, `memory_search`, `memory_get`, and `memory_delete`. Hits carry a cosine similarity score and long text is shown as a 100-character `…` fragment. Memory is enabled by default (`--no-memory` disables it), and an existing `memory.json` migrates once to SQLite.
- **Session handling** — `ai run` and bare `ai` continue the newest session when it is under 60 minutes old and matches the provider/model; `-s NAME` names or resumes sessions across dates (`YYYY-MM-DD_NAME`); `--no-session` runs stateless.
- **Model-initiated exit** — the `exit_program` tool lets the model end an interactive session or `ai setup`, aborting the in-flight stream so nothing further is generated.
- **Prompt and terminal polish** — an informative prompt (blue session name, container `@ image`, `[NN%]` context usage), dimmed history replay, a first-token spinner, `--no-color`, and `ai completions SHELL`.
- **Configurable search providers** — an ordered `search.providers` ladder (Brave, Tavily, Exa, Serper, SearXNG, or keyless DuckDuckGo/Google/Bing) that returns the first success, skips providers missing credentials, and falls through on auth or payment errors.
- **Strict config handling** — unknown keys warn (and fail `ai setup`), a `flavor` field records the request shape, `~` expands for every configured path, and provider-specific API-key environment variables are honored.

### Changed

- **Breaking: CLI reworked into subcommands** — `ai run "PROMPT"`, `ai session list|delete NAME`, `ai memory list|search QUERY`, `ai dream [--jobs N]`, `ai setup [FILE]`, and `ai completions SHELL`. Value flags accept space-separated values (`-r .`, `--model gpt-4o`); `--init`, `--session`, `-m`/`--memory`, and the flag-based commands are removed with no aliases.
- **Breaking: memory is SQLite- and vector-based** — the `memory:` path now names a SQLite database (`memory.db`); the FTS5 keyword index is replaced by embeddings and existing databases are re-embedded lazily. Inline `reconcile_memory` is replaced by `ai dream`, and `dream_jobs` moves under `dream.jobs`.
- **Breaking: `rig` 0.40 → 0.42** — the agent runtime moved to the `rig` facade (`rig-core` + `rig-agent`); tools implement `rig::tool::PortableTool` and context pruning uses the hook-v2 API. `rmcp` stays on 2.x.
- **Dependency refresh** — the lockfile is updated to the latest compatible releases (`clap` 4.6.7, `bashkit` 0.18.1, `rustls` 0.23.45, `lopdf` 0.45, refreshed `obscura`). `fastembed` stays at 4.9.1 and `rmcp` at 2.x because `rig` 0.42 pins `ort` 2.0.0-rc.9 via its optional `rig-fastembed` and targets `rmcp` 2.x.

### Removed

- **FTS5 keyword index** for `memory` and `transcripts` — superseded by semantic search.
- **Inline memory reconciliation** and the session `reconciled_until` field — superseded by `ai dream`.

### Fixed

- **Linux arm64 and Windows release builds** — local embeddings now sit behind an `embed` cargo feature (default on), which the Windows release build disables because ONNX Runtime has no `x86_64-pc-windows-gnu` build; on Windows facts still store but semantic retrieval is unavailable. The arm64 cross build now installs the C++ toolchain needed to link the ONNX Runtime static library.
- **Interactive prompt colors were never shown** — rustyline only emits `Prompt::styled()` with a `Highlighter` installed; the editor now installs the identity helper.
- **`container.runtime: auto` no longer reports a missing runtime**, and unknown or unavailable runtime values get an accurate message.
- **`command`/`type`/`which` no longer request a Read grant per `PATH` entry** — metadata-only `stat`/`exists` are not policy-gated; file contents and directory listings still are.
- **UTF-8 truncation panic and offset/limit overflow** in `truncate`/`process_output`, and a **tool-bar underflow** for long titles.
- **Session-name path traversal** — session names are validated to a single path component.
- **`~` expansion** for `--config`, `--policy`, `--skill`, configured paths, and the `ai setup FILE` argument.
- **Silent session loss** — a corrupt session file now warns with the actual error before starting a new session.
- **Provider context window** — the provider's known window is used when `context_window` is unset, so context pruning works out of the box.
- **`ai session list`** now shows each session's provider and reports sessions that cannot be parsed.
- **Pre-epoch clock** — session-name generation no longer panics.

### Added (infra)

- MIT `LICENSE`, a `rust-toolchain.toml` pinning stable (rustfmt + clippy), and binary-level integration tests under `tests/`.

## v0.4.0 – Container Isolation, Memory & Sessions

### Security

- **Container isolation for external commands** — `-X`/`--container[=IMAGE]` (default `debian:stable-slim`) starts a single session container at agent startup, and every `execute` command (builtins included) runs inside it via `exec`. Policy read roots are bind-mounted read-only and write roots read-write; the host filesystem is otherwise invisible, and the container gets `--network none` unless web access is granted. Docker and Podman are supported; the container is restarted once if it dies and removed on exit (including Ctrl-C).
- **One checked filesystem layer (`src/sandbox.rs`)** — every tool that touches a user-supplied path canonicalizes it *before* consulting policy and operates on the resolved path, so `deny` rules inside an allowed tree and symlink escapes are honored.
- **Session-scoped approvals** — interactive `--ask` approval prompts through the line editor (allow once / remember target / remember directory / deny) instead of reading raw stdin inside the policy check; remembered decisions live only for the session and never touch disk.

### Added

- **Container exec configuration** — `container.default_image`, `--container-runtime=auto|docker|podman`, `--no-container`, and `container.network=policy|none|host`. Mounts are derived from policy `-r`/`-w` grants and denied directories are masked with a tmpfs.
- **Full-context sessions (schema v2)** — session files persist the complete provider-agnostic message log, including tool calls and results, and resume replays it exactly; legacy v1 files migrate to text-only history.
- **Provider/model binding** — a session records the provider and model that produced it; resuming under a different pair forks a fresh session instead of replaying an incompatible history.
- **Deterministic context editing (`src/context.rs`)** — stale tool-result payloads are stubbed in the history sent to the model while the persisted session keeps the originals; `/compact` remains the manual fallback.
- **Provider capability flags** — `supports_thinking`/`supports_tools` gate provider-specific features, fixing `--thinking` leaking an Anthropic-only parameter into other providers' requests.
- **Retrieval-based memory** — replaces the flat JSON map dumped into the system prompt with a versioned store (id, text, keywords, timestamps, origin) scored by BM25 + keyword overlap and retrieved per message; existing `memory.json` migrates automatically.
- **Memory tools** — `memory_add` accepts optional `keywords` and upserts near-duplicates; `memory_search` queries the store directly; session exit reconciles durable facts with the model.
- **Proxy support for web tools** — `--proxy=<URL>` / the `proxy` config key (HTTP, HTTPS, SOCKS5) plus the standard `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` environment variables; clients are pooled per proxy.
- **SearXNG as a first-class option** — `ai --init` asks for an optional instance URL and a bare URL gets `?q=` appended; docker-compose setup documented.
- **Search and fetch resilience** — per-engine throttling (3s), one retry with backoff, and 20–60s cooldowns for blocked engines; `web_fetch` retries up to 3 times with 1s/2s backoff, while hard errors and blocks escalate to the stealth browser.
- **Markdown console formatting** — assistant output is ANSI-rendered on a TTY (`NO_COLOR` respected) for bold, italic, strikethrough, inline code, fenced blocks, and headers; piped output stays byte-identical raw markdown.
- **Missing-permissions guidance** — the system prompt lists missing capabilities and the exact flag to enable them.
- **Memory-injection notice** — stderr shows the memory entries injected into a message.
- **`x-opencode-session` header** — custom opencode-compatible endpoints receive the session id on every request.
- **`get_current_time` tool**, **`config.example.yaml`**, and **`AGENTS.md`**.

### Fixed

- **`--yolo` no longer disables the container** — it relaxes only the tool policy; external commands still run in the container. Whole-filesystem (`**`) grants are not bind-mounted (they would shadow the container rootfs).
- **bashkit working directory and `HOME`** — the virtual shell is seeded with the real effective cwd and a minimal `HOME`/`USER`/`PATH`/`PWD`, so builtin and external path resolution agree.

### Removed

- **`--sandbox` / `--sandbox-agent` / `sandbox:` config** (breaking) — replaced by container isolation; the `landlock`/`rlimit` dependencies are gone. `src/sandbox.rs` and the in-process `PolicyFsBackend` remain (the policy engine, not the OS sandbox).
- **`git_diff` / `git_log` tools** (breaking) — use `execute` with an `-x=git` grant instead.
- **Grant-width startup warnings** — moved to the README's Security contract.

### Changed

- **Container lifecycle logging** — starting, restarting, and removing the session container emit a 📦 `info!` line with the name, image, and runtime.
- **Memory trust** — durable memory is captured only from user-authored turns, tagged with an origin, and injected as explicitly non-instructional reference material.
- **Codebase cleanup** — split oversized files (`main.rs`, `tools/web_search.rs`, `tools/browser.rs`) and removed dead code; no behavior changes.
- **Fresh timestamps** — the system prompt no longer bakes in a `Current time:` line; the agent fetches it on demand via `get_current_time`.
- **Document conversion switched to `anydoc`** — adds legacy Word/PowerPoint/Excel, RTF, and EPUB with consistent Markdown output and a smaller dependency tree; ZIP/image/audio/video preview dropped.

## v0.3.0 – Skills, Provider Flavors & Security

### Added

- **Skills system** — load reusable `SKILL.md` definitions via `--skill=PATH` (file or folder) or auto-discovery from the skills directory (configurable via `skills_dir`). Skills are listed in the system prompt and their full instructions are loaded on demand with the `load_skill` tool.
- **Provider flavors** — all 9 wizard providers (OpenAI, Anthropic, Ollama, Groq, DeepSeek, Google, Mistral, OpenRouter, xAI) now work at runtime, each mapped to an OpenAI- or Anthropic-compatible endpoint. New generic `openai-compatible` / `anthropic-compatible` providers support custom endpoints via `api_base`.
- **`--model` / `--provider` CLI overrides** — switch model/provider for a single invocation without editing the config.

### Changed

- **CLI cleanup** — `--interactive` renamed to `--ask`; `-s/--session` now implies `--ask` (previously tools silently failed in interactive sessions); `--yolo` conflicts with `--ask`; `--init`/`--config` and `--list`/`--delete`/`--init`/`--session` are mutually exclusive; removed the stale `/tools` REPL command.
- **Typed message roles** — `session::Message.role` is now a `Role { User, Assistant, System }` enum instead of a raw string (session files stay backward compatible). `/compact` summaries are now consistently persisted and resumed as system messages.
- **Enforced external-command timeouts** — external commands now run through async `tokio::process` with `kill_on_drop`, so a long-running command (e.g. `sleep 300`) is killed at the configured timeout instead of running to completion and blocking a worker thread.
- **`main()` decomposed** — split into a thin entry point plus a `run()` orchestrator with extracted helpers; `--list`/`--delete` no longer fail on a broken policy file.
- **Misc** — shared HTTP client with connection pooling; `fmt_bytes()` deduplication; replaced hand-rolled UTC conversions with the `time` crate.

### Security

- **Resolved RUSTSEC-2026-0187/0195/0194** (high-severity lopdf/quick-xml advisories) by replacing `markitdown` with `markdownify` + `pdf-extract`; removed the stale CI audit ignore list.
- Replaced deprecated `serde_yaml` with the maintained `serde_yaml_ng` fork.
- Hardened JS-injection in browser/search tools — selectors and queries are now emitted as proper JSON string literals.

### Quality

- ~30 new unit tests across config, memory, util, tool output helpers, CLI parsing, and policy path resolution (71 total).

## v0.2.0 – Bashkit Integration & Policy-Based Filesystem

### Breaking

- **Remove `Think` tool** — deemed unnecessary for agent reasoning.
- **Execute permissions no longer required for built-in commands** — execute tool is always available; only external (non-bashkit) commands need `-x`/`Action::Execute`. Builtins are governed by filesystem read/write policy.
- **Feature-gated builtins removed from bashkit list** — `curl`, `wget`, `git`, `python`, `node`, `ssh`, `sqlite`, `jq` are now external commands handled via fork-exec, requiring `Action::Execute` policy.

### Added

- **Bashkit virtual bash interpreter** — replaces `sh -c` with 164 in-process builtins (echo, grep, sed, awk, find, tar, etc.). Sandboxed execution with resource limits and timeout control.
- **Policy-based filesystem (`PolicyFsBackend`)** — custom `FsBackend` for bashkit that checks `Action::Read`/`Action::Write` on every file operation, enabling fine-grained policy enforcement for built-in commands.
- **`file_view` tool** — extracts text from PDF, DOCX, XLSX, and other binary formats via `markdownify` + `pdf-extract`.
- **Obscura headless browser** — stealth-mode browser for web search (Bing, Google, DuckDuckGo) with anti-detection.

### Changed

- **`execute` tool**: Rewired to use bashkit `Bash::exec()` instead of `sh -c`. External commands fork-exec through registered `ExtBuiltin` wrappers with policy checks.
- **Tool file layout**: Each tool now lives in its own file under `src/tools/` (22 files + `mod.rs` + `shared.rs`). Old group files deleted.
- **Git tools** (`git_diff`, `git_log`): Check `Action::Read` on `.git` folder instead of `Action::Execute` on `"git"`.
- **Stdio reading**: Replaced 2-second timeout with async `tokio::io::stdin().read_to_end()`.
- **Policy summary**: Injected into system prompt as flat bullet list; default system prompt no longer claims tool access.
- **Memory tool**: Max 100 entries, 4-char hex keys via `rand 0.10`.
- **Token stats**: Displayed as `(in, thinking, out)`.
- **Session naming**: Random mnemonic when `-s` given without a name.

### Infrastructure

- Dependency added: `bashkit 0.14` (virtual bash interpreter), `obscura` (headless browser), `markdownify` + `pdf-extract` (document conversion).
- Build deps added: `cmake`, `clang`, `llvm-dev`, `libssl-dev` (for Obscura/Deno).
- Binary size: ~22 MB (release).


## v0.1.0 – Initial Release

**Project:** CLI interface for interacting with AI models, powered by `rig-core` with tool-use capabilities.

### Features

- **Multi-provider support:** OpenAI, Anthropic, Ollama, Groq, DeepSeek, Google (Gemini), Mistral, OpenRouter, and xAI (Grok).
- **Interactive & one-shot modes:** Run with a direct prompt, pipe input via stdin, or launch an interactive session with persistent history.
- **Session management:** Save, list, continue, and delete sessions with message history and system prompt preservation.
- **Tool system:**
  - **Execute tool** – Run shell commands with configurable timeout, offset/limit output control, and support for multiple commands.
  - **Filesystem tools** – Read/write/search files with policy-based access restrictions.
  - **Web tool** – Fetch URLs and search the web, with pattern-based allowlisting.
  - **Memory tool** – Persistent agent memory stored to disk and injected into the system prompt.
  - **Think tool** – Extended reasoning capability.
- **Policy engine:** Granular permission system controlling file access (read/write), command execution (glob patterns), and web access (fetch/search URLs). Supports policy files and CLI overrides.
- **Config management:** YAML-based configuration, interactive `--init` wizard for setting up providers, API keys, models, and context windows.
- **CLI flags:** `--memory`, `--session`, `--ask`, `--thinking`, `--max-tokens`, `--max-turns`, `--verbose`/`--quiet`, `--list`, `--delete`, `--tool` (for external MCP tool servers), `--policy`, `--yolo` (allow-all mode).
- **Output control:** Offset/limit pagination, hard caps (200 lines / 100 KB), and truncation notices for all tool outputs.
- **Logging:** `log` crate-based logging with emoji icons, color-coded levels, and configurable verbosity.
- **CI/CD:** Gitea workflows for audit, CI (build & test), and release automation.
- **Cross-platform:** Windows policy path handling, cross-compilation fixes.

### Infrastructure

- Rust edition 2024, async runtime with Tokio.
- Dependencies: `rig-core` (LLM framework), `clap` (CLI parsing), `serde`/`serde_yaml` (config), `rustyline` (interactive input), `reqwest` (HTTP), `dialoguer` (init wizard), and others.