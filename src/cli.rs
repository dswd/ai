use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;
use std::ops::Deref;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "ai",
    version = env!("CARGO_PKG_VERSION"),
    about = "CLI interface for interacting with AI models",
    long_about = "AI Tool interacts with AI models using tools gated by a policy engine.\n\
                  Without a subcommand it starts an interactive session; `ai run` performs a\n\
                  single one-shot request. Thinking output and tool calls are printed to stderr.",
    after_help = "Environment Variables:\n  \
                  OPENAI_API_KEY, ANTHROPIC_API_KEY, OLLAMA_API_KEY,\n  \
                  GROQ_API_KEY, GEMINI_API_KEY, OPENAI_BASE_URL, etc."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub agent: AgentArgs,

    /// Whether this invocation runs an interactive session (set from `command`).
    #[arg(skip)]
    pub interactive: bool,

    #[arg(
        short = 'c',
        long = "config",
        help = "Load configuration from FILE",
        value_name = "FILE",
        global = true
    )]
    pub config: Option<PathBuf>,

    #[arg(
        long = "provider",
        help = "Override the provider",
        value_name = "PROVIDER",
        global = true
    )]
    pub provider: Option<String>,

    #[arg(
        long = "model",
        help = "Override the model",
        value_name = "MODEL",
        global = true
    )]
    pub model: Option<String>,

    #[arg(
        long = "max-turns",
        help = "Set the maximum number of agent turns (tool call rounds)",
        value_name = "N",
        default_value = "100",
        global = true
    )]
    pub max_turns: usize,

    #[arg(
        short = 'v',
        long = "verbose",
        help = "Enable verbose mode",
        conflicts_with = "quiet",
        global = true
    )]
    pub verbose: bool,

    #[arg(
        short = 'q',
        long = "quiet",
        help = "Enable quiet mode",
        conflicts_with = "verbose",
        global = true
    )]
    pub quiet: bool,

    #[arg(
        long = "no-color",
        help = "Disable colored output (also honors the NO_COLOR environment variable)",
        global = true
    )]
    pub no_color: bool,
}

#[derive(Args, Debug, Default, PartialEq)]
pub struct AgentArgs {
    #[arg(long = "system", help = "Set the system prompt", value_name = "PROMPT")]
    pub system: Option<String>,

