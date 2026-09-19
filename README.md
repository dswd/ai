# ai

A CLI agent for interacting with AI models, with tool use, filesystem and command-execution capabilities.

`ai` is a single-binary Rust CLI that talks to LLM providers (OpenAI, Anthropic, Ollama, Groq, DeepSeek, Google, Mistral, OpenRouter, xAI) and lets the model use tools: read/write files, search code, run shell commands, browse the web, and more — all gated by a configurable policy engine. Source: <https://github.com/dswd/ai>.

## Features

- **Multi-provider support** — OpenAI, Anthropic, Ollama, Groq, DeepSeek, Google (Gemini), Mistral, OpenRouter, and xAI (Grok), plus every compatible provider in the [models.dev](https://models.dev) catalog, configurable via `ai setup`. Providers map to an OpenAI- or Anthropic-compatible endpoint (`flavor`); `openai-compatible` and `anthropic-compatible` cover custom endpoints (requires `api_base`).
- **Interactive & one-shot modes** — Run `ai` for an interactive session with persistent history, or `ai run "PROMPT"` (or `cat f | ai run`) for a one-shot request.
- **Sessions** — Start, name (`-s NAME`), list (`ai session list`), and delete (`ai session delete NAME`) sessions with message history and system prompt preservation.
- **Tool system** — Filesystem tools, code search, web fetch/search, command execution, downloads, document extraction, and more.
- **Sandboxed command execution** — The `execute` tool runs through a virtual bash interpreter (bashkit) with ~150 in-process builtins; external commands require explicit policy approval and, when a container image is configured (`-X`/`--container`), the whole command runs in a session container with only the policy-granted paths bind-mounted.
- **Policy engine** — Granular allow/deny rules for read, write, execute, web fetch, and web search. Supports policy files, CLI overrides, interactive approval (`--ask`), and `--yolo` mode.
- **Persistent memory** — Agent memory stored to disk (path configurable via `memory:` in the config) and injected into the system prompt; `--no-memory` disables it. Retrieval is semantic: facts and past exchanges are embedded locally (ONNX, via fastembed — configurable `embedding_model`, default `multilingual-e5-small`) and searched with sqlite-vec. Search results and injected references are capped to a 100-character fragment. Run maintenance with `ai dream`; set `dream.auto: true` to run it automatically at the end of each interactive session, or the session offers it once more than 50 maintenance tasks are pending.
- **Skills** — Reusable `SKILL.md` definitions in the skills folder (config `skills.dir`, default `<data-dir>/ai/skills`), listed in the system prompt and loadable on demand with the `load_skill` tool; the folder is granted read access automatically. With `skills.auto_create: true`, `ai dream` can author skills from past sessions (tagged `origin: ai`, only those are agent-editable). Inspect or remove them with `ai skills list` / `ai skills delete NAME`.
- **Extended thinking** — Optional reasoning budgets for models that support it.
- **Headless browser** — Optional stealth-mode browser (Obscura) for web tools.
- **MCP tool servers** — Connect to external MCP servers with `--tool URL` or the `mcp.servers` config list; config servers that fail to connect are skipped with a warning.
- **Self-documenting** — ask the agent questions about `ai` itself (commands, flags, configuration, behavior) and it answers from a built-in manual via the `manual` tool. In a session, `/help` lists the slash commands and reminds you to just ask.

## Installation

### Prebuilt binaries

Download the latest release for your platform from the [Releases](https://github.com/dswd/ai/releases) page:

- `ai-linux-amd64` / `ai-linux-arm64`
- `ai-windows-amd64.exe` (built without the `browser` and `embed` features, so it has no semantic memory search)

### Build from source

Requires Rust (edition 2024). The default features also need `cmake`, `clang`, `llvm-dev`, and `libssl-dev` for the `browser` feature, plus a C++ toolchain and network access for the `embed` feature (ONNX Runtime is downloaded and linked at build time).

```sh
cargo build --release
# Disable the headless browser and local embeddings (smaller build, no system deps):
cargo build --release --no-default-features
```

The binary is `target/release/ai`.

## Getting started

Run the interactive setup to pick a provider, enter an API key, and choose a model:

```sh
ai setup
```

Setup runs in two phases. First it connects to an LLM: it reuses an existing
config (or detected credentials) if you want, otherwise you pick from the common
providers or any provider in the [models.dev](https://models.dev) catalog, or
enter a custom URL. Model choices show context size and prices when known. Then
an AI conversation walks you through the rest of the configuration. The existing
config (with the API key omitted) is given to the model in the prompt, and
changes are saved through a `write_config` tool that validates the YAML, keeps
the provider and credentials, and asks for approval before writing. Phase 2
opens with a tiny test call; if the provider or model can't be reached, setup
reports the error and returns you to the provider selection.

The config is written to `~/.config/ai/config.yaml` (or the path you pass to
`ai setup /path/to/config.yaml`). Running `ai setup` again reconfigures an
existing file, backing it up to `config.yaml.bak` first.
A fully-commented template with every supported key lives at [`config.example.yaml`](config.example.yaml):

```yaml
provider: openai
api_key: "sk-..."          # or "env:OPENAI_API_KEY"
api_base: "https://api.openai.com/v1"
model: "gpt-4o"
context_window: 128000
# proxy: "http://127.0.0.1:8080"   # optional: route web requests through a proxy
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
ai run "explain the code in main.rs"
cat file.txt | ai run "summarize this"
cat file.txt | ai run
```

### Interactive session

Running `ai` with no subcommand starts an interactive session:

```sh
ai                    # start a new session
ai -s myname          # start or continue the named session
ai session list       # list sessions
ai session delete myname
```

### Granting the agent access

By default the agent is denied access to everything. Grant access explicitly:

```sh
# read-only access to the current directory
ai run -r . "what does this repo do?"

# read/write access to a directory
ai run -w ./src "refactor this module"

# allow specific commands to run
ai run -x cargo,git -r . "run the tests and show me the failures"

# allow all web access
ai run --web "find the latest docs for rig-core"

# everything, everywhere (dangerous)
ai run --yolo "do whatever it takes"
```

With `--ask` (interactive approval), the agent can request access and you approve each request as it happens. Approvals are remembered for the session only: choose allow-once, remember the exact target, remember its directory, or deny.

## Security contract

The policy engine is a **boundary for narrow grants**, not a sandbox for broad ones. Stated plainly:

- **Read/write/execute decisions for the agent's own tools are enforced by one checked filesystem layer.** Paths are resolved (symlinks included) before the policy is consulted, and traversal checks every entry, so `deny` rules inside an allowed tree are honored. This is cross-platform and works on any filesystem.
- **External commands run in a session container (`-X`/`--container[=IMAGE]`).** When an image is configured (per-run via `-X`/`--container` — which defaults to `debian:stable-slim` when no image is given — or `container.default_image` in config), a single container is started at agent startup and **every** `execute` command (builtins included) runs inside it via `exec`. The whole command string is sent to the container's shell, so bashkit and the in-process policy layer are bypassed for that command. Only the policy's read roots are bind-mounted (read-only) and write roots (read-write); the host filesystem is otherwise invisible, and the container gets no network unless the policy grants web access. Works on stacked filesystems (ecryptfs, overlay) and wherever Docker/Podman run. The container is removed on exit.
- **Without a container, external commands are trusted.** `-x`/`--execute` gates command names; if no container is configured the command runs on the host with your privileges. When a container is active, `-x` gating is skipped (the container is the boundary). Grant `-x` narrowly, and prefer a container image for untrusted workloads.
- **`--no-container` forces host execution** even when a container is configured. `--yolo` does **not** disable the container: it grants the agent's tools full policy access, and external commands still run in the container (with only explicitly granted `-r`/`-w` paths mounted; whole-filesystem `**` grants are not mountable and are warned about).
- **`-x` still grants the command.** A container constrains what the command can reach, not whether it runs.
- **Broad grants are equivalent to full user compromise.** Write access to `$HOME` or `/` lets the agent plant auto-run files (shell rc files, git hooks, configs), which is equivalent to execute. `--yolo` grants everything.
- **Read + web = exfiltration.** Network egress is only filtered for containerized execs (no network unless web is granted). Otherwise, if the agent can read sensitive files *and* reach the network, an injected instruction can ship them out. Never grant both for untrusted content.
- **Memory is data, not instructions.** Durable memory is captured only from user-authored turns and injected as clearly non-instructional reference material.
- **Sessions are bound to their provider/model.** Resuming under a different model forks a fresh session rather than replaying an incompatible history.

Treat a grant as **narrow** or don't grant it. A grant whose width makes the boundary meaningless is not a boundary:

- **`--yolo`, or any whole-filesystem write grant** (`*`, `**`, `/`) — equivalent to full user compromise.
- **`-x` without a container** — external commands run as you, unsandboxed.
- **Write access to `$HOME` or `/`** — planting auto-run files (shell rc files, git hooks, configs) is equivalent to execute.
- **Read access combined with web access** — data exfiltration.

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

If a task needs a capability that isn't granted, the agent will suggest the exact flag to re-run with (e.g. `-r .`, `-w ./src`, `-x cargo,git`, `--web`).

## Agent tools

Available tools (enabled based on policy):

| Category | Tools |
| --- | --- |
| Filesystem | `read_file`, `write_file`, `list_dir`, `file_info`, `find_files`, `search_content` |
| File mutation | `replace_in_file`, `delete_file`, `create_directory`, `move_file`, `copy_file` |
| Documents | `file_view` (extracts text from PDF, DOCX, XLSX, PPTX, ODT, RTF, EPUB, CSV, HTML, …) |
| Command execution | `execute` (bashkit builtins sandboxed; external commands need `-x`) |
| Web | `web_fetch`, `web_search`, `download_file`, `browser_navigate`, `browser_click`, `browser_get_content`, `browser_get_element`, `browser_evaluate` |
| Memory | `memory_add`, `memory_search`, `memory_delete` |
| Skills | `load_skill` (loads a skill's full instructions by name) |
| Utility | `get_current_time` (current UTC date/time) |

Tool output is capped (200 lines / ~100 KB) with offset/limit pagination and truncation notices.

### Debugging web access

Search engines sometimes block automated requests. To see which engines work from
your current network, run the hidden probe flag:

```
ai --probe-web="your query"
```

It runs every configured search provider in order and prints per-provider
diagnostics: HTTP outcome, latency, response size, and the reason a provider was
rejected (e.g. a detected Cloudflare/CAPTCHA marker, or a 401/402 from an API).
It also prints each provider's results exactly as they would be returned to the
AI, so you can inspect what the model sees. Only configured providers are probed,
so a keyed API is queried at most once. Web requests use rotating user-agents and
a shared cookie jar; `web_fetch` automatically retries through the stealth
browser when it detects a block.

### Search providers

`web_search` walks an ordered list of providers and returns the first success.
Configure it under `search.providers`; each entry is `{ name, api_key?, url? }`:

```yaml
# ~/.config/ai/config.yaml
search:
  providers:
    - name: brave                 # uses BRAVE_API_KEY when api_key is omitted
    - name: tavily
      api_key: env:TAVILY_API_KEY
    - name: searxng
      url: "http://localhost:8080/search"
    - name: duckduckgo
    - name: google                # needs the browser feature
    - name: bing                  # needs the browser feature
```

- **Keyed APIs** — `brave`, `tavily`, `exa`, `serper`. An omitted `api_key` falls
  back to `BRAVE_API_KEY` / `TAVILY_API_KEY` / `EXA_API_KEY` / `SERPER_API_KEY`.
  A literal key or `env:VAR` both work; `env:` is preferred. Brave has a free
  tier: create a key at <https://brave.com/search/api/> and set `BRAVE_API_KEY`.
- **SearXNG** — a normal entry that needs `url`. It queries the instance's JSON
  API and falls back to HTML scraping if JSON is disabled.
- **Keyless scrapers** — `duckduckgo`, `google`, `bing` (`google`/`bing` need the
  `browser` feature). Their old search APIs are retired/sunsetting, so only the
  scrape paths exist.
- Unset `search.providers` defaults to DuckDuckGo → Google → Bing. Keyed
  providers run only when you list them, so an env var set for another tool never
  spends money.
- A listed provider missing its key/URL is reported at startup and skipped; a
  rejected key (401/403) or exhausted quota (402) skips to the next provider.

### Avoiding search-engine blocks

Search engines rate-limit and block automated clients. In order of effectiveness:

1. **Add an API provider (recommended).** A keyed backend such as Brave or Tavily
   returns structured results without scraping, so it is unaffected by CAPTCHA or
   IP blocks. Put it first in `search.providers`.

2. **Run your own SearXNG instance.** SearXNG aggregates results from many upstream
   engines and rotates them itself, so your IP is rarely the one being blocked.
   Minimal setup:

   ```yaml
   # docker-compose.yml
   services:
     searxng:
       image: searxng/searxng:latest
       ports: ["8080:8080"]
   ```

   Then list it (also configurable during `ai setup`):

   ```yaml
   search:
     providers:
       - name: searxng
         url: "http://localhost:8080/search"
   ```

   If the instance is unreachable the ladder falls back to the next provider.

3. **Route through a proxy.** Pass `--proxy=http://127.0.0.1:8080` (or
   `--proxy=socks5h://127.0.0.1:1080` for SOCKS5), or set `proxy` in
   `config.yaml`. Without an explicit proxy, the standard environment variables
   (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`) are honored. The
   explicit proxy applies to the HTTP-based tools (`web_fetch`, `web_search`,
   `download_file`); browser-based engines (Google/Bing under the `browser`
   feature) use the browser's own network stack, which honors the environment
   variables above.

4. **Let the built-in throttling work.** Requests are rate-limited (a 2s floor
   with jitter between searches, at least 3s between hits to the same engine),
   transient failures are retried with backoff, and engines that return a block
   page are put on a cooldown and skipped for a while instead of being hammered.
   Avoid back-to-back runs against the same engine — that is the fastest way to
   get blocked.

## CLI reference

```
Usage: ai [OPTIONS] [COMMAND]

Commands:
  run          Run the agent once with an inline prompt (or read it from stdin)
  session      Manage saved sessions (list, delete)
  setup        Set up or reconfigure the AI interactively
  dream        Run memory maintenance
  memory       Inspect persistent memory (list, search)
  skills       Inspect and manage skills (list, delete)
  completions  Generate a shell completion script and exit

Options:
  -s, --session-name <NAME>  Name of the session (continue it when it exists)
      --no-session           Do not read or write a session; run stateless
      --no-memory            Disable persistent memory for this run
      --system <PROMPT>      Set the system prompt
  -c, --config <FILE>        Load configuration from FILE
      --model <MODEL>        Override the model
      --provider <PROVIDER>  Override the provider
  -r, --read <PATH>          Allow read-only access to PATH
  -w, --write <PATH>         Allow read/write access to PATH
  -x, --execute <PATTERN>    Allow execution of PATTERN
      --web                  Allow all web access (fetch and search)
      --web-fetch <PATTERN>  Allow web fetch for matching URL pattern
      --web-search <PATTERN> Allow web search with matching query pattern
      --proxy <URL>          Route web requests through a proxy (http://… or socks5h://…)
  -p, --policy <FILE>        Load policy from FILE
  -i, --ask                  Ask for approval instead of denying
  -t, --tool <URL>           Connect to an MCP tool server (repeatable)
  -y, --yolo                 Allow everything without asking (overrides all policy, dangerous)
  -X, --container [<IMAGE>]  Run all external commands in this container image (default debian:stable-slim)
      --container-runtime <RUNTIME>  Container runtime: auto (default), docker, or podman
      --no-container         Run external commands on the host, ignoring any configured container
      --max-tokens <N>       Maximum number of tokens
      --max-turns <N>        Maximum agent turns (tool call rounds) [default: 100]
      --thinking [<TOKENS>]  Enable extended thinking [default: 16000]
  -v, --verbose              Enable verbose mode
  -q, --quiet                Enable quiet mode
      --no-color             Disable colored output (also honors NO_COLOR)
  -h, --help                 Print help
  -V, --version              Print version
```

Global options (`--config`, `--provider`, `--model`, `--max-turns`, `--verbose`,
`--quiet`, `--no-color`) may also follow any subcommand. Value flags accept both
`--opt value` and `--opt=value`. The persistent memory database location is a
config setting (`memory:`), not a CLI flag; use `--no-memory` to disable memory.

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