# ai manual

A model-facing guide to the `ai` CLI. Project home: <https://github.com/dswd/ai>.

This manual is embedded into the binary and served by the `manual` tool. Pass a topic to get a
section, or no topic for the table of contents. Sections are `##` headings; subsections are `###`.

## Overview

`ai` is a single-binary Rust CLI agent. It sends a prompt to an LLM provider, and the model may
call tools (filesystem, shell, web, memory, skills, MCP) that are gated by a declarative policy
engine. Sessions persist the full chat log (tool calls and results included) so a run can be
resumed later.

The design principle is least privilege: the model starts with **only the prompt you give it** —
no filesystem, no network, no shell. Every capability is an explicit opt-in through CLI flags or a
policy file. Two things are on by default and can be turned off:

- **Semantic memory** (SQLite + local embeddings), disabled per run with `--no-memory`.
- **Session persistence**, disabled per run with `--no-session`.

### Mental model

```
        your prompt ──▶ ai ──▶ provider (OpenAI / Anthropic / compatible)
                        │
                        ├── policy decides which tools exist
                        ├── tools read/write files, run commands, fetch web
                        ├── memory injects relevant facts + scores
                        └── the session log is saved for next time
```

Five things to keep straight:

1. **Provider** — where model requests go (`provider`, `api_base`, `model`, `api_key`, `flavor`).
2. **Policy** — allow/deny rules for read, write, execute, web fetch, web search. The policy
   determines which tools the model even sees. First matching rule wins.
3. **Tools** — the actions the model can take. Only the ones the policy permits are registered.
4. **Sessions** — every run is saved; recent runs continue automatically.
5. **Memory** — durable facts and past exchanges, retrieved semantically and injected per message.

### Data locations

| What | Default path | Configured by |
| --- | --- | --- |
| Config | `~/.config/ai/config.yaml` | `-c`/`--config` |
| Sessions | `<data-dir>/ai/sessions` | `session_dir` |
| Memory DB | `<data-dir>/ai/memory.db` | `memory` |
| Embedding cache | `<cache-dir>/ai/fastembed` | — |
| Skills | `<data-dir>/ai/skills` | `skills.dir` |
| Policy file | `<config-dir>/ai/policy` | `policy` |

`<data-dir>` is the platform data dir (on Linux `~/.local/share`), `<cache-dir>` the platform cache
dir (on Linux `~/.cache`).

### Where to start

- First time? Run `ai setup`.
- Quick question? `ai run "..."`.
- Working session? `ai` (interactive), optionally `-s NAME`.
- Need file/shell/web access? Add `-r`, `-w`, `-x`, `--web`.
- Curious about `ai` itself? Ask, or call the `manual` tool.

## Getting started

### Install

Prebuilt binaries are published for Linux (amd64/arm64) and Windows. On macOS, build from source.

```sh
# user install to ~/.bin (adds it to PATH)
curl -fsSL https://dswd.github.io/ai/install.sh | sh

# system-wide to /usr/local/bin
curl -fsSL https://dswd.github.io/ai/install.sh | sudo sh -s -- --global
```

From source (requires Rust, edition 2024):

```sh
git clone https://github.com/dswd/ai
cd ai
cargo build --release
./target/release/ai --version
```

### Configure

```sh
ai setup
```

`ai setup` runs a short wizard: choose a provider (or reuse an existing config or detected
credentials), pick a model — optionally browsing the models.dev catalog — confirm the connection,
then let the AI finish the rest of the config. Your API key never reaches the model, and an
existing config is backed up to `config.yaml.bak` first.

### First prompts

```sh
ai run "explain what a race condition is"
ai run "summarize this changelog" < CHANGELOG.md
cat access.log | ai run "group these errors by type"
ai run -r ./src "where is the retry logic?"
```

### Interactive sessions

```sh
ai                      # continue the newest session if it is < 60 minutes old
ai -s refactor          # named session; resumes it if it exists
ai -s refactor -r ./ -w ./ -x cargo,git
```

Inside a session: `/help`, `/session`, `/compact`, `/clear`, `/exit`. Type plain text to talk to
the model.

### Everyday recipes

```sh
# inspect a document
ai run -r ./ "summarize the key risks in this contract"     # uses file_view on PDFs

# repo chores with a container for safety
ai run -X -r ./ -w ./ "run the tests and fix the failures"

# research
ai run --web "compare the current Rust and Go release cadences"

# recurring tasks with skills
ai run "cut a release"            # loads the 'release' skill if one exists
ai skills list
```

### How a one-off run behaves

- It reads the prompt from arguments, or from stdin if no prompt is given.
- It continues the newest session when it is under 60 minutes old and matches provider/model;
  otherwise it starts a new one.
- It saves the session only after a successful run.
- `--no-session` makes it fully stateless (no read, no write).

## Commands and flags

### Commands

