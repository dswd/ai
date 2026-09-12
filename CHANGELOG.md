# Changelog

## Unreleased

### Added

- **`--completions=<SHELL>`** — generate a shell completion script for bash, zsh, fish, and the other supported shells, then exit.
- **`--no-color`** — disable colored output (in addition to the `NO_COLOR` environment variable). ANSI styling is stripped from tool logs and reasoning output, not just assistant markdown.
- **First-token spinner** — while waiting for the model on an interactive terminal, a spinner is shown on stderr and cleared before any output.
- **Unknown config-key warnings** — `config.yaml` keys that don't match a known field now print a `warning: unknown config key '…'` instead of being silently ignored.

### Fixed

- **UTF-8 truncation panic** — `truncate`/`process_output` could panic when the line/byte cap fell inside a multi-byte character (emoji, CJK, accented text). Caps now snap to a char boundary.
- **Offset/limit overflow** — a huge model-supplied `limit` could overflow `offset + limit` and panic on the resulting slice; it is now saturated.
- **Tool-bar underflow** — `bar_title` underflowed (debug panic; huge allocation in release) for titles longer than 68 bytes; now saturated.
- **Session-name path traversal** — `-s=../../x` and `--delete=../foo` could read/delete files outside the session directory; names are now validated to a single path component.
- **Provider API-key env vars** — `DEEPSEEK_API_KEY`, `GROQ_API_KEY`, `GEMINI_API_KEY`, and the other provider-specific variables are now used as a fallback when `api_key` is unset (previously only `OPENAI_API_KEY`/`ANTHROPIC_API_KEY` were).
- **`~` expansion** — `~`/`~/…` is now expanded for `--config`, `--policy`, `--memory`, `--skill`, `--init`, and config paths (`session_dir`, `skills_dir`, `policy`, `memory`), matching the documented examples.
- **Silent session loss** — a corrupt or unreadable session file now warns with the actual error before starting a new session, instead of silently discarding the history.
- **Provider context window** — the provider's known context window is used when `context_window` is not set in the config, so context pruning works out of the box.
- **`--list` output** — now shows each session's provider and reports sessions that cannot be parsed instead of printing only the name.
- **Pre-epoch clock** — session-name generation no longer panics if the system clock is before the Unix epoch.

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