    #[arg(
        short = 's',
        long = "session-name",
        help = "Name of the session (continue it when it exists)",
        value_name = "NAME"
    )]
    pub session_name: Option<String>,

    #[arg(
        long = "no-session",
        help = "Do not read or write a session; run stateless",
        conflicts_with = "session_name"
    )]
    pub no_session: bool,

    #[arg(long = "no-memory", help = "Disable persistent memory for this run")]
    pub no_memory: bool,

    #[arg(
        short = 'r',
        long = "read",
        help = "Allow read-only access to PATH",
        value_name = "PATH"
    )]
    pub read: Vec<String>,

    #[arg(
        short = 'w',
        long = "write",
        help = "Allow read/write access to PATH",
        value_name = "PATH"
    )]
    pub write: Vec<String>,

    #[arg(
        short = 'x',
        long = "execute",
        help = "Allow execution of PATTERN",
        value_name = "PATTERN"
    )]
    pub execute: Vec<String>,

    #[arg(long = "web", help = "Allow all web access (fetch and search)")]
    pub web: bool,

    #[arg(
        long = "web-fetch",
        help = "Allow web fetch for matching URL pattern",
        value_name = "PATTERN"
    )]
    pub web_fetch: Vec<String>,

    #[arg(
        long = "web-search",
        help = "Allow web search with matching query pattern",
        value_name = "PATTERN"
    )]
    pub web_search: Vec<String>,

    #[arg(
        long = "proxy",
        help = "Route web requests through a proxy (e.g. http://127.0.0.1:8080 or socks5h://127.0.0.1:1080)",
        value_name = "URL"
    )]
    pub proxy: Option<String>,

    #[arg(
        short = 'p',
        long = "policy",
        help = "Load policy from FILE",
        value_name = "FILE"
    )]
    pub policy: Option<PathBuf>,

    #[arg(
        short = 'i',
        long = "ask",
        help = "Ask the user for confirmation instead of denying policy checks"
    )]
    pub ask: bool,

    #[arg(
        short = 't',
        long = "tool",
        help = "Connect to tool server (can be given multiple times)",
        value_name = "URL"
    )]
    pub tool: Vec<String>,

    #[arg(
        short = 'y',
        long = "yolo",
        help = "Allow everything without asking (overrides all policy rules, DANGEROUS)",
        conflicts_with = "ask"
    )]
    pub yolo: bool,

    #[arg(
        short = 'X',
        long = "container",
        help = "Run external commands in this container image (default debian:stable-slim); -X is the short form",
        num_args = 0..=1,
        value_name = "IMAGE",
        default_missing_value = "debian:stable-slim"
    )]
    pub container: Option<String>,

    #[arg(
        long = "container-runtime",
        help = "Container runtime to use: auto, docker, or podman",
        value_name = "RUNTIME"
    )]
    pub container_runtime: Option<String>,

    #[arg(
        long = "no-container",
        help = "Run external commands on the host, ignoring any configured container"
    )]
    pub no_container: bool,

    #[arg(
        long = "max-tokens",
        help = "Set the maximum number of tokens",
        value_name = "N"
    )]
    pub max_tokens: Option<usize>,

    #[arg(
        long = "thinking",
        help = "Enable extended thinking (budget in tokens, default: 16000)",
        num_args = 0..=1,
        value_name = "TOKENS",
        default_missing_value = "16000"
    )]
    pub thinking: Option<usize>,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    /// Run the agent once with an inline prompt (or read it from stdin)
    Run {
        #[arg(
            value_name = "PROMPT",
            help = "Prompt text (if absent, read from stdin)"
        )]
        prompt: Vec<String>,
        #[command(flatten)]
        agent: AgentArgs,
    },

    /// Manage saved sessions
    #[command(subcommand)]
    Session(SessionCommand),

    /// Set up or reconfigure the AI interactively
    Setup {
        #[arg(value_name = "FILE", help = "Config file to create or update")]
        file: Option<String>,
    },

    /// Run memory maintenance: distill transcripts, prune, judge stale entries
    Dream {
        #[arg(
            long = "jobs",
            value_name = "N",
            help = "Number of parallel dream requests (default 4)"
        )]
        jobs: Option<usize>,
    },

    /// Inspect persistent memory
    #[command(subcommand)]
    Memory(MemoryCommand),

    /// Inspect and manage skills
    #[command(subcommand)]
    Skills(SkillsCommand),

    /// Generate a shell completion script for SHELL and exit
    Completions {
        #[arg(value_name = "SHELL", help = "bash, zsh, fish, …")]
        shell: Shell,
    },

    #[command(hide = true)]
    ProbeWeb {
        #[arg(value_name = "QUERY")]
        query: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// List all saved sessions
    List,
    /// Delete a session by NAME
    Delete {
        #[arg(
            value_name = "NAME",
            help = "An undated NAME matches the latest session across dates"
        )]
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum MemoryCommand {
    /// List all persistent memory entries
    List,
    /// Search memory and past conversation transcripts
    Search {
        #[arg(value_name = "QUERY")]
        query: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SkillsCommand {
    /// List discovered skills
    List,
    /// Delete a skill by NAME (its folder, including bundled files)
    Delete {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

impl Deref for Cli {
    type Target = AgentArgs;

    fn deref(&self) -> &Self::Target {
        &self.agent
    }
}

impl Cli {
    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    pub fn is_vanilla(&self) -> bool {
        self.command.is_none()
            && self.agent == AgentArgs::default()
            && self.config.is_none()
            && self.provider.is_none()
            && self.model.is_none()
            && self.max_turns == 100
            && !self.verbose
            && !self.quiet
            && !self.no_color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("ai").chain(args.iter().copied()))
    }

    fn run_prompt(c: &Cli) -> Option<Vec<String>> {
        match &c.command {
            Some(Command::Run { prompt, .. }) => Some(prompt.clone()),
            _ => None,
        }
    }

    #[test]
    fn test_space_and_equals_values() {
        assert_eq!(parse(&["-r", "."]).read, vec![".".to_string()]);
        assert_eq!(parse(&["-r=."]).read, vec![".".to_string()]);
        assert_eq!(
            parse(&["--model", "gpt-4o"]).model.as_deref(),
            Some("gpt-4o")
        );
        assert_eq!(parse(&["--model=gpt-4o"]).model.as_deref(), Some("gpt-4o"));
        assert_eq!(
            parse(&["--proxy", "socks5h://127.0.0.1:1080"])
                .proxy
                .as_deref(),
            Some("socks5h://127.0.0.1:1080")
        );
    }

    #[test]
    fn test_session_name() {
        assert_eq!(parse(&["-s", "foo"]).session_name.as_deref(), Some("foo"));
        assert_eq!(
            parse(&["--session-name=foo"]).session_name.as_deref(),
            Some("foo")
        );
        assert!(Cli::try_parse_from(["ai", "-s"]).is_err());
        assert!(Cli::try_parse_from(["ai", "--no-session", "-s", "foo"]).is_err());
    }

    #[test]
    fn test_memory_flags_removed() {
        assert!(Cli::try_parse_from(["ai", "-m", "x"]).is_err());
        assert!(Cli::try_parse_from(["ai", "--memory", "x"]).is_err());
        assert!(Cli::try_parse_from(["ai", "--memory-list"]).is_err());
        assert!(parse(&["--no-memory"]).no_memory);
    }

    #[test]
    fn test_container_default_image() {
        assert_eq!(
            parse(&["-X"]).container.as_deref(),
            Some("debian:stable-slim")
        );
        assert_eq!(
            parse(&["-X", "alpine"]).container.as_deref(),
            Some("alpine")
        );
        assert_eq!(parse(&["-X=alpine"]).container.as_deref(), Some("alpine"));
        assert_eq!(parse(&[]).container, None);
    }

    #[test]
    fn test_thinking_default() {
        assert_eq!(parse(&["--thinking"]).thinking, Some(16000));
        assert_eq!(parse(&["--thinking", "8000"]).thinking, Some(8000));
        assert_eq!(parse(&["--thinking=8000"]).thinking, Some(8000));
    }

    #[test]
    fn test_run_is_explicit() {
        assert_eq!(
            run_prompt(&parse(&["run", "hello"])),
            Some(vec!["hello".into()])
        );
        assert_eq!(
            run_prompt(&parse(&["run", "hello", "world"])),
            Some(vec!["hello".into(), "world".into()])
        );
        assert_eq!(run_prompt(&parse(&["run"])), Some(Vec::new()));
        assert!(Cli::try_parse_from(["ai", "hello"]).is_err());
    }

    #[test]
    fn test_subcommands() {
        assert!(matches!(
            parse(&["session", "list"]).command,
            Some(Command::Session(SessionCommand::List))
        ));
        assert!(matches!(
            parse(&["session", "delete", "foo"]).command,
            Some(Command::Session(SessionCommand::Delete { .. }))
        ));
        assert!(matches!(
            parse(&["memory", "list"]).command,
            Some(Command::Memory(MemoryCommand::List))
        ));
        assert!(matches!(
            parse(&["memory", "search", "berlin"]).command,
            Some(Command::Memory(MemoryCommand::Search { .. }))
        ));
        assert!(matches!(
            parse(&["skills", "list"]).command,
            Some(Command::Skills(SkillsCommand::List))
        ));
        assert!(matches!(
            parse(&["skills", "delete", "foo"]).command,
            Some(Command::Skills(SkillsCommand::Delete { .. }))
        ));
        assert!(matches!(
            parse(&["dream", "--jobs", "8"]).command,
            Some(Command::Dream { jobs: Some(8) })
        ));
        assert!(matches!(
            parse(&["setup"]).command,
            Some(Command::Setup { file: None })
        ));
        assert!(matches!(
            parse(&["setup", "/tmp/ai.yaml"]).command,
            Some(Command::Setup { file: Some(_) })
        ));
        assert!(matches!(
            parse(&["completions", "bash"]).command,
            Some(Command::Completions { .. })
        ));
        assert!(matches!(
            parse(&["probe-web", "test"]).command,
            Some(Command::ProbeWeb { .. })
        ));
    }

    #[test]
    fn test_is_vanilla() {
        assert!(parse(&[]).is_vanilla());
        assert!(!parse(&["run"]).is_vanilla());
        assert!(!parse(&["session", "list"]).is_vanilla());
        assert!(!parse(&["-r", "."]).is_vanilla());
        assert!(!parse(&["-v"]).is_vanilla());
        assert!(!parse(&["--no-memory"]).is_vanilla());
    }

    #[test]
    fn test_conflicting_flags() {
        assert!(Cli::try_parse_from(["ai", "-v", "-q"]).is_err());
        assert!(Cli::try_parse_from(["ai", "-y", "-i"]).is_err());
    }

    #[test]
    fn test_completions_and_no_color() {
        assert!(parse(&["--no-color"]).no_color);
        assert!(!parse(&[]).no_color);
    }
}
