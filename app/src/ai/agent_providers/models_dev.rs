//! models.dev data source access.
//!
//! When the user opens the Providers settings page, `https://models.dev/api.json` is pulled asynchronously in the background,
//! Cache to `${cache_dir}/models-dev.json`. Next time you start reading the cache directly,
//! If the cache is hit and the TTL has not passed (default 24h), no request will be sent; it will be pulled again when it expires/is missing.
//!
//! Data structure alignment opencode's `provider/models.ts`: the top level is
//! `{ <provider_id>: Provider }`, Provider contains `models: { <model_id>: Model }`.
//! We only care about a few fields required for UI "quick selection":
//! - provider: id / name / api / env (implies which env var is required)
//! - model:    id / name / limit.context / limit.output / reasoning / tool_call
//!
//! Fields not listed will be tolerated by `serde(default)` + `#[allow(dead_code)]`.
//!
//! Design trade-offs: **Synchronous cache reading, asynchronous network pull**. The reading side is used by the UI and must be fast;
//! Pull the side background spawn, if it fails, it will not play the error and only log. If the cache cannot be read, it will give empty data and UI display.
//! "Models.dev has not been pulled yet, please check the network."

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::sync::RwLock;
use std::time::{Duration, SystemTime};

use http_client::Client;
use serde::{Deserialize, Serialize};

const MODELS_DEV_URL: &str = "https://models.dev/api.json";
const CACHE_FILENAME: &str = "models-dev.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// `models.dev` Top-level data — provider_id → Provider.
pub type Catalog = BTreeMap<String, Provider>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Provider {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Upstream API base URL, such as `https://api.deepseek.com/v1`.
    #[serde(default)]
    pub api: Option<String>,
    /// The provider usually requires an environment variable name, such as `["DEEPSEEK_API_KEY"]`.
    #[serde(default)]
    pub env: Vec<String>,
    /// Available models, key is model id.
    #[serde(default)]
    pub models: BTreeMap<String, Model>,
    /// Document URL (some providers have it).
    #[serde(default)]
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Model {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default = "default_true")]
    pub tool_call: bool,
    /// Whether to support file attachments (attachment field, complementary to modalities:
    /// modalities describes native multimodals; attachment covers PDF/Universal File Attachment Protocol).
    #[serde(default)]
    pub attachment: bool,
    /// Input/output modalities, typical values: `text` / `image` / `audio` / `video` / `pdf`.
    #[serde(default)]
    pub modalities: ModelModalities,
    /// Context window upper limit.
    #[serde(default)]
    pub limit: ModelLimit,
    /// "alpha" / "beta" / "deprecated" tags.
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

impl ModelModalities {
    pub fn supports_input(&self, modality: &str) -> bool {
        self.input.iter().any(|m| m.eq_ignore_ascii_case(modality))
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelLimit {
    #[serde(default)]
    pub context: u32,
    #[serde(default)]
    pub output: u32,
}

// ── In-process singleton cache ──────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct State {
    /// loaded catalog. `None` means it was never loaded successfully.
    catalog: Option<Catalog>,
    /// The last modification time of the cache (used to determine whether it has expired).
    loaded_at: Option<SystemTime>,
}

fn state() -> &'static RwLock<State> {
    static S: OnceLock<RwLock<State>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(State::default()))
}

fn cache_path() -> PathBuf {
    let mut p = warp_core::paths::cache_dir();
    p.push(CACHE_FILENAME);
    p
}

/// Read a loaded catalog copy (no lock waits - direct cloning).
/// If there is no data, return `None`, and the UI should display the "Pull" / Retry button.
pub fn cached() -> Option<Catalog> {
    state().read().ok().and_then(|s| s.catalog.clone())
}

/// Capability snapshot of a model pulled from models.dev for the BYOP UI/chat_stream decision attachment type.
#[derive(Debug, Clone, Default)]
pub struct ModelCaps {
    pub vision: bool,
    pub pdf: bool,
    pub audio: bool,
    pub attachment: bool,
}

impl ModelCaps {
    pub fn from_model(m: &Model) -> Self {
        Self {
            vision: m.modalities.supports_input("image"),
            pdf: m.modalities.supports_input("pdf") || m.attachment,
            audio: m.modalities.supports_input("audio"),
            attachment: m.attachment,
        }
    }
}