| Command | What it does |
| --- | --- |
| `ai` | Start an interactive session (default with no subcommand). |
| `ai run [PROMPT]...` | One-shot request. Reads stdin when PROMPT is absent. |
| `ai session list` | List saved sessions (current marked `*`). |
| `ai session delete NAME` | Delete a session by name. |
| `ai memory list` | List stored memory facts (id, tags, origin, dates). |
| `ai memory search QUERY` | Semantic search over memory + transcripts, with scores. |
| `ai skills list` | List discovered skills with origin and path. |
| `ai skills delete NAME` | Delete a skill folder (SKILL.md + bundled files). |
| `ai dream [--jobs N]` | Run memory maintenance (extract → skills → prune → judge). |
| `ai setup [FILE]` | Interactive provider/model wizard + AI config editing. |
| `ai completions SHELL` | Print a shell completion script and exit. |
| `ai probe-web QUERY` | Hidden: test the configured search providers. |

Examples:

```sh
ai run "hello"
cat logs.txt | ai run
ai session list
ai session delete old-experiment
ai memory search "how do we deploy?"
ai skills delete obsolete-skill
ai dream --jobs 8
ai completions zsh > ~/.zsh/completions/_ai
```

### Global flags

Value flags accept `--flag value` and `--flag=value`. Short flags accept `-f value` and `-f=value`.

| Flag | Meaning |
| --- | --- |
| `-c`, `--config FILE` | Load config from FILE. |
| `--provider PROVIDER` | Override the provider id. |
| `--model MODEL` | Override the model id. |
| `--system PROMPT` | Override the system prompt. |
| `-s`, `--session-name NAME` | Name the session / resume that name (value required). |
| `--no-session` | Run stateless; conflicts with `-s`. |
| `--no-memory` | Disable memory for this run. |
| `-r`, `--read PATH` | Grant read on PATH (repeatable; globs allowed). |
| `-w`, `--write PATH` | Grant read+write on PATH (repeatable). |
| `-x`, `--execute PATTERN` | Allow running commands matching PATTERN (repeatable, comma-separated). |
| `--web` | Allow all web fetch and search. |
| `--web-fetch PATTERN` | Allow fetching URLs matching PATTERN (repeatable). |
| `--web-search PATTERN` | Allow searches matching PATTERN (repeatable). |
| `--proxy URL` | Route web requests via HTTP or SOCKS5 proxy. |
| `-p`, `--policy FILE` | Load allow/deny rules from FILE. |
| `-i`, `--ask` | Ask for approval instead of denying unmatched actions. |
| `-t`, `--tool URL` | Connect an MCP tool server (repeatable). |
| `-y`, `--yolo` | Allow everything. Dangerous. |
| `-X`, `--container [IMAGE]` | Run external commands in a container. |
| `--container-runtime RUNTIME` | `auto` (default), `docker`, or `podman`. |
| `--no-container` | Force host execution even if a container is configured. |
| `--max-tokens N` | Cap output tokens. |
| `--max-turns N` | Cap agent turns (tool rounds; default 100). |
| `--thinking [TOKENS]` | Enable extended reasoning (default 16000 tokens). |
| `-v`, `--verbose` | Debug logging. |
| `-q`, `--quiet` | Suppress thinking output and token stats. |
| `--no-color` | Disable ANSI colors (also honors `NO_COLOR`). |

### Notes and pitfalls

- Bare `ai "prompt"` is **not** a one-off; use `ai run "prompt"`. Bare `ai` is interactive.
- `-s` always takes a value; it no longer starts a random session by itself.
- `-w` implies `-r` on the same path (write access needs read).
- `-x` governs external commands only; bashkit builtins are gated by read/write policy instead.
- `--yolo` relaxes **tool** policy; if a container is configured it stays in effect.

## Configuration

Config is YAML, normally at `~/.config/ai/config.yaml`, overridable with `-c`. Every field is
optional. Unknown keys print a warning and are ignored; `ai setup` rejects them so the setup AI
cannot invent options.

### Full example

```yaml
provider: openai
flavor: openai                     # derived from the provider table when omitted
api_key: env:OPENAI_API_KEY        # literal or env:VAR
api_base: https://api.openai.com/v1
model: gpt-4o

system_prompt: "You are a concise, helpful CLI assistant."
max_tokens: 4096
thinking: 16000                    # Anthropic-flavored providers only
context_window: 128000             # used by the interactive usage indicator

session_dir: ~/.local/share/ai/sessions

policy: ~/.config/ai/policy        # allow/deny file (created on first persisted rule)

memory: ~/.local/share/ai/memory.db
embedding_model: multilingual-e5-small
memory_max_distance: 0.175

skills:
  dir: ~/.local/share/ai/skills
  auto_create: false
  min_tuples: 10

dream:
  auto: false
  jobs: 4

proxy: socks5h://127.0.0.1:1080

search:
  providers:
    - name: brave
    - name: duckduckgo

container:
  # default_image: "alpine:latest"
  runtime: auto
  network: policy

mcp:
  servers:
    - url: https://example.com/mcp
      name: example
```

