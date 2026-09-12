use crate::providers::Flavor;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

pub(crate) const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Native-protocol providers that a plain bearer-token chat client cannot
/// serve (cloud request signing or non-OpenAI deployment paths).
const INCOMPATIBLE_NPM: &[&str] = &[
    "@ai-sdk/amazon-bedrock",
    "@ai-sdk/google-vertex",
    "@ai-sdk/google-vertex/anthropic",
    "@ai-sdk/azure",
    "watsonx-ai-provider",
    "@jerome-benoit/sap-ai-provider-v2",
];

const ANTHROPIC_NPM: &[&str] = &["@ai-sdk/anthropic"];

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Catalog {
    #[serde(flatten)]
    pub providers: BTreeMap<String, CatalogProvider>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogProvider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub npm: Option<String>,
    /// Base URL, when the dataset provides one.
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub doc: Option<String>,
    #[serde(default)]
    pub models: BTreeMap<String, CatalogModel>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogModel {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub limit: Option<CatalogLimit>,
    #[serde(default)]
    pub cost: Option<CatalogCost>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CatalogLimit {
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub output: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CatalogCost {
    #[serde(default)]
    pub input: Option<f64>,
    #[serde(default)]
    pub output: Option<f64>,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

impl Catalog {
    /// Fetch models.dev, falling back to a cached copy (even stale) and finally
    /// to an empty catalog, so setup never hard-fails on the network.
    pub async fn load() -> Catalog {
        let path = cache_path();
        let cached = path.as_ref().and_then(|p| read_cached(p));
        if path.as_ref().is_some_and(|p| is_fresh(p))
            && let Some(catalog) = cached.clone()
        {
            return catalog;
        }

        match fetch().await {
            Ok(catalog) => {
                if let Some(path) = &path {
                    let _ = write_cached(path, &catalog);
                }
                catalog
            }
            Err(e) => {
                log::warn!("models.dev fetch failed: {e}");
                cached.unwrap_or_default()
            }
        }
    }

    /// Providers whose protocol we can speak, sorted by display name.
    pub fn compatible_providers(&self) -> Vec<&CatalogProvider> {
        let mut out: Vec<&CatalogProvider> = self
            .providers
            .values()
            .filter(|p| flavor_for(p).is_some())
            .collect();
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }

    pub fn provider(&self, id: &str) -> Option<&CatalogProvider> {
        self.providers.get(id)
    }

    /// Case-insensitive substring match over id and name, for the "search all"
    /// picker.
    pub fn search(&self, needle: &str) -> Vec<&CatalogProvider> {
        let needle = needle.to_lowercase();
        self.compatible_providers()
            .into_iter()
            .filter(|p| {
                p.id.to_lowercase().contains(&needle) || p.name.to_lowercase().contains(&needle)
            })
            .collect()
    }
}

impl CatalogProvider {
    pub fn env_vars(&self) -> &[String] {
        &self.env
    }

    /// Models sorted by id, each with its metadata.
    pub fn models(&self) -> Vec<&CatalogModel> {
        let mut out: Vec<&CatalogModel> = self.models.values().collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

impl CatalogModel {
    pub fn context(&self) -> Option<u64> {
        self.limit.as_ref().and_then(|l| l.context)
    }

    pub fn price(&self) -> Option<(f64, f64)> {
        let cost = self.cost.as_ref()?;
        Some((cost.input.unwrap_or(0.0), cost.output.unwrap_or(0.0)))
    }

    /// `123k ctx · $0.15/$0.60 per Mtok`, or the parts that exist.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(ctx) = self.context() {
            parts.push(format!("{} ctx", fmt_tokens(ctx)));
        }
        if let Some((input, output)) = self.price() {
            parts.push(format!("${input}/${output} per Mtok"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" — {}", parts.join(" · "))
        }
    }
}

pub(crate) fn flavor_for(provider: &CatalogProvider) -> Option<Flavor> {
    let npm = provider.npm.as_deref().unwrap_or("");
    if INCOMPATIBLE_NPM.contains(&npm) {
        return None;
    }
    if ANTHROPIC_NPM.contains(&npm) {
        return Some(Flavor::Anthropic);
    }
    Some(Flavor::OpenAi)
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.0}m", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub(crate) fn cache_path() -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ai")
            .join("cache")
            .join("models.dev.json"),
    )
}

fn is_fresh(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age < CACHE_TTL)
}

fn read_cached(path: &std::path::Path) -> Option<Catalog> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_cached(path: &std::path::Path, catalog: &Catalog) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec(catalog)?;
    std::fs::write(path, data).with_context(|| format!("writing cache: {}", path.display()))
}

