# Changelog

## Unreleased

### Added

- **Proxy support for web tools** — route `web_fetch`, `web_search`, and `download_file` through a proxy via `--proxy=<URL>` (HTTP, HTTPS, or SOCKS5 such as `socks5h://127.0.0.1:1080`) or the `proxy` key in `config.yaml`. Without an explicit proxy, the standard `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` environment variables are honored. HTTP clients are pooled per proxy configuration.
- **SearXNG as a first-class option** — `ai --init` now asks for an optional SearXNG instance URL, and a configured URL no longer requires a `{query}` placeholder (a bare instance URL gets `?q=` appended automatically). Documented a docker-compose setup in the README.
- **Per-engine throttling and backoff for search** — at least 3s between requests to the same engine; transient failures (network, timeouts, rate limits) are retried once with a 2s backoff; engines that return a block page are put on a 60s cooldown (20s for transient errors) so the ladder skips them instead of hammering them.
- **Retry with backoff for `web_fetch`** — up to 3 attempts with 1s/2s exponential backoff. Transient failures are retried; hard errors (HTTP 404/5xx, detected Cloudflare/CAPTCHA pages) stop the loop so blocks escalate straight to the stealth browser.
- **Missing-permissions guidance** — when a permission group isn't granted, the system prompt now lists the missing capability and the exact flag to re-run with (`-r <PATH>`, `-w <PATH>`, `-x <PATTERN>`, `--web`), so the agent can tell the user how to enable what it needs.

### Changed

- **Fresh timestamps via `get_current_time`** — the system prompt no longer bakes in a `Current time:` line at startup (it went stale in long-running sessions). The agent now fetches the current UTC time on demand with the new `get_current_time` tool, which is always available.

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