### Field reference

| Field | Default | Notes |
| --- | --- | --- |
| `provider` | `openai` | Built-in id or any models.dev id. |
| `flavor` | derived | `openai` or `anthropic` request shape. |
| `api_key` | — | Literal or `env:VAR`. |
| `api_base` | provider default | Required for custom endpoints. |
| `model` | `gpt-4o` | Model id sent to the provider. |
| `system_prompt` | built-in | Prepended to every conversation. |
| `max_tokens` | provider default | Output cap. |
| `thinking` | off | Extended-thinking budget. |
| `context_window` | provider | Used for the usage indicator and pruning. |
| `session_dir` | `<data-dir>/ai/sessions` | Session storage. |
| `policy` | `<config-dir>/ai/policy` | Policy file (loaded when it exists; created on first persisted rule). |
| `memory` | `<data-dir>/ai/memory.db` | SQLite database (or legacy `*.json`). |
| `embedding_model` | `multilingual-e5-small` | Local embedding model. |
| `memory_max_distance` | `0.175` | Cosine distance cutoff (E5 default). |
| `skills.dir` | `<data-dir>/ai/skills` | Skill discovery root. |
| `skills.auto_create` | `false` | Let `ai dream` author skills. |
| `skills.min_tuples` | `10` | Review threshold. |
| `dream.auto` | `false` | Run maintenance at session end. |
| `dream.jobs` | `4` | Parallel maintenance requests. |
| `proxy` | env | Web proxy URL. |
| `search.providers` | DuckDuckGo→Google→Bing | Ordered search backends. |
| `container.default_image` | — | Image for external commands. |
| `container.runtime` | `auto` | `auto`, `docker`, `podman`. |
| `container.network` | `policy` | `policy`, `none`, `host`. |
| `mcp.servers` | — | External MCP servers. |

### Environment variables

API keys may be referenced with `env:VAR` so the secret stays out of the file:

| Variable | Used by |
| --- | --- |
| `OPENAI_API_KEY` | OpenAI |
| `ANTHROPIC_API_KEY` | Anthropic |
| `OLLAMA_API_KEY` | Ollama |
| `GROQ_API_KEY` | Groq |
| `GEMINI_API_KEY` | Google Gemini |
| `DEEPSEEK_API_KEY` | DeepSeek |
| `MISTRAL_API_KEY` | Mistral |
| `OPENROUTER_API_KEY` | OpenRouter |
| `XAI_API_KEY` | xAI |
| `BRAVE_API_KEY` / `TAVILY_API_KEY` / `EXA_API_KEY` / `SERPER_API_KEY` | Web search |
| `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY` | Web requests |
| `NO_COLOR` | Disable ANSI colors |

Provider-specific env vars are used as a fallback when `api_key` is unset, even for
OpenAI-compatible custom providers.

### Precedence

For any setting, the effective value is chosen in this order:

1. Command-line flag (e.g. `--model`, `-r`, `--config`).
2. Config value.
3. Provider default / built-in default.

### Migrations and warnings

- Old `skills_dir` moved to `skills.dir`; the old key warns and is ignored.
- `dream_jobs` moved to `dream.jobs`.
- Legacy `memory.json` is migrated once to `memory.db` and renamed `memory.json.bak`.
- Unknown keys warn (and fail `ai setup` strict parsing).

## Providers

`ai` speaks two request shapes, called **flavors**:

- `openai` — the OpenAI Chat Completions shape.
- `anthropic` — the Anthropic Messages shape.