async fn fetch() -> anyhow::Result<Catalog> {
    let response = reqwest::Client::new()
        .get(MODELS_DEV_URL)
        .send()
        .await
        .context("requesting models.dev")?
        .error_for_status()
        .context("models.dev returned an error status")?;
    response
        .json::<Catalog>()
        .await
        .context("parsing models.dev response")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "openai": {
            "id": "openai", "name": "OpenAI", "env": ["OPENAI_API_KEY"],
            "npm": "@ai-sdk/openai",
            "models": {
                "gpt-4o": {
                    "id": "gpt-4o", "name": "GPT-4o",
                    "limit": {"context": 128000, "output": 16384},
                    "cost": {"input": 2.5, "output": 10}
                }
            }
        },
        "anthropic": {
            "id": "anthropic", "name": "Anthropic", "env": ["ANTHROPIC_API_KEY"],
            "npm": "@ai-sdk/anthropic",
            "models": {
                "claude-sonnet-4": {
                    "id": "claude-sonnet-4", "name": "Claude Sonnet 4",
                    "limit": {"context": 200000},
                    "cost": {"input": 3, "output": 15}
                }
            }
        },
        "groq": {
            "id": "groq", "name": "Groq", "env": ["GROQ_API_KEY"],
            "npm": "@ai-sdk/openai-compatible", "api": "https://api.groq.com/openai/v1",
            "models": {}
        },
        "bedrock": {
            "id": "bedrock", "name": "Amazon Bedrock", "env": [],
            "npm": "@ai-sdk/amazon-bedrock", "api": null, "models": {}
        }
    }"#;

    fn catalog() -> Catalog {
        serde_json::from_str(FIXTURE).unwrap()
    }

    #[test]
    fn parses_providers_and_models() {
        let c = catalog();
        assert_eq!(c.providers.len(), 4);
        let openai = c.provider("openai").unwrap();
        assert_eq!(openai.name, "OpenAI");
        assert_eq!(openai.env_vars(), &["OPENAI_API_KEY".to_string()]);
        let model = &openai.models()[0];
        assert_eq!(model.context(), Some(128_000));
        assert_eq!(model.price(), Some((2.5, 10.0)));
    }

    #[test]
    fn maps_flavor_and_hides_incompatible() {
        let c = catalog();
        assert_eq!(
            flavor_for(c.provider("openai").unwrap()),
            Some(Flavor::OpenAi)
        );
        assert_eq!(
            flavor_for(c.provider("anthropic").unwrap()),
            Some(Flavor::Anthropic)
        );
        assert_eq!(flavor_for(c.provider("bedrock").unwrap()), None);
        let ids: Vec<&str> = c
            .compatible_providers()
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(ids, vec!["anthropic", "groq", "openai"]);
    }

    #[test]
    fn search_matches_id_and_name() {
        let c = catalog();
        assert_eq!(c.search("gro").len(), 1);
        assert_eq!(c.search("anthropic").len(), 1);
        assert!(c.search("zzz").is_empty());
    }

    #[test]
    fn summary_formats_context_and_price() {
        let c = catalog();
        let s = c.provider("openai").unwrap().models()[0].summary();
        assert!(s.contains("128k ctx"), "{s}");
        assert!(s.contains("$2.5/$10 per Mtok"), "{s}");
    }
}
