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

    #[arg(long = "system", help = "Set the system prompt")]
    pub system: Option<String>,

    #[arg(
        short = 's',
        long = "session",
        help = "Start an interactive session (or continue one given NAME), implies -i",
        num_args = 0..=1,
        value_name = "NAME",
        default_missing_value = ""
    )]
    pub session: Option<String>,

    #[arg(
        short = 'm',
        long = "memory",
        help = "Enable persistent memory with optional FILE",
        num_args = 0..=1,
        value_name = "FILE",
        default_missing_value = ""
    )]
    pub memory: Option<String>,

    #[arg(
        short = 'c',
        long = "config",
        help = "Load configuration from FILE",
        value_name = "FILE"
    )]
    pub config: Option<PathBuf>,

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
        short = 'p',
        long = "policy",
        help = "Load policy from FILE",
        value_name = "FILE"
    )]
    pub policy: Option<PathBuf>,

    #[arg(
        short = 'i',
        long = "interactive",
        help = "Enable interactive mode (ask for confirmation instead of denying)"
    )]
    pub interactive: bool,

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
        help = "Enable yolo mode (set default policy to allow, DANGEROUS)"
    )]
    pub yolo: bool,

    #[arg(
        long = "max-tokens",
        help = "Set the maximum number of tokens",
        value_name = "N"
    )]
    pub max_tokens: Option<usize>,

    #[arg(
        long = "max-turns",
        help = "Set the maximum number of agent turns (tool call rounds)",
        value_name = "N",
        default_value = "100"
    )]
    pub max_turns: usize,

    #[arg(
        long = "thinking",
        help = "Enable extended thinking (budget in tokens, default: 16000)",
        num_args = 0..=1,
        value_name = "TOKENS",
        default_missing_value = "16000"
    )]
    pub thinking: Option<usize>,

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
}
