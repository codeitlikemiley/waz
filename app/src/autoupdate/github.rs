// openWarp(Channel::Oss) autoupdate uses the GitHub Releases API instead of Waz official
// channel_versions / GCS. This module is only responsible for "fetching the latest release metadata" + "picking assets by filename";
// the actual downloading/saving to disk + opening the directory is done by windows.rs / mac.rs.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context as _, Result};
use lazy_static::lazy_static;
use serde::Deserialize;

const REPO_OWNER: &str = "codeitlikemiley";
const REPO_NAME: &str = "waz";

// GitHub strictly requires a User-Agent; also explicitly declare the API version to avoid future default shifts.
const USER_AGENT: &str = "Waz-Autoupdate";
const ACCEPT: &str = "application/vnd.github+json";
const API_VERSION: &str = "2022-11-28";

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    pub html_url: String,
    pub assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    /// Asset digest returned by GitHub Releases API (2024.12+) in asset metadata,
    /// in the format of `"sha256:<hex>"`. It is None if old releases do not have this field.
    #[serde(default)]
    pub digest: Option<String>,
}

impl GithubAsset {
    /// Parses the `digest` field, returning lowercase hexadecimal SHA-256 (64 characters) or None.
    /// Currently, the algorithm returned by GitHub is only sha256; other algorithms are directly treated
    /// as None to let the upper layers skip validation, rather than implementing a "green pass" based on unknown algorithms.
    pub fn sha256_hex(&self) -> Option<String> {
        let raw = self.digest.as_ref()?;
        let hex = raw.strip_prefix("sha256:")?;
        if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            Some(hex.to_ascii_lowercase())
        } else {
            None
        }
    }
}

impl GithubRelease {
    pub fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }

    pub fn find_asset(&self, expected_name: &str) -> Option<&GithubAsset> {
        self.assets.iter().find(|a| a.name == expected_name)
    }
}

lazy_static! {
    /// The latest fetched release. Written by fetch_version and read by download_update.
    /// This prevents requesting the GitHub API again during the download stage, and avoids race conditions (releases updating between requests).
    static ref LATEST_RELEASE: Mutex<Option<GithubRelease>> = Mutex::new(None);
}

pub fn cached_release() -> Option<GithubRelease> {
    LATEST_RELEASE.lock().ok().and_then(|g| g.clone())
}

fn store_cached(release: GithubRelease) {
    if let Ok(mut guard) = LATEST_RELEASE.lock() {
        *guard = Some(release);
    }
}

pub async fn fetch_latest_release(client: &http_client::Client) -> Result<GithubRelease> {
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");
    log::info!("Fetching latest release from {url}");
    let release: GithubRelease = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", ACCEPT)
        .header("X-GitHub-Api-Version", API_VERSION)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .context("Failed to call GitHub Releases API")?
        .error_for_status()
        .context("GitHub Releases API returned non-2xx status code")?
        .json()
        .await
        .context("Failed to parse GitHub Releases JSON")?;
    log::info!(
        "GitHub latest release: tag={} assets={}",
        release.tag_name,
        release.assets.len()
    );
    store_cached(release.clone());
    Ok(release)
}
