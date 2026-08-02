# ai

A CLI agent for interacting with AI models, with tool use, filesystem and command-execution capabilities.

`ai` is a single-binary Rust CLI that talks to LLM providers (OpenAI, Anthropic, Ollama, Groq, DeepSeek, Google, Mistral, OpenRouter, xAI) and lets the model use tools: read/write files, search code, run shell commands, browse the web, and more — all gated by a configurable policy engine.

## Features

- **Multi-provider support** — OpenAI, Anthropic, Ollama, Groq, DeepSeek, Google (Gemini), Mistral, OpenRouter, and xAI (Grok), configurable via `ai --init`. All map to an OpenAI- or Anthropic-compatible endpoint; `openai-compatible` and `anthropic-compatible` are also available for custom endpoints (requires `api_base`).
- **Interactive & one-shot modes** — Run with a direct prompt, pipe text via stdin, or start an interactive session with persistent history.
- **Sessions** — Save, list (`-l`), continue (`-s NAME`), and delete (`--delete NAME`) sessions with message history and system prompt preservation.
- **Tool system** — Filesystem tools, code search, git diff/log, web fetch/search, command execution, downloads, document extraction, and more.
- **Sandboxed command execution** — The `execute` tool runs through a virtual bash interpreter (bashkit) with 160+ in-process builtins; external commands require explicit policy approval.
- **Policy engine** — Granular allow/deny rules for read, write, execute, web fetch, and web search. Supports policy files, CLI overrides, interactive approval (`--ask`), and `--yolo` mode.
- **Persistent memory** — Optional agent memory stored to disk and injected into the system prompt.
- **Skills** — Load reusable skill definitions from `SKILL.md` files (via `--skill=PATH` or the skills folder), listed in the system prompt and loadable on demand with the `load_skill` tool.
- **Extended thinking** — Optional reasoning budgets for models that support it.
- **Headless browser** — Optional stealth-mode browser (Obscura) for web tools.
- **MCP tool servers** — Connect to external MCP servers with `--tool URL`.

## Installation

### Prebuilt binaries

