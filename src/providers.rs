#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    OpenAi,
    Anthropic,
}

#[derive(Debug, Clone, Copy)]
pub struct Provider {
    pub name: &'static str,
    pub flavor: Flavor,
    pub default_base_url: Option<&'static str>,
    pub env_var: &'static str,
    pub models: &'static [&'static str],
    pub context_window: usize,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        name: "openai",
        flavor: Flavor::OpenAi,
        default_base_url: Some("https://api.openai.com/v1"),
        env_var: "OPENAI_API_KEY",
        models: &[
            "gpt-4o",
            "gpt-4.1",
            "gpt-4.1-mini",
            "gpt-4o-mini",
            "gpt-4.1-nano",
        ],
        context_window: 128_000,
    },
    Provider {
        name: "anthropic",
        flavor: Flavor::Anthropic,
        default_base_url: Some("https://api.anthropic.com"),
        env_var: "ANTHROPIC_API_KEY",
        models: &[
            "claude-sonnet-4-20250514",
            "claude-3-5-sonnet-20241022",
            "claude-3-5-haiku-20241022",
        ],
        context_window: 200_000,
    },
    Provider {
        name: "ollama",
        flavor: Flavor::OpenAi,
        default_base_url: Some("http://localhost:11434/v1"),
        env_var: "OLLAMA_API_KEY",
        models: &["llama3.2", "qwen3", "deepseek-r1"],
        context_window: 128_000,
    },
    Provider {
        name: "groq",
        flavor: Flavor::OpenAi,
        default_base_url: Some("https://api.groq.com/openai/v1"),
        env_var: "GROQ_API_KEY",
        models: &["llama-3.3-70b-versatile", "mixtral-8x7b-32768"],
        context_window: 128_000,
    },
    Provider {
        name: "deepseek",
        flavor: Flavor::OpenAi,
        default_base_url: Some("https://api.deepseek.com"),
        env_var: "DEEPSEEK_API_KEY",
        models: &["deepseek-chat", "deepseek-reasoner"],
        context_window: 128_000,
    },
    Provider {
        name: "google",
        flavor: Flavor::OpenAi,
        default_base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai/"),
        env_var: "GEMINI_API_KEY",
        models: &["gemini-2.5-flash", "gemini-2.5-pro"],
        context_window: 1_000_000,
    },
    Provider {
        name: "mistral",
        flavor: Flavor::OpenAi,
        default_base_url: Some("https://api.mistral.ai/v1"),
        env_var: "MISTRAL_API_KEY",
        models: &["mistral-large-latest", "mistral-small-latest"],
        context_window: 128_000,
    },
    Provider {
        name: "openrouter",
        flavor: Flavor::OpenAi,
        default_base_url: Some("https://openrouter.ai/api/v1"),
        env_var: "OPENROUTER_API_KEY",
        models: &[
            "openai/gpt-4o",
            "openai/gpt-4.1",
            "anthropic/claude-sonnet-4",
        ],
        context_window: 128_000,
    },
    Provider {
        name: "xai",
        flavor: Flavor::OpenAi,
        default_base_url: Some("https://api.x.ai/v1"),
        env_var: "XAI_API_KEY",
        models: &["grok-3"],
        context_window: 128_000,
    },
    Provider {
        name: "openai-compatible",
        flavor: Flavor::OpenAi,
        default_base_url: None,
        env_var: "OPENAI_API_KEY",
        models: &[],
        context_window: 128_000,
    },
    Provider {
        name: "anthropic-compatible",
        flavor: Flavor::Anthropic,
        default_base_url: None,
        env_var: "ANTHROPIC_API_KEY",
        models: &[],
        context_window: 200_000,
    },
];

pub fn resolve(name: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.name == name)
}

pub fn all_names() -> impl Iterator<Item = &'static str> {
    PROVIDERS.iter().map(|p| p.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_names_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in PROVIDERS {
            assert!(seen.insert(p.name), "duplicate provider name: {}", p.name);
        }
    }

    #[test]
    fn test_resolve_flavors() {
        assert_eq!(resolve("openai").unwrap().flavor, Flavor::OpenAi);
        assert_eq!(resolve("anthropic").unwrap().flavor, Flavor::Anthropic);
        assert_eq!(resolve("groq").unwrap().flavor, Flavor::OpenAi);
        assert_eq!(resolve("openai-compatible").unwrap().flavor, Flavor::OpenAi);
        assert_eq!(
            resolve("anthropic-compatible").unwrap().flavor,
            Flavor::Anthropic
        );
        assert!(resolve("nope").is_none());
    }

    #[test]
    fn test_named_providers_have_default_base_url() {
        for p in PROVIDERS {
            if p.default_base_url.is_none() {
                assert!(
                    p.name.ends_with("-compatible"),
                    "{} should not lack a default base url",
                    p.name
                );
            }
        }
    }

    #[test]
    fn test_all_names_contains_all() {
        let names: Vec<&str> = all_names().collect();
        assert_eq!(names.len(), PROVIDERS.len());
        assert!(names.contains(&"openai"));
        assert!(names.contains(&"anthropic-compatible"));
    }
}
