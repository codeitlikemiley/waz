//! Minimal subset of OpenAI compatible clients: currently only used to grab the `/models` list.
//!
//! When the multi-agent call is made in the second phase, this will be expanded to a complete
//! Chat Completions + tool call stream.

use serde::Deserialize;

use http_client::Client;

/// A single model entry returned by the `/models` endpoint.
///
/// We only care about the `id` (which is used as the model name for the Agent). Other fields (`object`/`created`/`owned_by`)
/// Different providers vary greatly, so all are ignored here.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct OpenAiCompatibleModel {
    pub id: String,
    /// The owner inferred from `owned_by`, mainly used for UI display, may be empty.
    #[serde(default)]
    pub owned_by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<OpenAiCompatibleModel>,
}

/// Possible errors during fetch.
#[derive(Debug, thiserror::Error)]
pub enum OpenAiCompatibleError {
    #[error("Invalid base URL: {0}")]
    InvalidBaseUrl(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("HTTP status {status}: {body}")]
    Status { status: u16, body: String },

    #[error("Response decode failed: {0}")]
    Decode(String),

    #[error("Network/stream request failed: {0}")]
    Stream(String),

    #[error("Call failed: {0}")]
    Other(String),
}

/// Normalize the base_url entered by the user into an absolute URL form,
/// Tolerates trailing `/`, missing `/v1`, `/openai/v1`, etc.
pub(crate) fn normalize_base_url(input: &str) -> Result<String, OpenAiCompatibleError> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(OpenAiCompatibleError::InvalidBaseUrl(
            "Base URL cannot be empty".to_string(),
        ));
    }
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(OpenAiCompatibleError::InvalidBaseUrl(format!(
            "Base URL must start with http:// or https://: {trimmed}"
        )));
    }
    Ok(trimmed.to_string())
}

/// Call `${base_url}/models` to return a list of model IDs (duplicated + sorted alphabetically).
///
/// Authentication: If `api_key` is not empty, it will be brought in the form of `Authorization: Bearer...`.
/// Some local services (such as Ollama) allow no authentication, so the header is not sent when the key is empty.
pub async fn fetch_openai_compatible_models(
    client: Client,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<OpenAiCompatibleModel>, OpenAiCompatibleError> {
    let base = normalize_base_url(base_url)?;
    let url = format!("{base}/models");

    let mut req = client.get(&url);
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        req = req.bearer_auth(key);
    }

    let response = req.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(OpenAiCompatibleError::Status {
            status: status.as_u16(),
            body,
        });
    }

    let parsed: ModelsResponse = response
        .json()
        .await
        .map_err(|e| OpenAiCompatibleError::Decode(e.to_string()))?;

    let mut models = parsed.data;
    models.sort_by(|a, b| a.id.cmp(&b.id));
    models.dedup_by(|a, b| a.id == b.id);
    Ok(models)
}
