# Changelog

## v0.3.0 – Skills

### Added

- **Skills system** — load reusable skill definitions from `SKILL.md` files:
  - **`--skill=PATH`** CLI flag (repeatable) — accepts a single `SKILL.md` file or a folder containing skills.
  - **Automatic discovery** — `SKILL.md` files in subfolders of the skills directory (configurable via `skills_dir` in config; default `~/.local/share/ai/skills`) are loaded automatically.
  - **Front matter parsing** — each skill's `name` and `description` are read from its YAML front matter and cached at startup. Missing names fall back to the folder/file stem.
  - **System prompt listing** — available skill names and descriptions are injected into the system prompt under a `## Skills` section.
  - **`load_skill` tool** — loads the full definition (instructions) of a skill by name on demand.
  - Duplicate skill names are deduplicated (first occurrence wins, warning logged).

### Changed

- **`--interactive` renamed to `--ask`** (`-i` short flag retained) — the flag only controls ask-before-deny policy behavior, not the interactive session, so the old name was misleading.
- **`-s/--session` now implies `--ask`** — previously the help text claimed it did but the code never set `policy.ask`, so tools silently failed in interactive sessions instead of prompting the user. Bug fix.
- **`--model` and `--provider` CLI overrides** — override the config model/provider for a single invocation without editing the config file.
- **`--yolo` conflicts with `--ask`** — the two are contradictory (allow everything vs. ask before denying), so the combination is now rejected at parse time. Help text clarified that yolo overrides all policy rules without asking.
- **Removed `/tools` interactive command** — its hardcoded list was out of sync with actually-registered tools and users can't call tools directly anyway.
- **`--init` now conflicts with `--config`** — `--init` writes to FILE, so `--init -c=...` was silently ignoring the `-c` value; the combination is now rejected at parse time.
- **`--list`, `--delete`, `--init`, and `--session` are now mutually exclusive** — these are standalone actions that bail out of `main()` early, so combining them was silently ignoring the extra flags. All combinations are now rejected at parse time.
- **Provider flavors** — all providers now map to one of two client flavors: OpenAI-compatible or Anthropic-compatible, each with a default base URL. Previously only `openai` and `anthropic` worked at runtime while the init wizard advertised 9. Added generic `openai-compatible` and `anthropic-compatible` providers that require a user-supplied `api_base` (e.g. for proxies or self-hosted endpoints). `api_base` in config still overrides the provider default.
- **Dependency security fixes** — replaced `markitdown` (which pinned vulnerable `lopdf 0.34`, `quick-xml 0.31/0.37`, and old `rig-core 0.8`) with `markdownify 0.3` + `pdf-extract 0.12` (`lopdf 0.42`, `quick-xml 0.41`). This resolves RUSTSEC-2026-0187 (lopdf stack overflow) and RUSTSEC-2026-0195/0194 (quick-xml DoS), which were previously ignored in CI. `file_view` routes PDFs to `pdf-extract` and all other formats through `markdownify`. The stale `ignore:` list was removed from the GitHub audit workflow. The four remaining advisories are all "unmaintained" warnings (not vulnerabilities) from the optional `obscura` browser crate and `lopdf`'s font parser.
- **Deprecated `serde_yaml` replaced** — migrated to the maintained `serde_yaml_ng 0.10` fork.
- **Hand-rolled UTC conversions removed** — the duplicated date math in `main.rs` and `session.rs` is replaced with the `time` crate.
- **Deduplicated byte-size formatting** — `fmt_bytes()` helper in `util.rs` shared by `file_info` and `download_file`.
- **Docs/version drift fixed** — `Cargo.toml` bumped to `0.3.0` to match the changelog; README CLI reference regenerated (`--system`, `--skill` added, `/tools` removed, `-i` → `--ask`).
- **`Role` enum replaces stringly-typed roles** — `session::Message.role` is now a typed `Role { User, Assistant, System }` instead of a raw string. Session files remain backward compatible (`#[serde(rename_all = "lowercase")]` keeps the on-disk format). The `/compact` summary is now consistently persisted and resumed as a `system` message (previously it was stored as `system` but rebuilt as `user`, mislabeling the summary as a user turn).
- **Shared HTTP client** — `web_fetch`, `web_search`, and `download_file` now reuse a single lazily-built `reqwest::Client` (connection pooling) instead of constructing a new client per request. A unified realistic browser user-agent is used across all three.
- **JS-injection hardening** — selectors and search queries spliced into `page.evaluate(...)` are now emitted as proper JSON string literals via `js_literal()` (which escapes `\`, `"`, newlines, and unicode line separators) instead of ad-hoc backslash/single-quote escaping. Applies to `browser_click`, `browser_get_element`, and Google/Bing search.

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