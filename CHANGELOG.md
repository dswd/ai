# Changelog

## Unreleased

### Added

- **Two-phase `--setup`** — replaces the old `--init` wizard. Phase 1 is deterministic: it offers to reuse an existing config or detected credentials, otherwise lets you pick from common providers, search the full [models.dev](https://models.dev) catalog, or enter a custom URL, then choose a model with context size and input/output prices shown when known. Phase 2 is an AI conversation that edits the rest of the config; the existing config (API key omitted) is provided in the prompt and the setup-only `write_config` tool saves changes with approval.
- **Connection test with fallback** — phase 2 begins with a tiny model call to prove the phase-1 provider/model/credentials work. If it fails, the error is reported and setup returns to the provider wizard (up to 5 attempts) instead of dropping into a conversation that cannot respond.
- **models.dev provider catalog (`src/catalog.rs`)** — fetches and caches `models.dev/api.json` (24h TTL, stale-cache and built-in fallbacks), maps each provider to an OpenAI or Anthropic flavor, hides native-protocol providers (Bedrock, Vertex, Azure, Watson X, SAP), and surfaces model context windows and prices.
- **Config `flavor` field** — records whether a provider speaks the OpenAI or Anthropic request shape, so any models.dev provider (not just the built-in table) can be used at runtime. Omitted values are derived from the built-in provider table.
- **`write_config` setup tool (`src/tools/write_config.rs`)** — the setup AI's only way to persist config: it strictly parses the YAML, restores the existing provider/credentials/model/flavor, and saves through the sandbox with user approval. It neither takes nor reveals the config path, and the API key is never sent to the model.
- **Strict config parsing** — `Config::parse_strict`/`from_file_strict` reject unknown keys; `write_config` returns parse errors to the model so it can correct them in-session.
- **Setup shows current values and preserves settings** — reconfigure prints a full summary of the existing configuration (with unset fields marked `(default)`), seeds the wizard defaults from it, offers to keep the current API key, and no longer drops non-connection settings (`system_prompt`, `proxy`, `container`, `memory`, …) when you decline reuse.
- **Setup AI recaps the config** — the setup prompt instructs the model to show the complete configuration after every change, and `write_config` returns the saved config with the API key `(redacted)` so the model stays anchored to the full state.
- **Current session for one-off runs** — a prompt without `-s` now continues the newest session (by file modification time) when it is less than 60 minutes old and its provider/model match the run, replaying its full history like an interactive resume; otherwise it starts a new session. One-off sessions are saved only after a successful run, and auto-generated names are date-prefixed (`YYYY-MM-DD_calm-hawk`). `--no-session` runs stateless (no read, no write), and `--list` marks the current session with `*`.
- **`ai -s` with no name resumes the current session** — with no NAME, `-s` now continues the newest session under the same rules as one-off runs (<60 min, schema v2, matching provider/model) and starts a new one otherwise, reporting `Continuing session: NAME` / `Started new session: NAME` at info level.
- **Date-aware explicit session names** — an undated `-s=NAME` is an alias across dates: it resumes the most recently modified session named `NAME` or `YYYY-MM-DD_NAME`, and creates `YYYY-MM-DD_NAME` when none exists. A dated `-s=YYYY-MM-DD_NAME` is matched exactly. `--delete=NAME` resolves the same way.
- **Model-initiated exit** — in an interactive session (`-s`) the model gets an `exit_program` tool and can end the program when the user asks to quit or exit, running the normal end-of-session path (memory reconciliation, session save, resume hint). The `--setup` AI gets the same tool so it can end setup on request. Once the tool runs, the in-flight model stream is aborted, so no further assistant output is generated or printed. It is not exposed to one-off runs.
- **Informative session prompt** — the interactive prompt shows the session name (date prefix dropped) in blue, the active container image as `@ <image>` in yellow, and the context-window usage as `[NN%]` in green/orange/red (50%/75% tiers), ending in a white `❯` caret. Styling collapses automatically under `--no-color`/`NO_COLOR`.
- **Dimmed session replay** — when resuming a session, the replayed exchange (the echoed user line and the assistant reply) is rendered dim so it reads as past history; the assistant's markdown styling is kept over the dim base.
- **`--completions=<SHELL>`** — generate a shell completion script for bash, zsh, fish, and the other supported shells, then exit.
- **`--no-color`** — disable colored output (in addition to the `NO_COLOR` environment variable). ANSI styling is stripped from tool logs and reasoning output, not just assistant markdown.
- **First-token spinner** — while waiting for the model on an interactive terminal, a spinner is shown on stderr and cleared before any output.
- **Unknown config-key warnings** — `config.yaml` keys that don't match a known field now print a `warning: unknown config key '…'` instead of being silently ignored.
- **Configurable search-API providers** — `search.providers` is an ordered ladder of web-search backends: keyed APIs (`brave`, `tavily`, `exa`, `serper`), SearXNG (now queried via its JSON API with an HTML fallback), and the keyless `duckduckgo`/`google`/`bing`. Entries are `{ name, api_key?, url? }`; an omitted key falls back to `BRAVE_API_KEY` / `TAVILY_API_KEY` / `EXA_API_KEY` / `SERPER_API_KEY`. The ladder returns the first success; a listed provider missing its key/URL is skipped with a startup warning, and 401/403/402 fall through to the next provider. `--probe-web` probes only the configured providers, and the setup AI never sees literal search keys.

### Changed

- **Breaking: `--init` renamed to `--setup`** — no alias. `ai --setup` creates or reconfigures the config; an existing file is backed up to `config.yaml.bak` first.
- **Breaking: `search.searxng_url` replaced by `search.providers`** — SearXNG is now a normal provider entry (`{ name: searxng, url: ... }`) in the ordered list. Configs still using `search.searxng_url` get an unknown-key warning (and fail strict parsing during `--setup`); migrate by listing the provider.
- **Breaking: `rig` 0.40 → 0.42** — the agent runtime moved out of `rig-core` into the `rig` facade (`rig-core` + `rig-agent`). Tools now implement `rig::tool::PortableTool`; the `Agent` type is no longer model-generic; and context pruning uses the hook-v2 `AgentHook::on_completion_call` event instead of `on_event`/`Flow`. MCP stays on `rmcp` 2.x because that is what `rig-agent` 0.42 targets (3.x is not yet compatible).
- **Breaking: session files from the previous rig version** — assistant content is now tagged in the serialized log, so sessions written by rig 0.40 may fail to load. They are skipped with a warning and a new session starts; delete old session files to silence the warning.

### Fixed

- **Interactive prompt colors were never shown** — the styled prompt variant was built but never rendered because rustyline only emits `Prompt::styled()` when a `Highlighter` is installed; the line editor now installs the identity helper, so the colored session prompt appears (and still degrades to plain text under `--no-color`/`NO_COLOR`). Tab completion stays silent via `BellStyle::None`.
- **`container.runtime: auto` no longer reports a missing runtime** — the documented default was treated as an unknown runtime, so a configured container image failed with "no container runtime found" even when docker or podman was installed. `auto` (and an empty value) now auto-detects; unknown or unavailable runtime values get an accurate message.
- **`command`/`type`/`which` no longer spam Read approvals** — the shell's filesystem backend no longer gates metadata-only `stat`/`exists`, so bashkit's PATH lookups do not request a Read grant per `PATH` entry. File contents and directory listings remain policy-gated (metadata such as existence/type is now visible without a Read grant). `command` is also no longer advertised as a free builtin.
- **UTF-8 truncation panic** — `truncate`/`process_output` could panic when the line/byte cap fell inside a multi-byte character (emoji, CJK, accented text). Caps now snap to a char boundary.
- **Offset/limit overflow** — a huge model-supplied `limit` could overflow `offset + limit` and panic on the resulting slice; it is now saturated.
- **Tool-bar underflow** — `bar_title` underflowed (debug panic; huge allocation in release) for titles longer than 68 bytes; now saturated.
- **Session-name path traversal** — `-s=../../x` and `--delete=../foo` could read/delete files outside the session directory; names are now validated to a single path component.
- **Provider API-key env vars** — `DEEPSEEK_API_KEY`, `GROQ_API_KEY`, `GEMINI_API_KEY`, and the other provider-specific variables are now used as a fallback when `api_key` is unset (previously only `OPENAI_API_KEY`/`ANTHROPIC_API_KEY` were).
- **`~` expansion** — `~`/`~/…` is now expanded for `--config`, `--policy`, `--memory`, `--skill`, `--setup`, and config paths (`session_dir`, `skills_dir`, `policy`, `memory`), matching the documented examples.
- **Silent session loss** — a corrupt or unreadable session file now warns with the actual error before starting a new session, instead of silently discarding the history.
- **Provider context window** — the provider's known context window is used when `context_window` is not set in the config, so context pruning works out of the box.
- **`--list` output** — now shows each session's provider and reports sessions that cannot be parsed instead of printing only the name.
- **Pre-epoch clock** — session-name generation no longer panics if the system clock is before the Unix epoch.

### Added (infra)

- MIT `LICENSE`, a `rust-toolchain.toml` pinning stable (rustfmt + clippy), and binary-level integration tests under `tests/`.
- **Dependency refresh** — moved to `rig` 0.42, `rmcp` 2.x, `dirs` 7, `anydoc` 0.2, and `bashkit` 0.18, plus `cargo update` for the rest (`log` 0.4.34, `reqwest` 0.13.5, refreshed `obscura` git revision). The `pdf-inspector` git override is gone: released `pdf-inspector` 1.19.0 already carries the fixed `lopdf` 0.44.

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