# ai manual

A model-facing guide to the `ai` CLI. Project home: <https://github.com/dswd/ai>.

## Overview

`ai` is a single-binary Rust CLI agent. It talks to an LLM provider, and the model can use
tools (filesystem, shell, web, memory, skills) that are gated by a policy engine. Sessions
persist the full chat log so a run can be resumed.

Key concepts:

- **Provider** — where model requests go (`provider`, `api_base`, `model`, `api_key`). Flavor is
  `openai` or `anthropic`; any OpenAI/Anthropic-compatible endpoint works.
- **Policy** — line-based allow/deny rules for read, write, execute, web fetch, and web search.
  Tools are only registered when the policy allows them. First matching rule wins.
- **Tools** — `read_file`, `write_file`, `search_content`, `execute`, `web_fetch`, `web_search`,
  memory tools, `load_skill`, `manual`, and any MCP tools from `mcp.servers`/`--tool`.
- **Sessions** — every run is saved; `ai run` and bare `ai` continue the newest session under 60
  minutes old with a matching provider/model. `--no-session` runs stateless.
- **Memory** — durable facts (SQLite) plus a transcript index, retrieved semantically and injected
  into the prompt. `ai dream` maintains it.
- **Skills** — reusable `SKILL.md` procedures loaded on demand; `ai dream` can author them.

## Getting started

```sh
ai setup                 # interactive provider/model wizard, writes config.yaml
ai "summarize this repo" # one-off; continues the current session if recent
ai                       # interactive session
```

Useful extras:

- `ai run "PROMPT"` — one-shot request (also reads stdin: `cat f | ai run`).
- `ai -s NAME` — name or resume a session across dates (`YYYY-MM-DD_NAME`).
- `ai --help` / `ai <command> --help` — full flag reference.

## Permissions and policy

Rules are one per line; the first match wins. Patterns use `*` within a path segment and `**`
across segments; `~` expands to your home directory.

```
# ~/.config/ai/policy.txt
allow read /home/you/projects/**
deny read /home/you/projects/secret/**
allow execute cargo,git
allow web fetch https://docs.rs/**
```

CLI overrides add rules for one run: `-r PATH` (read), `-w PATH` (write), `-x PATTERN`
(execute), `--web-fetch PATTERN`, `--web-search PATTERN`, `--web` (all web), `-p FILE`
(policy file), `-y/--yolo` (allow everything — dangerous). `--ask` prompts for unmatched
actions and remembers approvals for the session only.

Common symptom: a tool is missing from the model's toolbox. That means no allow rule covers it —
add the matching `-r`/`-w`/`-x`/`--web` flag or a policy rule.

## Filesystem and containers

Every user-supplied path is resolved (symlinks included) and authorized before the operation, so
`deny` rules inside an allowed tree are honored. Metadata-only checks (exists/type) are not gated.

External commands run through bashkit's virtual bash. In-process builtins (~150) run without a
grant; external commands require `-x` unless a container is active. With a container configured
(`-X`/`--container[=IMAGE]`, default `debian:stable-slim`, or `container.default_image`), the whole
command runs inside it, and only policy read roots (read-only) and write roots (read-write) are
bind-mounted. `--no-container` forces host execution. `--yolo` does not disable the container.

## Memory

Memory is enabled by default (`--no-memory` disables it) and stored at the `memory` config path
(default `<data-dir>/ai/memory.db`). Facts are embedded locally with `fastembed` (ONNX) and
retrieved by cosine similarity; at most a few hits are injected per message, each shown with a
match score. Key settings:

- `embedding_model` — default `multilingual-e5-small` (E5 `query:`/`passage:` prefixes applied).
- `memory_max_distance` — cutoff; lower is stricter (default 0.175 for E5, 0.45 otherwise).
- `ai memory list` / `ai memory search QUERY` — inspect the store.

The memory tools are `memory_add`, `memory_search`, `memory_get`, `memory_delete`.

## Memory maintenance (dream)

`ai dream` runs three steps over a work pool (`--jobs N`, config `dream.jobs`, default 4):

1. **extract** — distill durable facts from unprocessed transcript tuples.
2. **skills** — when `skills.auto_create` is true, review sessions with at least
   `skills.min_tuples` unreviewed exchanges and author skills.
3. **prune** — delete processed transcripts older than 7 days (skill-pending sessions are held).