Any provider that speaks one of these shapes works. Built-in providers (OpenAI, Anthropic, Ollama,
Groq, DeepSeek, Google Gemini, Mistral, OpenRouter, xAI, llamafile) have sensible defaults. For
anything else, set `provider`, `api_base`, `flavor`, and `model`; or pick any entry from the
[models.dev](https://models.dev) catalog by its id in `ai setup`.

### Examples

OpenAI:

```yaml
provider: openai
api_key: env:OPENAI_API_KEY
model: gpt-4o
```

Anthropic:

```yaml
provider: anthropic
api_key: env:ANTHROPIC_API_KEY
model: claude-sonnet-4-20250514
```

Local Ollama (OpenAI-compatible):

```yaml
provider: openai
flavor: openai
api_base: http://localhost:11434/v1
api_key: ollama
model: llama3.2
```

A private gateway:

```yaml
provider: openai-compatible
api_base: https://gateway.internal/v1
api_key: env:GATEWAY_KEY
model: internal-large
```

### Switching providers

Keep several config files and choose one per run:

```sh
ai run -c ~/.config/ai/anthropic.yaml "hello"
ai run -c ~/.config/ai/ollama.yaml "hello"
```

A session remembers its provider and model; resuming under a different pair forks a fresh session
instead of replaying an incompatible history.

### Thinking support

Extended thinking (`--thinking`, `thinking:`) is only sent to Anthropic-flavored providers; other
providers ignore it with a warning.

## Permissions and policy

The policy engine decides what the model may touch. Rules are evaluated **first-match-wins**: the
first rule that matches the action and target decides. CLI rules are evaluated before policy-file
rules, so `-r`/`-w`/`-x`/`--web` always win.

### Actions

| Action | Governs |
| --- | --- |
| `read` | `read_file`, `list_dir`, `search_content`, `find_files`, `file_info`, `file_view` |
| `write` | `write_file`, `replace_in_file`, `delete_file`, `create_directory`, `move_file`, `copy_file` |
| `execute` | external commands run by `execute` |
| `web fetch` | `web_fetch`, `download_file`, browser tools |
| `web search` | `web_search` |

If no allow rule covers an action at startup, the corresponding tools are not registered, and the
system prompt lists the missing permission with the flag that enables it.

### Syntax

```text
allow <action> <pattern>
deny  <action> <pattern>
```

Blank lines and `#` comments are ignored. Actions are `read`, `write`, `execute`, `web-fetch`,
and `web-search` (`web fetch`/`web search` with a space are *not* accepted).

```text
# read a project tree but never its secrets
allow read ~/projects/**
deny  read ~/projects/**/secrets/**

# allow a handful of commands
allow execute cargo,npm,git,rustc

# web requests, narrowly
allow web-fetch https://docs.rs/**
allow web-fetch https://api.github.com/**
deny  web-search *confidential*
```

### Path matching

- `*` matches within one path segment; `**` matches across segments.
- A bare directory matches the directory and everything under it (`/tmp` matches `/tmp/a/b` but
  not `/tmpfile`).
- `~` expands to your home directory.
- Relative patterns resolve against the home directory for policy files and against the current
  directory for CLI flags.
- Paths are resolved (symlinks included) before matching, so `deny` rules nested inside an allowed
  tree are honored and a symlink cannot escape an allowed root.

### Execute patterns

- Built-in commands (echo, cat, grep, sed, awk, find, tar, curl, git, …) run inside bashkit and are
  governed by read/write policy, not the execute list.
- External commands (cargo, npm, python, …) are matched against `execute` allow rules.
- Patterns are comma-separated command names, e.g. `-x cargo,git`.
- If a container is active, external commands run inside it; the container is the boundary.

### Interactive ask mode

With `-i`/`--ask` (or automatically in an interactive session), an action with no matching rule
prompts you:

```text
Allow read for /home/me/project/src/auth.rs? [y=once, r=rule, N=deny]
```

- `y` — allow this one time; nothing is remembered.
- `r` — build a reusable rule (see below).
- anything else — deny.

Rules created here only ever apply to a target nothing else covers, and the usual first-match-wins
order still holds, so a created `deny` cannot override an existing `allow`.

#### Creating a rule

Choosing `r` walks through three steps:

1. **Direction** — `Create an allow or deny rule? [a=allow, d=deny]`.
2. **Subject** — the exact target is pre-filled and editable, so you can widen it with wildcards:

   ```text
   Rule subject (edit as needed): /home/me/project/src/**
   ```

   Path subjects are normalized like `-r`/`-w` (`~` expands, relative paths resolve against the
   current directory, `.`/`..` collapse) and stored absolute. URLs, search queries, and commands
   are stored as typed.
3. **Persistence** — the exact rule line is shown and you choose whether to keep it:

   ```text
   Persist "allow read /home/me/project/src/**" to /home/me/.config/ai/policy? [y/N]
   ```

   - **yes** — the rule is appended to the effective policy file, so it survives restarts.
   - **no** (default) — the rule is remembered for this session only.

If the finished rule does not actually cover the request that triggered it, ai says so and asks
again. Cancelling any step (Ctrl-C/Ctrl-D/empty) returns to the original prompt.

The effective policy file is `--policy` if given, otherwise `config.policy`, otherwise the default
`<config-dir>/ai/policy` (e.g. `~/.config/ai/policy`). Persisting appends a single line there,
creating the file and its directories if needed; the config file itself is never rewritten. If the
write fails, the rule is kept for the session and a warning is printed.

`ai setup`'s config-save approval does **not** offer rule creation — it is a plain yes/no.

### Common recipes

```sh
# read-only exploration
ai run -r . "explain this codebase"

# edit in place
ai run -w ./src -r ./Cargo.toml "add a --json flag"

# run tools but only git and cargo
ai run -x git,cargo "run clippy"

# research with a narrow fetch allowlist
ai run --web-fetch 'https://docs.rs/**' --web-search '**' "summarize serde"

# trusted automation (be careful)
ai run -y "do whatever it takes"
```

### Why a tool was denied

If the model says it lacks a permission, the fix is one of:

- Add the matching flag (`-r`, `-w`, `-x`, `--web`), or
- Add an `allow` rule to the policy file, or
- Re-run with `--ask` to approve interactively, or
- Re-run with `-y` for unrestricted access (dangerous).

## Filesystem and containers

### The sandbox contract

Every user-supplied path an action touches is passed through one filesystem layer that:

1. Resolves the path (canonicalizes it, following symlinks; for not-yet-existing paths it
   canonicalizes the deepest existing ancestor).
2. Consults the policy for the requested action on the resolved path.
3. Performs the operation on the resolved path.

This means `deny` rules inside an allowed tree hold, and a symlink inside an allowed directory
cannot redirect a read or write outside the granted roots. Metadata-only checks (existence and
type) are intentionally not gated, so shell PATH lookups do not request a read grant per entry;
file contents and directory listings remain gated.

### Builtins vs external commands

`execute` runs through bashkit's virtual bash:

- 150+ built-in commands run in-process; no process is forked.
- External commands fork-exec and require an `execute` allow rule.
- With a container configured, the whole command string is sent to the container's shell instead.

### Containers

```sh
ai run -X "run the test suite"                    # default debian:stable-slim
ai run -X alpine:latest -r ./ "build the project"
ai run -X --container-runtime podman "..."        # force podman
ai run --no-container "..."                       # run on the host
```

What containers change:

- A single session container starts at agent startup and every `execute` command runs inside it.
- Policy read roots are bind-mounted read-only; write roots read-write.
- The host filesystem is otherwise invisible inside the container.
- The container gets no network unless web access is granted (`container.network: policy`).
- The container is removed on exit (including Ctrl-C).

Caveats:

- `--yolo` does not disable the container; it only relaxes tool policy.
- Whole-filesystem grants (`**`) cannot be bind-mounted (they would shadow the root filesystem)
  and are warned about.
- `-x` still controls whether an external command is allowed; the container controls what it can
  reach once running.

### When not to use a container

Containers need Docker or Podman and add startup overhead. For trusted, local, high-frequency
tasks, running on the host without `-X` is faster. The container is most valuable for untrusted
code, generated scripts, and network-adjacent work.

## Sessions

Sessions are JSON files under `session_dir`. Each records the full message log (schema v2: user,
assistant, tool calls and tool results), the system prompt, provider, model, and timestamps.

### Commands

```sh
ai session list
ai session delete my-session
```

### Naming and resuming

- `ai -s NAME` names the session; if a session with that name exists it is continued.
- An undated `NAME` is a cross-date alias: it matches the most recent session named `NAME` or
  `YYYY-MM-DD_NAME` and creates `YYYY-MM-DD_NAME` when none exists.
- A dated `--session-name YYYY-MM-DD_NAME` is matched exactly.
- `ai session delete NAME` resolves names the same way.

### Automatic continuation

`ai run` and bare `ai` continue the newest session when all of these hold:

- It was modified within the last 60 minutes.
- Its provider and model match the current configuration.
- Its schema version is current.

Otherwise a new session starts. One-off sessions are saved only after a successful run.

### Stateless runs

`--no-session` reads and writes nothing to disk. No history is replayed and nothing is saved. It
conflicts with `-s`.

### In-session commands

| Command | Effect |
| --- | --- |
| `/exit`, `/quit` | End the session. |
| `/clear` | Clear the message history. |
| `/compact` | Summarize the conversation and replace history with the summary. |
| `/session` | Print the current session name. |
| `/help` | List slash commands (and remind you to just ask about `ai`). |

### Provider/model binding

A session is bound to the provider and model that produced it. Resuming under a different pair
starts a fresh session to avoid mixing incompatible histories. To intentionally switch mid-project,
start a new session (or let `ai` do it when it detects the mismatch).

### Context usage

When `context_window` is known, the prompt shows usage as `[NN%]`, colored by tier. The history
sent to the model is pruned of stale tool output; the saved session keeps the originals.
`/compact` remains the manual fallback for long conversations.

## Memory

Memory stores durable facts and a transcript index, and injects relevant items into the prompt.

### How it works

- Facts live in a SQLite database (`memory.db`), with text, tags, origin (`user`/`agent`),
  timestamps, and provenance.
- Facts and completed conversation exchanges are embedded **locally** with fastembed (ONNX).
  Embeddings are stored in sqlite-vec (`vec0`) tables and searched by cosine similarity.
- On each user message, the query is embedded and the closest memory facts and transcript excerpts
  are retrieved. Items within the distance cutoff are injected, memory above transcripts, capped
  at a small number of hits.
- Transcript excerpts from the **session in progress are excluded**, so a resumed or long session
  never re-injects its own conversation. Excerpts from other sessions stay retrievable. Memory
  maintenance (`ai dream`, including the end-of-session auto-dream) is unaffected: it reviews every
  session, including the one that just ended.
- Each injected item is shown with a cosine **match score** (higher is better).

Nothing leaves your machine for retrieval; the embedding model is downloaded once to the cache dir.

### Tools

| Tool | Purpose |
| --- | --- |
| `memory_add` | Store a fact (`data`, optional `tags`, optional `origin`). Upserts near-duplicates. |
| `memory_search` | Semantic search over memory + transcripts. |
| `memory_get` | Fetch one entry by key. |
| `memory_delete` | Delete an entry by key. |

### CLI

```sh
ai memory list
ai memory search "how does the deploy pipeline work?"
```

`ai memory search` prints each hit's kind, key, tags/origin, date, and score.

### Configuration

- `memory` — database path. Default `<data-dir>/ai/memory.db`.
- `embedding_model` — one of `multilingual-e5-small` (default), `multilingual-e5-base`,
  `multilingual-e5-large`, `paraphrase-multilingual-minilm-l12-v2`,
  `paraphrase-multilingual-mpnet-base-v2`, `bge-small-en-v1.5`, `all-minilm-l6-v2`.
- `memory_max_distance` — cosine distance cutoff. Default `0.175` for E5 models, `0.45` otherwise.
  Lower is stricter; raise it if relevant items are missed.

Changing the embedding model rebuilds the vector index and re-embeds rows on first use.

### Transcript index

Every save indexes the session's completed user/agent exchanges. Re-indexing is idempotent:
changed exchanges are re-embedded, and rows beyond a cleared or compacted log are dropped. A
one-time backfill indexes pre-existing session files.

### Disabling memory

`--no-memory` disables both reading and writing memory for a run. Facts already stored are
untouched.

### Troubleshooting

- **Search returns nothing**: the model may still be downloading on first use, or the cutoff is too
  strict. Check `ai memory list` for stored facts.
- **A fact is not injected**: distances are model-relative; try raising `memory_max_distance`.
- **Too many weak hits**: lower `memory_max_distance`.
- **Different language**: switch to a multilingual E5 model.

## Memory maintenance (dream)

`ai dream` keeps memory healthy. It runs over a work pool, so independent batches proceed in
parallel.

### Steps

1. **extract** — unprocessed transcript tuples are batched (10 from one session per request) and
   sent to the model to distill durable facts via `memory_add`. Successful batches are marked
   processed.
2. **skills** — when `skills.auto_create` is on, sessions with at least `skills.min_tuples`
   unreviewed exchanges are reviewed, and the model may create/update/delete skills.
3. **prune** — processed transcripts older than 7 days are deleted. When skill auto-creation is on,
   sessions large enough to be reviewed are held back until reviewed, so pending candidates are not
   lost.
4. **judge** — entries unused and unjudged for 30 days are reviewed with similar-but-newer
   neighbours as evidence and hard-deleted when obsolete.

### Running it

```sh
ai dream                # uses dream.jobs (default 4)
ai dream --jobs 8       # more parallel requests
```

At the end of an interactive session, `ai` offers to run maintenance when more than 50 tasks are
pending (default: No). With `dream.auto: true`, it runs automatically after every saved session.
Both use the configured provider and model, block until done, and never fail the session exit.

### What is skipped

One-off `ai run`, piped stdin, `--no-session`, empty sessions, and `--no-memory` runs do not
trigger maintenance.

### Costs

Maintenance makes LLM requests; batch sizes and the job count bound the cost. Lower `dream.jobs`
for a slower, cheaper run.

## Skills

A skill is a Markdown file with YAML front matter that teaches the model a repeatable procedure.

### Format

`SKILL.md` with `name` and optional `description`:

```markdown
---
name: release
description: Cut a release of this project
---

1. Bump the version in Cargo.toml
2. Update CHANGELOG.md
3. Commit, tag vX.Y.Z, and push
```

The file may be accompanied by other files (scripts, references) in the same folder. Loading the
skill returns the body plus the absolute paths of those bundled files so the model can read them.

### Discovery

- Root: `skills.dir` (default `<data-dir>/ai/skills`).
- A skill is any file named exactly `SKILL.md`, at any depth.
- The name comes from front matter, falling back to the containing folder name.
- Duplicate names are resolved first-wins; the rest are ignored with a warning.

### Using skills

- Skills are listed in the system prompt by name and description.
- The model loads one with the `load_skill` tool.
- The skills folder is granted read access automatically, so the model can read a skill's bundled
  files with `read_file`.

### Authoring skills

Write them by hand, or let `ai dream` author them when `skills.auto_create` is true:

```yaml
skills:
  auto_create: true
  min_tuples: 10
```

AI-created skills are tagged `origin: ai` in front matter, marked `(AI-created)` in the prompt, and
are the only skills the agent may modify or delete. This is an accident-prevention boundary, not a
security boundary — treat AI-authored skills as untrusted text you can edit or remove.

### Managing skills

```sh
ai skills list
ai skills delete NAME
```

`delete` removes the skill's folder (SKILL.md plus bundled files). A `SKILL.md` that sits directly
in the skills root deletes only the file, never the whole tree.

### Writing good skills

- Give a crisp `description` describing when to use it.
- Keep the body as ordered, actionable steps.
- Put scripts and long references in the folder and mention them by relative path.
- Never embed secrets in a skill.

## Web tools and search

Web access is off unless granted.

### Granting access

```sh
ai run --web "..."                                   # everything
ai run --web-fetch 'https://docs.rs/**' "..."        # fetch only, narrow
ai run --web-search '**' --web-fetch 'https://x/**'  # search broadly, fetch narrowly
```

### web_fetch

Fetches a URL and returns text, Markdown, or HTML. HTML is converted to readable text. Responses
are capped (5 MB) and support offset/limit pagination.

### web_search

Search uses an ordered ladder from `search.providers`; the first provider that succeeds wins.

| Provider | Needs |
| --- | --- |
| `brave` | `BRAVE_API_KEY` or `api_key` |
| `tavily` | `TAVILY_API_KEY` or `api_key` |
| `exa` | `EXA_API_KEY` or `api_key` |
| `serper` | `SERPER_API_KEY` or `api_key` |
| `searxng` | `url` |
| `duckduckgo`, `google`, `bing` | nothing (scraped) |

A listed provider missing credentials is skipped with a warning. When unset, the default ladder is
DuckDuckGo → Google → Bing. `google` and `bing` need the browser feature.

### Brave Search API

Brave has a free tier.

1. Create a key at <https://brave.com/search/api/>.
2. Set `BRAVE_API_KEY`, or put `api_key: env:BRAVE_API_KEY` in the provider entry.
3. Add `- name: brave` to `search.providers` and grant search access.

```yaml
search:
  providers:
    - name: brave
    - name: duckduckgo
```

### Browser automation

For JavaScript-heavy pages, login flows, and dynamic content, the model can drive a stealth
headless browser: `browser_navigate`, `browser_click`, `browser_get_content`,
`browser_get_element`, `browser_evaluate`. These require web-fetch permission on the URL.

### Proxy

`--proxy URL` or the `proxy` config key routes web requests through HTTP/HTTPS or SOCKS5. The
standard `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` variables are honored too.

### Diagnostics

`ai probe-web QUERY` tests the configured search providers and reports which one answers.

## MCP tool servers

MCP (Model Context Protocol) servers extend the agent with external tools — IDE integrations,
database agents, API gateways.

### From the command line

```sh
ai run -t https://example.com/mcp "what tables exist?"
ai run -t http://localhost:8081 -t http://localhost:8082 "query the inventory"
```

### From the config

```yaml
mcp:
  servers:
    - url: https://example.com/mcp
      name: example
```

### Merge and failure behavior

- `--tool` URLs and config servers are merged and deduplicated by URL (CLI first).
- A config server that fails to connect is skipped with a warning.
- A `--tool` server that fails to connect aborts startup.

The transport is streamable HTTP.

## Built-in tools reference

Tools are registered only when policy permits.

### Filesystem

| Tool | Params | Notes |
| --- | --- | --- |
| `read_file` | `path`, `offset?`, `limit?` | Read file contents. |
| `write_file` | `path`, `content` | Create/overwrite; makes parent dirs. |
| `list_dir` | `path`, `offset?`, `limit?` | Types: `d`, `l`, `f`. |
| `replace_in_file` | `path`, `old_str`, `new_str` | Exact, single match. |
| `delete_file` | `path`, `recursive?` | Directories need `recursive`. |
| `create_directory` | `path` | Like `mkdir -p`. |
| `file_info` | `path` | Size, mode, mtime, type. |
| `move_file` | `source`, `destination` | Rename/move. |
| `copy_file` | `source`, `destination` | Copy; makes parent dirs. |
| `file_view` | `path` | Extract text from PDF/DOCX/XLSX/PPTX/HTML/CSV/images/ZIP. |

### Execution

| Tool | Params | Notes |
| --- | --- | --- |
| `execute` | `command`, `cwd?`, `offset?`, `limit?`, `timeout?` | bashkit builtins in-process; external commands by grant or container. |

### Search

| Tool | Params | Notes |
| --- | --- | --- |
| `search_content` | `path`, `pattern`, `file_types?`, `offset?`, `limit?` | Regex; skips hidden dirs and binaries; max 500 matches. |
| `find_files` | `path`, `pattern`, `offset?`, `limit?` | Glob; sorted; truncated at 500. |

### Web

| Tool | Params | Notes |
| --- | --- | --- |
| `web_fetch` | `url`, `format?`, `timeout?`, `offset?`, `limit?` | Max 5 MB. |
| `web_search` | `query`, `offset?`, `limit?` | Provider ladder + browser fallback; max 20. |
| `download_file` | `url`, `path` | Max 5 MB; needs write on `path`. |

### Browser

`browser_navigate(url)`, `browser_click(selector, url?)`, `browser_get_content(format?, offset?,
limit?)`, `browser_get_element(selector, format?, offset?, limit?)`,
`browser_evaluate(expression, url?)`.

### Memory, skills, and utility

| Tool | Params | Notes |
| --- | --- | --- |
| `memory_add` | `data`, `tags?`, `origin?` | Upserts near-duplicates. |
| `memory_search` | `query`, `limit?` | Semantic; with scores. |
| `memory_get` | `key` | Fetch one entry. |
| `memory_delete` | `key` | Delete one entry. |
| `load_skill` | `name` | Body + bundled file paths. |
| `manual` | `topic?` | This documentation. |
| `get_current_time` | — | Current date/time. |
| `exit_program` | — | End an interactive session on request. |

Dream-only tools (not available in normal runs): `skill_create`, `skill_update`, `skill_delete`.
Setup-only tool: `write_config`.

## Inline help

The agent can answer questions about `ai` itself using the `manual` tool, which serves this
document, the example config, and generated CLI help.

```sh
ai run "how do I allow running cargo?"
ai run "what does memory_max_distance do?"
ai run "which flags enable web search?"
```

- Call `manual` with no topic for a table of contents.
- `manual("flags")` returns the generated CLI help; a subcommand name returns that subcommand's
  help; `manual("configuration")` returns the annotated example config.
- Any other topic returns matching sections from this manual and the README.

In a session, `/help` prints the slash commands and reminds you that you can just ask.

## Output, thinking, and terminal behavior

### Rendering

- Assistant Markdown is rendered to ANSI on a terminal and emitted as raw Markdown when piped.
- Tool calls and thinking output go to stderr, so stdout stays pipe-friendly.
- A first-token spinner shows while waiting for the model.
- `--no-color` or `NO_COLOR` disables styling.

### Quiet and verbose

```sh
ai run -q "2+2"     # no thinking stream, no token stats
ai run -v "..."     # debug logging
```

### Token stats

Non-quiet runs print a usage line as `(in, thinking, out)`.

### Extended thinking

```sh
ai run --thinking "design a rate limiter"           # default 16000 tokens
ai run --thinking 32000 "audit this codebase"
```

Thinking is only sent to Anthropic-flavored providers.

### Prompt

The interactive prompt shows the session name (date prefix dropped), the active container
(`@ image`), and context usage `[NN%]`, ending in a `❯` caret.

## Troubleshooting / FAQ

**A tool is missing or "access denied".**
Add the matching permission: `-r` (read), `-w` (write), `-x` (execute), `--web` or narrow
`--web-fetch`/`--web-search`; or add a policy rule; or run with `--ask` / `-y`.

**`ai "..."` did nothing useful.**
Bare `ai "..."` is interactive. Use `ai run "..."`.

**How do I use a different config?**
`ai -c path/to/config.yaml ...`, or `ai setup path/to/config.yaml`.

**Where is my data?**
Sessions in `session_dir`; memory at `memory`; skills under `skills.dir`. Defaults live under the
platform data dir in `ai/`.

**How do I switch provider/model?**
Change `provider`/`model` in config or pass `--provider`/`--model`. Existing sessions bind to their
original pair, so a change starts a fresh session.

**The model keeps forgetting things.**
Memory retrieval only injects relevant hits. Store facts with `memory_add` (or let `ai dream`
extract them), and check `ai memory search`.

**`ai dream` didn't run at exit.**
It only offers when more than 50 tasks are pending; set `dream.auto: true` to always run, or call
`ai dream` directly. One-off and `--no-memory` sessions are skipped.

**A skill isn't listed.**
Confirm it is a `SKILL.md` under `skills.dir` with valid front matter; run `ai skills list`.

**Which flags exist?**
Ask `manual("flags")` or run `ai --help`.

**How do I install a specific version or build an older one?**
Prebuilt binaries come from the Releases page; for anything else, clone the repo at the tag and
`cargo build --release`.

**How do I get a shell completion?**
`ai completions bash|zsh|fish` and install it where your shell looks for completions.

**Is my data sent anywhere?**
Only your prompt and the model calls you configure go to the provider. Embeddings and memory
retrieval run locally.

## Building and customizing

Source: <https://github.com/dswd/ai>. Requires Rust (edition 2024).

```sh
git clone https://github.com/dswd/ai
cd ai
cargo build --release
./target/release/ai --version
```

### Feature prerequisites

- Default features are `browser` (Obscura headless browser) and `embed` (local ONNX embeddings).
- `browser` needs `cmake`, `clang`, `llvm-dev`, and `libssl-dev`.
- `embed` needs a C++ toolchain and network access at build time; ONNX Runtime is downloaded and
  linked. The Windows release is built without it because ONNX Runtime has no
  `x86_64-pc-windows-gnu` build.
- `cargo build --release --no-default-features` builds without both (smaller, fewer system deps).
  Without `embed`, facts still store but semantic retrieval returns nothing.

### Quality gates

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Run `cargo fmt` after editing and before committing.

### Documentation

- `README.md` — user overview and installation.
- `docs/manual.md` — this manual (embedded into the `manual` tool).
- `config.example.yaml` — annotated configuration reference.
- `CHANGELOG.md` — release history.
- `AGENTS.md` — contributor/agent guidance.