Download the latest release for your platform from the [Releases](https://github.com/dswd/ai/releases) page:

- `ai-linux-amd64` / `ai-linux-arm64`
- `ai-windows-amd64.exe`

### Build from source

Requires Rust (edition 2024) and, for the default `browser` feature, `cmake`, `clang`, `llvm-dev`, and `libssl-dev`.

```sh
cargo build --release
# Optionally disable the headless browser (smaller build, no system deps):
cargo build --release --no-default-features
```

The binary is `target/release/ai`.

## Getting started

Run the interactive setup wizard to pick a provider, enter an API key, and choose a model:

```sh
ai --init
```

The config is written to `~/.config/ai/config.yaml` (or the path you pass to `--init`):

```yaml
provider: openai
api_key: "sk-..."          # or "env:OPENAI_API_KEY"
api_base: "https://api.openai.com/v1"
model: "gpt-4o"
context_window: 128000
```

API keys can also be supplied via environment variables (e.g. `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `DEEPSEEK_API_KEY`).

All providers map to an OpenAI- or Anthropic-compatible endpoint. To use a custom
endpoint, set `provider` to `openai-compatible` or `anthropic-compatible` and
provide `api_base`:

```yaml
provider: openai-compatible
api_key: "env:MY_PROXY_KEY"
api_base: "https://my-proxy.example.com/v1"
model: "some-model"
```

### One-shot prompt

```sh
ai "explain the code in main.rs"
cat file.txt | ai "summarize this"
```

### Interactive session

```sh
ai -s          # start a new session
ai -s=myname   # continue an existing session
ai -l          # list sessions
ai --delete=myname
```

### Granting the agent access

By default the agent is denied access to everything. Grant access explicitly:

```sh
# read-only access to the current directory
ai -r=. "what does this repo do?"

# read/write access to a directory
ai -w=./src "refactor this module"

# allow specific commands to run
ai -x="cargo,git" -r=. "run the tests and show me the failures"

# allow all web access
ai --web "find the latest docs for rig-core"

# everything, everywhere (dangerous)
ai --yolo "do whatever it takes"
```

With `--ask` (interactive approval), the agent can request access and you approve each request as it happens.

## Policy files

Policies are line-based allow/deny rules; the first matching rule wins. Patterns support `*` (within a segment) and `**` (across segments), and `~` expands to your home directory.

```
# ~/.config/ai/policy.txt
allow read /home/you/projects/**
allow write /home/you/projects/**
deny read /home/you/projects/secret/**
allow execute cargo,git,npm,npx
allow web-fetch https://docs.rs/**
allow web-search **
```

Load it with `ai -p=~/.config/ai/policy.txt ...`.

## Agent tools

Available tools (enabled based on policy):

| Category | Tools |
| --- | --- |
| Filesystem | `read_file`, `write_file`, `list_dir`, `file_info`, `find_files`, `search_content` |
| File mutation | `replace_in_file`, `delete_file`, `create_directory`, `move_file`, `copy_file` |
| Documents | `file_view` (extracts text from PDF, DOCX, XLSX, …) |
| Command execution | `execute` (bashkit builtins sandboxed; external commands need `-x`) |
| Git | `git_diff`, `git_log` |
| Web | `web_fetch`, `web_search`, `download_file`, `browser_navigate`, `browser_click`, `browser_get_content`, `browser_get_element`, `browser_evaluate` |
| Memory | `memory_add`, `memory_delete` |
| Skills | `load_skill` (loads a skill's full instructions by name) |

Tool output is capped (200 lines / ~100 KB) with offset/limit pagination and truncation notices.

## CLI reference

```
Usage: ai [OPTIONS] [PROMPT]...

Arguments:
  [PROMPT]...  Prompt text (if absent, read from stdin)

Options:
      --system=<PROMPT>      Set the system prompt
  -s, --session=[<NAME>]     Start an interactive session (or continue NAME, implies --ask)
  -m, --memory=[<FILE>]      Enable persistent memory
  -c, --config=<FILE>        Load configuration from FILE
      --model=<MODEL>        Override the model
      --provider=<PROVIDER>  Override the provider
  -r, --read=<PATH>          Allow read-only access to PATH
  -w, --write=<PATH>         Allow read/write access to PATH
  -x, --execute=<PATTERN>    Allow execution of PATTERN
      --web                  Allow all web access (fetch and search)
      --web-fetch=<PATTERN>  Allow web fetch for matching URL pattern
      --web-search=<PATTERN> Allow web search with matching query pattern
  -p, --policy=<FILE>        Load policy from FILE
      --skill=<PATH>         Load a skill (SKILL.md file or folder; repeatable)
  -i, --ask                  Ask for approval instead of denying
  -t, --tool=<URL>           Connect to an MCP tool server (repeatable)
  -y, --yolo                 Allow everything without asking (overrides all policy, dangerous)
      --max-tokens=<N>       Maximum number of tokens
      --max-turns=<N>        Maximum agent turns (tool call rounds) [default: 100]
      --thinking=[<TOKENS>]  Enable extended thinking [default: 16000]
  -l, --list                 List all saved sessions
      --init=[<FILE>]        Initialize config interactively
      --delete=<NAME>        Delete a session by NAME
  -v, --verbose              Enable verbose mode
  -q, --quiet                Enable quiet mode
  -h, --help                 Print help
  -V, --version              Print version
```

## Interactive session commands

- `/exit`, `/quit` — leave the session
- `/clear` — clear the current conversation
- `/compact` — summarize the conversation to reclaim context
- `/session` — show the current session name
- `/help` — list commands

## Development

```sh
cargo test        # run tests
cargo clippy      # lint
cargo fmt         # format
```

CI (build, test, audit) and release automation are configured for both GitHub and Gitea.

## License

See the repository for licensing details.


## Transparency

This software obviously contains AI capabilities and generates responses based on large language models. It does not contain any LLM itself but uses external LLMs and tools.

Also the software has in huge parts been developed by AI (vibe coding)