/// Search by model_id in the loaded catalog and return the capabilities of the model declared on models.dev.
///
/// Priority is given to using `provider_id` to accurately match the catalog provider key; when missing, it degrades to "full catalog"
/// Scan for first model.id hit". This can accurately match (provider.id filled in by the user and models.dev
/// When consistent), it can also handle user-defined provider ids (such as "openrouter" or "siliconflow"
/// This aggregation provider forwards upstream models with an id different from the models.dev upstream provider).
pub fn lookup_caps(provider_id: &str, model_id: &str) -> Option<ModelCaps> {
    let s = state().read().ok()?;
    let catalog = s.catalog.as_ref()?;
    if let Some(p) = catalog.get(provider_id) {
        if let Some(m) = p.models.get(model_id) {
            return Some(ModelCaps::from_model(m));
        }
    }
    for p in catalog.values() {
        if let Some(m) = p.models.get(model_id) {
            return Some(ModelCaps::from_model(m));
        }
    }
    None
}

/// Read the disk cache into memory (synchronous, non-blocking; only called when the process starts or when the UI first needs it).
/// If the disk cache does not exist or parsing fails, false is returned and the caller should trigger a network pull.
pub fn load_from_disk() -> bool {
    let path = cache_path();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok());
    match serde_json::from_slice::<Catalog>(&bytes) {
        Ok(catalog) => {
            if let Ok(mut s) = state().write() {
                s.catalog = Some(catalog);
                s.loaded_at = mtime;
            }
            true
        }
        Err(e) => {
            log::warn!("[models.dev] Failed to parse disk cache ({path:?}): {e}");
            false
        }
    }
}

/// Is the cache expired - does not exist or exceeds TTL.
pub fn is_stale() -> bool {
    let s = match state().read() {
        Ok(s) => s,
        Err(_) => return true,
    };
    match s.loaded_at {
        Some(t) => SystemTime::now()
            .duration_since(t)
            .map(|d| d > CACHE_TTL)
            .unwrap_or(true),
        None => true,
    }
}

/// Asynchronously pull models.dev and write to disk cache and memory cache.
/// Failure is only logged, not propagate upward (the UI caller determines whether it is displayed according to whether `cached()` is `Some`).
pub async fn fetch_and_cache(client: Client) -> Result<(), String> {
    let resp = client
        .get(MODELS_DEV_URL)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response body: {e}"))?;

    let catalog: Catalog =
        serde_json::from_slice(&bytes).map_err(|e| format!("JSON parsing failed: {e}"))?;

    // Write to disk - failure is not fatal, just log.
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, &bytes) {
        log::warn!("[models.dev] Failed to write disk cache ({path:?}): {e}");
    }

    if let Ok(mut s) = state().write() {
        s.catalog = Some(catalog);
        s.loaded_at = Some(SystemTime::now());
    }
    Ok(())
}

// ── chip row folding/expanding status (process level, to avoid widget rebuild loss) ─────────────────

static CHIPS_EXPANDED: AtomicBool = AtomicBool::new(false);

pub fn chips_expanded() -> bool {
    CHIPS_EXPANDED.load(Ordering::Relaxed)
}

pub fn toggle_chips_expanded() {
    CHIPS_EXPANDED.fetch_xor(true, Ordering::Relaxed);
}

// ── Quickly add search filters for chip lines ────────────────────────────────────────────

fn search_state() -> &'static RwLock<String> {
    static S: OnceLock<RwLock<String>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(String::new()))
}

pub fn search_query() -> String {
    search_state()
        .read()
        .ok()
        .map(|s| s.clone())
        .unwrap_or_default()
}

pub fn set_search_query(q: String) {
    if let Ok(mut s) = search_state().write() {
        *s = q;
    }
}

/// Filter catalog according to current search query, case-insensitive substring matching provider.name and provider.id.
/// An empty query returns the entire order of entries. Returns an owned Vec for UI side take/iter.
pub fn filter_catalog(catalog: &Catalog, query: &str) -> Vec<(String, Provider)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return catalog
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
    }
    catalog
        .iter()
        .filter(|(id, p)| id.to_lowercase().contains(&q) || p.name.to_lowercase().contains(&q))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Convert the Model in models.dev to the AgentProviderModel used by local settings.
///
/// By default, the image/pdf/audio inferred by the catalog is written into the field (when the user sync / quick-add for the first time
/// You can directly see that the model capabilities are synchronized into toml, and you don’t need to expand detail to see it).
/// In subsequent sync, the caller only fills in the new value into the None slot, and Some(_) is deemed to be explicitly overridden by the user and skipped.
pub fn into_agent_provider_model(model: &Model) -> crate::settings::AgentProviderModel {
    let caps = ModelCaps::from_model(model);
    crate::settings::AgentProviderModel {
        name: if model.name.is_empty() {
            model.id.clone()
        } else {
            model.name.clone()
        },
        id: model.id.clone(),
        context_window: model.limit.context,
        max_output_tokens: model.limit.output,
        reasoning: model.reasoning,
        tool_call: model.tool_call,
        image: Some(caps.vision),
        pdf: Some(caps.pdf),
        audio: Some(caps.audio),
    }
}
