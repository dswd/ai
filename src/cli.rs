use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "ai",
    version = env!("CARGO_PKG_VERSION"),
    about = "CLI interface for interacting with AI models",
    long_about = "AI Tool processes the prompt given as parameter or read from stdin.\n\
                  It generates a response from the AI model and prints it to stdout.\n\
                  Thinking output and tool calls are printed to stderr.",
    after_help = "Environment Variables:\n  \
                  OPENAI_API_KEY, ANTHROPIC_API_KEY, OLLAMA_API_KEY,\n  \
                  GROQ_API_KEY, GEMINI_API_KEY, OPENAI_BASE_URL, etc."
)]
pub struct Cli {
    #[arg(help = "Prompt text (if absent, read from stdin)")]
    pub prompt: Vec<String>,

    #[arg(
        long = "system",
        help = "Set the system prompt",
        value_name = "PROMPT",
        require_equals = true
    )]
    pub system: Option<String>,

    #[arg(
        short = 's',
        long = "session",
        help = "Start an interactive session (or continue one given NAME), implies --ask",
        num_args = 0..=1,
        value_name = "NAME",
        default_missing_value = "",
        require_equals = true,
    )]
    pub session: Option<String>,

    #[arg(
        short = 'm',
        long = "memory",
        help = "Enable persistent memory with optional FILE",
        num_args = 0..=1,
        value_name = "FILE",
        default_missing_value = "",
        require_equals = true,
    )]
    pub memory: Option<String>,

    #[arg(
        short = 'c',
        long = "config",
        help = "Load configuration from FILE",
        value_name = "FILE",
        require_equals = true
    )]
    pub config: Option<PathBuf>,

    #[arg(
        long = "model",
        help = "Override the model",
        value_name = "MODEL",
        require_equals = true
    )]
    pub model: Option<String>,

    #[arg(
        long = "provider",
        help = "Override the provider (openai, anthropic)",
        value_name = "PROVIDER",
        require_equals = true
    )]
    pub provider: Option<String>,

    #[arg(
        short = 'r',
        long = "read",
        help = "Allow read-only access to PATH",
        value_name = "PATH",
        require_equals = true
    )]
    pub read: Vec<String>,

    #[arg(
        short = 'w',
        long = "write",
        help = "Allow read/write access to PATH",
        value_name = "PATH",
        require_equals = true
    )]
    pub write: Vec<String>,

    #[arg(
        short = 'x',
        long = "execute",
        help = "Allow execution of PATTERN",
        value_name = "PATTERN",
        require_equals = true
    )]
    pub execute: Vec<String>,

    #[arg(long = "web", help = "Allow all web access (fetch and search)")]
    pub web: bool,

    #[arg(
        long = "web-fetch",
        help = "Allow web fetch for matching URL pattern",
        value_name = "PATTERN",
        require_equals = true
    )]
    pub web_fetch: Vec<String>,

    #[arg(
        long = "web-search",
        help = "Allow web search with matching query pattern",
        value_name = "PATTERN",
        require_equals = true
    )]
    pub web_search: Vec<String>,

    #[arg(
        short = 'p',
        long = "policy",
        help = "Load policy from FILE",
        value_name = "FILE",
        require_equals = true
    )]
    pub policy: Option<PathBuf>,

    #[arg(
        long = "skill",
        help = "Load a skill from PATH (SKILL.md file or folder containing SKILL.md); can be given multiple times",
        value_name = "PATH",
        require_equals = true
    )]
    pub skill: Vec<String>,

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
        value_name = "URL",
        require_equals = true
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
        long = "max-tokens",
        help = "Set the maximum number of tokens",
        value_name = "N",
        require_equals = true
    )]
    pub max_tokens: Option<usize>,

    #[arg(
        long = "max-turns",
        help = "Set the maximum number of agent turns (tool call rounds)",
        value_name = "N",
        default_value = "100",
        require_equals = true
    )]
    pub max_turns: usize,

    #[arg(
        long = "thinking",
        help = "Enable extended thinking (budget in tokens, default: 16000)",
        num_args = 0..=1,
        value_name = "TOKENS",
        default_missing_value = "16000",
        require_equals = true,
    )]
    pub thinking: Option<usize>,

    #[arg(
        short = 'l',
        long = "list",
        help = "List all saved sessions",
        conflicts_with_all = ["delete", "init", "session"]
    )]
    pub list: bool,

    #[arg(
        long = "init",
        help = "Initialize config interactively",
        value_name = "FILE",
        num_args = 0..=1,
        default_missing_value = "",
        require_equals = true,
        conflicts_with_all = ["config", "list", "delete", "session"],
    )]
    pub init: Option<String>,

    #[arg(
        long = "delete",
        help = "Delete a session by NAME",
        value_name = "NAME",
        require_equals = true,
        conflicts_with_all = ["list", "init", "session"]
    )]
    pub delete: Option<String>,

    #[arg(
        short = 'v',
        long = "verbose",
        help = "Enable verbose mode",
        conflicts_with = "quiet"
    )]
    pub verbose: bool,

    #[arg(
        short = 'q',
        long = "quiet",
        help = "Enable quiet mode",
        conflicts_with = "verbose"
    )]
    pub quiet: bool,
}

impl Cli {
    pub fn prompt_text(&self) -> Option<String> {
        if self.prompt.is_empty() {
            None
        } else {
            Some(self.prompt.join(" "))
        }
    }

    pub fn is_interactive(&self) -> bool {
        self.session.is_some()
    }

    pub fn is_vanilla(&self) -> bool {
        self.prompt.is_empty()
            && self.system.is_none()
            && self.session.is_none()
            && self.memory.is_none()
            && self.config.is_none()
            && self.model.is_none()
            && self.provider.is_none()
            && self.read.is_empty()
            && self.write.is_empty()
            && self.execute.is_empty()
            && !self.web
            && self.web_fetch.is_empty()
            && self.web_search.is_empty()
            && self.policy.is_none()
            && self.skill.is_empty()
            && !self.ask
            && self.tool.is_empty()
            && !self.yolo
            && self.max_tokens.is_none()
            && self.max_turns == 100
            && self.thinking.is_none()
            && !self.verbose
            && !self.quiet
            && self.init.is_none()
            && !self.list
            && self.delete.is_none()
    }
}