It then **judges** entries unused and unjudged for 30 days. An interactive session offers to run
maintenance at exit when the backlog is large, and `dream.auto: true` runs it automatically.

## Skills

Skills are `SKILL.md` files (with YAML front matter `name`/`description`) discovered under the
`skills.dir` directory (default `<data-dir>/ai/skills`). They are listed in the system prompt and
loaded on demand with `load_skill`, which also returns the absolute paths of bundled files. The
skills folder is granted read access automatically.

With `skills.auto_create: true`, `ai dream` may create/update/delete skills it authored. AI skills
carry `origin: ai` in front matter and are the only ones the agent can modify (tagged
`(AI-created)` in the prompt). Manage them with `ai skills list` and `ai skills delete NAME`.

## Web tools and search

Web access is off unless granted. Use `--web` for everything, or narrow with `--web-fetch URL` and
`--web-search QUERY`. `web_fetch` retrieves and converts pages; `web_search` uses a configurable
ladder of providers (`search.providers`): keyed APIs (`brave`, `tavily`, `exa`, `serper`),
SearXNG (JSON API), and the keyless `duckduckgo`/`google`/`bing`. The first success wins; a listed
provider missing credentials is skipped.

### Brave Search API

Brave offers a free tier (a monthly query allowance with a rate limit; check current limits on the
site). To use it:

1. Create an account and get an API key at <https://brave.com/search/api/>.
2. Either set the environment variable `BRAVE_API_KEY`, or put the key in config:

   ```yaml
   search:
     providers:
       - name: brave          # api_key falls back to BRAVE_API_KEY
       - name: duckduckgo     # fallback
   ```

   Prefer `api_key: env:BRAVE_API_KEY` over a literal so the key stays out of the file.
3. Grant search access on the command line (`--web` or `--web-search '**'`).

`--probe-web QUERY` (hidden command) checks the configured search providers.

## Sessions

Sessions are JSON files under `session_dir` (default `<data-dir>/ai/sessions`). A session binds
its provider and model; resuming under a different pair starts a fresh session. `ai session list`
shows saved sessions (the current one marked `*`), and `ai session delete NAME` removes one.
In interactive mode: `/exit`, `/quit`, `/clear`, `/compact`, `/session`, `/help`.

## Configuration overview

Config lives at `~/.config/ai/config.yaml` (override with `-c/--config` or `AI_CONFIG`). Unknown
keys warn; `ai setup` uses strict parsing. See the full annotated example in the config reference
(ask for the `configuration` topic) and the setup help text. Highlights: `provider`, `model`,
`api_key`, `system_prompt`, `max_tokens`, `thinking`, `session_dir`, `skills.*`, `memory`,
`embedding_model`, `memory_max_distance`, `dream.*`, `policy`, `proxy`, `search.providers`,
`container.*`, `mcp.servers`.

## Building and customizing

Source: <https://github.com/dswd/ai>. Requires Rust (edition 2024).

```sh
git clone https://github.com/dswd/ai
cd ai
cargo build --release
./target/release/ai --version
```

Feature prerequisites:

- Default features are `browser` (Obscura headless browser) and `embed` (local ONNX embeddings).
- `browser` needs `cmake`, `clang`, `llvm-dev`, and `libssl-dev`.
- `embed` needs a C++ toolchain and network access at build time (ONNX Runtime is downloaded and
  linked); the Windows release is built without it because ONNX Runtime has no
  `x86_64-pc-windows-gnu` build.
- `cargo build --release --no-default-features` builds without both (smaller, fewer system deps).

Quality gates: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and
`cargo test`. After editing, run `cargo fmt` before committing.

## Troubleshooting/FAQ

- **"No matching rule" / a tool is missing** — grant it (`-r`, `-w`, `-x`, `--web`) or add a policy
  rule. `--ask` lets you approve at runtime.
- **Memory search returns nothing** — the embedding model may not be loaded yet (first run
  downloads it), or `memory_max_distance` is too strict; `ai memory list` shows stored facts.
- **A config key is ignored** — unknown keys warn; check spelling and nesting (e.g. `skills.dir`,
  not `skills_dir`).
- **Skills not listed** — confirm they are `SKILL.md` files under `skills.dir` with valid front
  matter; `ai skills list` shows what was discovered.
- **Which flags exist?** — ask for the `flags` topic or run `ai --help`.
- **Where is my data?** — sessions under `session_dir`, memory at `memory`, skills under
  `skills.dir`; defaults live under the platform data dir in `ai/`.
