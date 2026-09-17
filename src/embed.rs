use std::path::PathBuf;
use std::sync::OnceLock;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// Default local embedding model (multilingual, 384 dims).
pub const DEFAULT_MODEL: &str = "multilingual-e5-small";

/// Maximum cosine distance for a retrieved hit, per model family. E5 models
/// score related text very close (roughly 0.1–0.2) and unrelated text around
/// 0.25–0.3, so they need a tighter cutoff than the contrastive MiniLM family.
const E5_MAX_DISTANCE: f32 = 0.175;
const DEFAULT_MAX_DISTANCE: f32 = 0.45;

/// A resolved embedding model: the fastembed handle, its vector size, and
/// whether it needs the E5 `query:`/`passage:` instruction prefixes.
pub struct ModelSpec {
    pub model: EmbeddingModel,
    pub dims: usize,
    pub e5: bool,
}

/// Resolve a config model id. Unknown ids log a warning and fall back to the
/// default so a typo cannot make memory unusable.
pub fn resolve_model(id: &str) -> ModelSpec {
    let spec = match id {
        "multilingual-e5-small" => (EmbeddingModel::MultilingualE5Small, true),
        "multilingual-e5-base" => (EmbeddingModel::MultilingualE5Base, true),
        "multilingual-e5-large" => (EmbeddingModel::MultilingualE5Large, true),
        "paraphrase-multilingual-minilm-l12-v2" => (EmbeddingModel::ParaphraseMLMiniLML12V2, false),
        "paraphrase-multilingual-mpnet-base-v2" => (EmbeddingModel::ParaphraseMLMpnetBaseV2, false),
        "bge-small-en-v1.5" => (EmbeddingModel::BGESmallENV15, false),
        "all-minilm-l6-v2" => (EmbeddingModel::AllMiniLML6V2, false),
        _ => {
            log::warn!("unknown embedding model '{id}'; using {DEFAULT_MODEL}");
            (EmbeddingModel::MultilingualE5Small, true)
        }
    };
    let dims = TextEmbedding::get_model_info(&spec.0)
        .map(|info| info.dim)
        .unwrap_or(384);
    ModelSpec {
        model: spec.0,
        dims,
        e5: spec.1,
    }
}

/// The vector size for a configured model id, without loading the model.
#[cfg(test)]
pub fn dims_for(id: &str) -> usize {
    resolve_model(id).dims
}

/// Text embedding, kept synchronous so the memory store needs no async plumbing.
pub trait Embedder: Send + Sync {
    fn dims(&self) -> usize;
    fn model_id(&self) -> String;
    /// Cosine distance beyond which a hit is not considered relevant.
    fn max_distance(&self) -> f32;
    fn embed_passages(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>;
    fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>>;
}

/// Local ONNX embedder. The model is downloaded (once) and loaded lazily on the
/// first embedding call, so commands that never embed stay offline and fast.
pub struct FastembedEmbedder {
    id: String,
    spec: ModelSpec,
    max_distance: Option<f32>,
    engine: OnceLock<Result<TextEmbedding, String>>,
}

impl FastembedEmbedder {
    /// `max_distance` overrides the model family's default cutoff when set
    /// (config `memory_max_distance`).
    pub fn new(id: &str, max_distance: Option<f32>) -> Self {
        Self {
            id: id.to_string(),
            spec: resolve_model(id),
            max_distance,
            engine: OnceLock::new(),
        }
    }

    fn engine(&self) -> anyhow::Result<&TextEmbedding> {
        let loaded = self.engine.get_or_init(|| {
            log::debug!("loading embedding model '{}'", self.id);
            let options = InitOptions::new(self.spec.model.clone())
                .with_cache_dir(cache_dir())
                .with_show_download_progress(true);
            TextEmbedding::try_new(options).map_err(|e| e.to_string())
        });
        loaded
            .as_ref()
            .map_err(|e| anyhow::anyhow!("embedding model '{}' is unavailable: {e}", self.id))
    }

