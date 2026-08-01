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

## v0.2.0 – Bashkit Integration & Policy-Based Filesystem

### Breaking

- **Remove `Think` tool** — deemed unnecessary for agent reasoning.
- **Execute permissions no longer required for built-in commands** — execute tool is always available; only external (non-bashkit) commands need `-x`/`Action::Execute`. Builtins are governed by filesystem read/write policy.
- **Feature-gated builtins removed from bashkit list** — `curl`, `wget`, `git`, `python`, `node`, `ssh`, `sqlite`, `jq` are now external commands handled via fork-exec, requiring `Action::Execute` policy.

### Added

- **Bashkit virtual bash interpreter** — replaces `sh -c` with 164 in-process builtins (echo, grep, sed, awk, find, tar, etc.). Sandboxed execution with resource limits and timeout control.
- **Policy-based filesystem (`PolicyFsBackend`)** — custom `FsBackend` for bashkit that checks `Action::Read`/`Action::Write` on every file operation, enabling fine-grained policy enforcement for built-in commands.
- **`file_view` tool** — extracts text from PDF, DOCX, XLSX, and other binary formats via `markitdown`.
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

- Dependency added: `bashkit 0.14` (virtual bash interpreter), `obscura` (headless browser), `markitdown` (document conversion).
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
- **CLI flags:** `--memory`, `--session`, `--interactive`, `--thinking`, `--max-tokens`, `--max-turns`, `--verbose`/`--quiet`, `--list`, `--delete`, `--tool` (for external MCP tool servers), `--policy`, `--yolo` (allow-all mode).
- **Output control:** Offset/limit pagination, hard caps (200 lines / 100 KB), and truncation notices for all tool outputs.
- **Logging:** `log` crate-based logging with emoji icons, color-coded levels, and configurable verbosity.
- **CI/CD:** Gitea workflows for audit, CI (build & test), and release automation.
- **Cross-platform:** Windows policy path handling, cross-compilation fixes.

### Infrastructure

- Rust edition 2024, async runtime with Tokio.
- Dependencies: `rig-core` (LLM framework), `clap` (CLI parsing), `serde`/`serde_yaml` (config), `rustyline` (interactive input), `reqwest` (HTTP), `dialoguer` (init wizard), and others.