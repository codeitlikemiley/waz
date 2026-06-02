//! Common type definitions for cloud synchronization
//!
// author: logic
// date: 2026-05-24

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Synchronization platform
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPlatform {
    GitHub,
    Gitee,
}

impl SyncPlatform {
    /// Gets the platform API base URL
    pub fn base_url(&self) -> &str {
        match self {
            Self::GitHub => "https://api.github.com",
            Self::Gitee => "https://gitee.com/api/v5",
        }
    }

    /// Gets the platform display label
    pub fn label(&self) -> &str {
        match self {
            Self::GitHub => "GitHub",
            Self::Gitee => "Gitee",
        }
    }

    /// Gets the platform persistence identifier
    pub fn to_db_str(&self) -> &str {
        match self {
            Self::GitHub => "github",
            Self::Gitee => "gitee",
        }
    }
}

/// Synchronization result
#[derive(Debug, Clone)]
pub enum SyncResult {
    Success { version: i64, platform: SyncPlatform },
    Conflict { local_version: i64, remote_version: i64 },
    AlreadyUpToDate { version: i64 },
}

/// Gist list entry (returned by API)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GistEntry {
    pub id: String,
    pub description: Option<String>,
}

/// Gist detail (returned by API)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GistDetail {
    pub id: String,
    pub files: serde_json::Map<String, serde_json::Value>,
}

/// Complete sync data inside the Gist (top-level structure)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncData {
    pub version: i64,
    pub synced_at: String,
    /// Data of each section, where key is the section name (e.g. "ssh"), and value is the JSON representation of the section
    #[serde(flatten)]
    pub sections: serde_json::Map<String, serde_json::Value>,
}

/// Sync engine error
#[derive(Debug, Error)]
pub enum SyncEngineError {
    #[error("Crypto error: {0}")]
    Crypto(String),
    #[error("Gist error: {0}")]
    Gist(String),
    #[error("Provider error: {0}")]
    Provider(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Version store error: {0}")]
    VersionStore(String),
}

/// Sync metadata (version store trait, implemented by the caller)
pub trait SyncVersionStore: Send + Sync {
    /// Gets the current sync version number
    fn get_sync_version(&self) -> Result<i64, SyncEngineError>;
    /// Sets the sync version number
    fn set_sync_version(&self, version: i64) -> Result<(), SyncEngineError>;
    /// Updates sync metadata (timestamp, platform)
    fn update_sync_meta(&self, time: &str, platform: &str) -> Result<(), SyncEngineError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_platform_base_url() {
        assert_eq!(SyncPlatform::GitHub.base_url(), "https://api.github.com");
        assert_eq!(SyncPlatform::Gitee.base_url(), "https://gitee.com/api/v5");
    }

    #[test]
    fn test_sync_platform_label() {
        assert_eq!(SyncPlatform::GitHub.label(), "GitHub");
        assert_eq!(SyncPlatform::Gitee.label(), "Gitee");
    }

    #[test]
    fn test_sync_platform_to_db_str() {
        assert_eq!(SyncPlatform::GitHub.to_db_str(), "github");
        assert_eq!(SyncPlatform::Gitee.to_db_str(), "gitee");
    }

    #[test]
    fn test_sync_platform_equality() {
        assert_eq!(SyncPlatform::GitHub, SyncPlatform::GitHub);
        assert_ne!(SyncPlatform::GitHub, SyncPlatform::Gitee);
    }

    #[test]
    fn test_sync_data_serialization() {
        let mut sections = serde_json::Map::new();
        sections.insert("ssh".to_string(), serde_json::json!({"nodes": []}));
        let data = SyncData {
            version: 42,
            synced_at: "2026-01-01T00:00:00Z".to_string(),
            sections,
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: SyncData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, 42);
        assert_eq!(parsed.synced_at, "2026-01-01T00:00:00Z");
        assert!(parsed.sections.contains_key("ssh"));
    }

    #[test]
    fn test_sync_data_empty_sections() {
        let data = SyncData {
            version: 0,
            synced_at: String::new(),
            sections: serde_json::Map::new(),
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: SyncData = serde_json::from_str(&json).unwrap();
        assert!(parsed.sections.is_empty());
    }

    #[test]
    fn test_gist_entry_deserialization() {
        let json = r#"{"id":"abc123","description":"ZAP_CONFIG"}"#;
        let entry: GistEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.id, "abc123");
        assert_eq!(entry.description, Some("ZAP_CONFIG".to_string()));
    }

    #[test]
    fn test_gist_entry_null_description() {
        let json = r#"{"id":"abc123","description":null}"#;
        let entry: GistEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.id, "abc123");
        assert_eq!(entry.description, None);
    }

    #[test]
    fn test_gist_detail_deserialization() {
        let json = r#"{"id":"gist1","files":{}}"#;
        let detail: GistDetail = serde_json::from_str(json).unwrap();
        assert_eq!(detail.id, "gist1");
        assert!(detail.files.is_empty());
    }

    #[test]
    fn test_sync_engine_error_display() {
        let err = SyncEngineError::Crypto("bad key".to_string());
        assert_eq!(format!("{err}"), "Crypto error: bad key");

        let err = SyncEngineError::Gist("not found".to_string());
        assert_eq!(format!("{err}"), "Gist error: not found");

        let err = SyncEngineError::Provider("db fail".to_string());
        assert_eq!(format!("{err}"), "Provider error: db fail");

        let err = SyncEngineError::Serialization("parse err".to_string());
        assert_eq!(format!("{err}"), "Serialization error: parse err");

        let err = SyncEngineError::VersionStore("io err".to_string());
        assert_eq!(format!("{err}"), "Version store error: io err");
    }
}