    fn embed(&self, texts: &[String], prefix: &str) -> anyhow::Result<Vec<Vec<f32>>> {
        let engine = self.engine()?;
        let inputs: Vec<String> = if prefix.is_empty() {
            texts.to_vec()
        } else {
            texts.iter().map(|t| format!("{prefix}{t}")).collect()
        };
        engine
            .embed(inputs, None)
            .map_err(|e| anyhow::anyhow!("embedding failed: {e}"))
    }
}

impl Embedder for FastembedEmbedder {
    fn dims(&self) -> usize {
        self.spec.dims
    }

    fn model_id(&self) -> String {
        self.id.clone()
    }

    fn max_distance(&self) -> f32 {
        self.max_distance.unwrap_or(if self.spec.e5 {
            E5_MAX_DISTANCE
        } else {
            DEFAULT_MAX_DISTANCE
        })
    }

    fn embed_passages(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        self.embed(texts, if self.spec.e5 { "passage: " } else { "" })
    }

    fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let prefix = if self.spec.e5 { "query: " } else { "" };
        Ok(self
            .embed(&[text.to_string()], prefix)?
            .into_iter()
            .next()
            .unwrap_or_default())
    }
}

/// Where fastembed caches downloaded model files (avoids its CWD-relative default).
fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ai")
        .join("fastembed")
}

/// Serialize a float vector as little-endian bytes for sqlite-vec.
pub fn to_blob(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Deterministic bag-of-words embedder for tests; no model download.
#[cfg(test)]
pub struct HashEmbedder {
    dims: usize,
    max_distance: f32,
}

#[cfg(test)]
impl HashEmbedder {
    /// Permissive cutoff: keeps token-overlap matches, drops orthogonal ones.
    pub fn new(dims: usize) -> Self {
        Self {
            dims,
            max_distance: 1.1,
        }
    }

    /// A tighter cutoff, for exercising the distance filter.
    pub fn with_max_distance(dims: usize, max_distance: f32) -> Self {
        Self { dims, max_distance }
    }

    fn vector(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dims];
        for word in text.split(|c: char| !c.is_alphanumeric()) {
            let word = word.to_lowercase();
            if word.is_empty() {
                continue;
            }
            let hash = word.bytes().fold(1469598103934665603u64, |a, b| {
                (a ^ b as u64).wrapping_mul(1099511628211)
            }) as usize;
            v[hash % self.dims] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        } else {
            v[0] = 1.0;
        }
        v
    }
}

#[cfg(test)]
impl Embedder for HashEmbedder {
    fn dims(&self) -> usize {
        self.dims
    }

    fn model_id(&self) -> String {
        format!("hash-{}", self.dims)
    }

    fn max_distance(&self) -> f32 {
        self.max_distance
    }

    fn embed_passages(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.vector(t)).collect())
    }

    fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        Ok(self.vector(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_known_and_unknown() {
        let e5 = resolve_model("multilingual-e5-small");
        assert_eq!(e5.dims, 384);
        assert!(e5.e5);

        let mini = resolve_model("all-minilm-l6-v2");
        assert_eq!(mini.dims, 384);
        assert!(!mini.e5);

        let unknown = resolve_model("no-such-model");
        assert_eq!(unknown.dims, dims_for(DEFAULT_MODEL));
    }

    #[test]
    fn test_hash_embedder_is_deterministic_and_normalized() {
        let embedder = HashEmbedder::new(8);
        let a = embedder.vector("berlin trip");
        let b = embedder.vector("berlin trip");
        assert_eq!(a, b);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_blob_roundtrip() {
        let blob = to_blob(&[1.0, -2.5]);
        assert_eq!(blob.len(), 8);
        assert_eq!(f32::from_le_bytes(blob[0..4].try_into().unwrap()), 1.0);
        assert_eq!(f32::from_le_bytes(blob[4..8].try_into().unwrap()), -2.5);
    }
}
