//! xAI Grok OAuth (SuperGrok / X Premium+).
//!
//! Ported from Open Codex (`ocx login xai`): https://opencodex.me/
//! https://github.com/lidge-jun/opencodex/blob/main/src/oauth/xai.ts
//!
//! Same Grok CLI OIDC client against `https://auth.x.ai`. The access token is a
//! Bearer token for `https://api.x.ai/v1`. No `XAI_API_KEY` is required.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration as StdDuration, Instant};

/// Grok CLI / Open Codex public OAuth client (`src/oauth/xai.ts`).
pub const XAI_OAUTH_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_OAUTH_DISCOVERY_URL: &str = "https://auth.x.ai/.well-known/openid-configuration";
const XAI_AUTHORIZE_URL: &str = "https://auth.x.ai/oauth2/authorize";
const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const XAI_DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
const XAI_OAUTH_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_PORT: u16 = 56121;
const CALLBACK_PATH: &str = "/callback";
const REFRESH_SKEW_SECS: i64 = 120;
const TOKEN_TIMEOUT_SECS: u64 = 30;
const BROWSER_WAIT_SECS: u64 = 300;

const GROK_CLI_KEY_PREFIX: &str = "https://auth.x.ai::";
const IMPORT_WARNING: &str =
    "Imported Grok CLI session. Token refresh is now owned by waz; the grok CLI may need to log in again later.";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AuthFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    grok: Option<OAuthCreds>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anthropic: Option<OAuthCreds>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex: Option<OAuthCreds>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCreds {
    access_token: String,
    refresh_token: String,
    expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct LoginOptions {
    pub device: bool,
    pub browser: bool,
    pub import_grok_cli: bool,
    pub force: bool,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self {
            device: false,
            browser: false,
            import_grok_cli: true,
            force: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginResult {
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

impl LoginResult {
    fn from_creds(creds: &OAuthCreds, warning: Option<String>) -> Self {
        Self {
            email: creds.email.clone(),
            account_id: creds.account_id.clone(),
            source: creds.source.clone(),
            warning,
        }
    }

    pub fn identity(&self) -> String {
        self.email
            .clone()
            .or_else(|| self.account_id.clone())
            .unwrap_or_else(|| "account".into())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthStatus {
    pub logged_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

pub fn canonical_provider(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "grok" | "xai" | "x-ai" | "xai-oauth" | "grok-oauth" => "grok".into(),
        "anthropic" | "claude" | "claude-code" | "claude-oauth" => "anthropic".into(),
        "codex" | "chatgpt" | "chatgpt-oauth" | "openai-codex" => "codex".into(),
        other => other.to_string(),
    }
}

pub fn supported_providers() -> &'static [&'static str] {
    &["grok", "anthropic", "codex"]
}

fn slot<'a>(file: &'a AuthFile, provider: &str) -> Option<&'a OAuthCreds> {
    match provider {
        "grok" => file.grok.as_ref(),
        "anthropic" => file.anthropic.as_ref(),
        "codex" => file.codex.as_ref(),
        _ => None,
    }
}

fn set_slot(file: &mut AuthFile, provider: &str, creds: Option<OAuthCreds>) -> Result<(), String> {
    match provider {
        "grok" => file.grok = creds,
        "anthropic" => file.anthropic = creds,
        "codex" => file.codex = creds,
        other => return Err(format!("unknown OAuth provider: {other}")),
    }
    Ok(())
}

pub fn auth_path() -> PathBuf {
    if let Ok(p) = std::env::var("WAZ_AUTH_PATH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::config_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("waz")
        .join("auth.json")
}

fn grok_cli_auth_path() -> PathBuf {
    if let Ok(p) = std::env::var("WAZ_GROK_CLI_AUTH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
        .join("auth.json")
}

fn load_auth() -> AuthFile {
    let path = auth_path();
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_auth(file: &AuthFile) -> Result<(), String> {
    let path = auth_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create auth dir: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(file).map_err(|e| format!("serialize auth: {e}"))?;
    fs::write(&tmp, &body).map_err(|e| format!("write auth: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, &path).map_err(|e| format!("install auth: {e}"))?;
    Ok(())
}

fn save_provider(provider: &str, creds: OAuthCreds) -> Result<OAuthCreds, String> {
    let mut file = load_auth();
    set_slot(&mut file, provider, Some(creds.clone()))?;
    save_auth(&file)?;
    Ok(creds)
}

fn save_grok(creds: OAuthCreds) -> Result<OAuthCreds, String> {
    save_provider("grok", creds)
}

pub fn has_provider(provider: &str) -> bool {
    let name = canonical_provider(provider);
    slot(&load_auth(), &name)
        .map(|c| !c.refresh_token.is_empty() || !c.access_token.is_empty())
        .unwrap_or(false)
}

fn creds_status(creds: Option<&OAuthCreds>) -> AuthStatus {
    match creds {
        Some(c) if !c.refresh_token.is_empty() || !c.access_token.is_empty() => AuthStatus {
            logged_in: true,
            email: c.email.clone(),
            account_id: c.account_id.clone(),
            source: Some(if c.source.is_empty() {
                "oauth".into()
            } else {
                c.source.clone()
            }),
            expires_at: Some(c.expires_at.clone()),
        },
        _ => AuthStatus {
            logged_in: false,
            email: None,
            account_id: None,
            source: None,
            expires_at: None,
        },
    }
}

pub fn status_for(provider: &str) -> AuthStatus {
    let name = canonical_provider(provider);
    let file = load_auth();
    creds_status(slot(&file, &name))
}

#[derive(Debug, Clone, Serialize)]
pub struct AllAuthStatus {
    pub grok: AuthStatus,
    pub anthropic: AuthStatus,
    pub codex: AuthStatus,
}

pub fn status_all() -> AllAuthStatus {
    let file = load_auth();
    AllAuthStatus {
        grok: creds_status(file.grok.as_ref()),
        anthropic: creds_status(file.anthropic.as_ref()),
        codex: creds_status(file.codex.as_ref()),
    }
}

pub fn logout(provider: &str) -> Result<bool, String> {
    let name = canonical_provider(provider);
    if !supported_providers().contains(&name.as_str()) {
        return Err(format!(
            "OAuth logout supports {}. Got: {provider}",
            supported_providers().join(", ")
        ));
    }
    let mut file = load_auth();
    let had = slot(&file, &name).is_some();
    set_slot(&mut file, &name, None)?;
    if had || auth_path().exists() {
        save_auth(&file)?;
    }
    Ok(had)
}

pub fn access_token_for(provider: &str) -> Option<String> {
    access_token_inner(provider, false)
}

pub fn force_refresh_for(provider: &str) -> Option<String> {
    access_token_inner(provider, true)
}

pub fn account_id_for(provider: &str) -> Option<String> {
    let name = canonical_provider(provider);
    slot(&load_auth(), &name)
        .and_then(|c| c.account_id.clone())
        .filter(|s| !s.is_empty())
}

fn access_token_inner(provider: &str, force: bool) -> Option<String> {
    let name = canonical_provider(provider);
    let creds = slot(&load_auth(), &name)?.clone();
    if creds.access_token.is_empty() && creds.refresh_token.is_empty() {
        return None;
    }
    if !force && !needs_refresh(&creds) && !creds.access_token.is_empty() {
        return Some(creds.access_token);
    }
    if creds.refresh_token.is_empty() {
        return None;
    }
    match refresh_provider(&name, &creds.refresh_token, &creds.source) {
        Ok(fresh) => save_provider(&name, fresh).ok().map(|c| c.access_token),
        Err(_) => {
            if !force && !creds.access_token.is_empty() {
                Some(creds.access_token)
            } else {
                None
            }
        }
    }
}

fn refresh_provider(provider: &str, refresh: &str, source: &str) -> Result<OAuthCreds, String> {
    match provider {
        "grok" => refresh_token(refresh, source),
        "anthropic" => refresh_anthropic_token(refresh, source),
        "codex" => refresh_chatgpt_token(refresh, source),
        other => Err(format!("unknown OAuth provider: {other}")),
    }
}

fn needs_refresh(creds: &OAuthCreds) -> bool {
    let Ok(exp) = DateTime::parse_from_rfc3339(&creds.expires_at) else {
        return true;
    };
    let exp = exp.with_timezone(&Utc);
    Utc::now() + Duration::seconds(REFRESH_SKEW_SECS) >= exp
}

pub fn login(provider: &str, opts: LoginOptions) -> Result<LoginResult, String> {
    let name = canonical_provider(provider);
    if !supported_providers().contains(&name.as_str()) {
        return Err(format!(
            "OAuth login supports {}. Got: {provider}",
            supported_providers().join(", ")
        ));
    }
    if !opts.force {
        if let Some(creds) = slot(&load_auth(), &name).cloned() {
            if !creds.refresh_token.is_empty() && !needs_refresh(&creds) {
                return Ok(LoginResult::from_creds(&creds, None));
            }
            if !creds.refresh_token.is_empty() {
                if let Ok(fresh) = refresh_provider(&name, &creds.refresh_token, &creds.source) {
                    let saved = save_provider(&name, fresh)?;
                    return Ok(LoginResult::from_creds(&saved, None));
                }
            }
        }
    }

    match name.as_str() {
        "grok" => login_grok(opts),
        "anthropic" => login_anthropic(opts),
        "codex" => login_chatgpt(opts),
        _ => Err(format!(
            "OAuth login supports {}. Got: {provider}",
            supported_providers().join(", ")
        )),
    }
}

fn login_grok(opts: LoginOptions) -> Result<LoginResult, String> {
    if opts.import_grok_cli && !opts.force {
        if let Some(local) = detect_grok_cli_token() {
            if !needs_refresh(&local) {
                let saved = save_grok(local)?;
                return Ok(LoginResult::from_creds(&saved, Some(IMPORT_WARNING.into())));
            }
            match refresh_token(&local.refresh_token, "grok-cli") {
                Ok(mut fresh) => {
                    fresh.source = "grok-cli".into();
                    let saved = save_grok(fresh)?;
                    return Ok(LoginResult::from_creds(&saved, Some(IMPORT_WARNING.into())));
                }
                Err(e) => {
                    eprintln!("Grok CLI token found but refresh failed ({e}); starting OAuth.");
                }
            }
        }
    }

    let use_device = if opts.device {
        true
    } else if opts.browser {
        false
    } else {
        ssh_session()
    };

    let creds = if use_device {
        login_device()?
    } else {
        match login_browser() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Browser login failed ({e}); falling back to device code.");
                login_device()?
            }
        }
    };
    let saved = save_grok(creds)?;
    Ok(LoginResult::from_creds(&saved, None))
}

fn ssh_session() -> bool {
    std::env::var("SSH_CONNECTION")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
        || std::env::var("SSH_TTY")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
}

pub(crate) fn detect_grok_cli_token() -> Option<OAuthCreds> {
    let raw = fs::read_to_string(grok_cli_auth_path()).ok()?;
    parse_grok_cli_auth(&raw)
}

pub(crate) fn parse_grok_cli_auth(raw: &str) -> Option<OAuthCreds> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let obj = value.as_object()?;
    let entry = obj
        .iter()
        .find(|(k, _)| k.starts_with(GROK_CLI_KEY_PREFIX))
        .map(|(_, v)| v)?;
    let access = entry.get("key")?.as_str().filter(|s| !s.is_empty())?;
    let refresh = entry
        .get("refresh_token")?
        .as_str()
        .filter(|s| !s.is_empty())?;
    let expires_at = entry
        .get("expires_at")
        .and_then(|v| v.as_str())
        .unwrap_or("1970-01-01T00:00:00Z")
        .to_string();
    let email = entry
        .get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    let account_id = entry
        .get("user_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    Some(OAuthCreds {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at,
        email,
        account_id,
        source: "grok-cli".into(),
    })
}

// ── Anthropic (Claude Pro/Max) — ocx login anthropic ────────────────

const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ANTHROPIC_AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const ANTHROPIC_TOKEN_URL: &str = "https://api.anthropic.com/v1/oauth/token";
const ANTHROPIC_SCOPE: &str = "org:create_api_key user:profile user:inference";
const ANTHROPIC_CALLBACK_PORT: u16 = 54545;
pub const ANTHROPIC_OAUTH_BETA: &str = "claude-code-20250219,oauth-2025-04-20";
pub const CLAUDE_CODE_SYSTEM_INSTRUCTION: &str =
    "You are a Claude agent, built on Anthropic's Claude Agent SDK.";
const CLAUDE_IMPORT_WARNING: &str =
    "Imported Claude Code session. Token refresh is now owned by waz; Claude Code may need to log in again later.";

fn login_anthropic(opts: LoginOptions) -> Result<LoginResult, String> {
    if opts.import_grok_cli && !opts.force {
        if let Some(local) = detect_claude_code_token() {
            if !needs_refresh(&local) {
                let saved = save_provider("anthropic", local)?;
                return Ok(LoginResult::from_creds(
                    &saved,
                    Some(CLAUDE_IMPORT_WARNING.into()),
                ));
            }
            match refresh_anthropic_token(&local.refresh_token, "claude-code") {
                Ok(mut fresh) => {
                    fresh.source = "claude-code".into();
                    let saved = save_provider("anthropic", fresh)?;
                    return Ok(LoginResult::from_creds(
                        &saved,
                        Some(CLAUDE_IMPORT_WARNING.into()),
                    ));
                }
                Err(e) => {
                    eprintln!("Claude Code token found but refresh failed ({e}); starting OAuth.");
                }
            }
        }
    }
    let creds = anthropic_browser_login()?;
    let saved = save_provider("anthropic", creds)?;
    Ok(LoginResult::from_creds(&saved, None))
}

fn anthropic_browser_login() -> Result<OAuthCreds, String> {
    let redirect_uri = format!("http://localhost:{ANTHROPIC_CALLBACK_PORT}{CALLBACK_PATH}");
    let (code, state, verifier) = pkce_browser_code(
        ANTHROPIC_AUTHORIZE_URL,
        ANTHROPIC_CLIENT_ID,
        ANTHROPIC_SCOPE,
        "localhost",
        ANTHROPIC_CALLBACK_PORT,
        CALLBACK_PATH,
        &[("code", "true")],
    )?;
    let mut exchange_code = code.as_str();
    let mut exchange_state = state.as_str();
    if let Some((c, frag)) = code.split_once('#') {
        exchange_code = c;
        if !frag.is_empty() {
            exchange_state = frag;
        }
    }
    let payload = post_json_token(
        ANTHROPIC_TOKEN_URL,
        &json!({
            "grant_type": "authorization_code",
            "client_id": ANTHROPIC_CLIENT_ID,
            "code": exchange_code,
            "state": exchange_state,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
        }),
    )?;
    creds_from_anthropic_payload(&payload, "", "oauth")
}

fn refresh_anthropic_token(refresh: &str, source: &str) -> Result<OAuthCreds, String> {
    let payload = post_json_token(
        ANTHROPIC_TOKEN_URL,
        &json!({
            "grant_type": "refresh_token",
            "client_id": ANTHROPIC_CLIENT_ID,
            "refresh_token": refresh,
        }),
    )?;
    creds_from_anthropic_payload(&payload, refresh, source)
}

fn creds_from_anthropic_payload(
    payload: &Value,
    refresh_fallback: &str,
    source: &str,
) -> Result<OAuthCreds, String> {
    let access = payload["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Anthropic token response did not include an access token".to_string())?;
    let refresh = payload["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(refresh_fallback);
    if refresh.is_empty() {
        return Err("Anthropic token response did not include a refresh token".into());
    }
    let expires_in = json_u64(&payload["expires_in"]).unwrap_or(3600);
    Ok(OAuthCreds {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at: (Utc::now() + Duration::seconds(expires_in as i64)).to_rfc3339(),
        email: payload["account"]["email_address"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase()),
        account_id: payload["account"]["uuid"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        source: source_or_oauth(source),
    })
}

fn detect_claude_code_token() -> Option<OAuthCreds> {
    let raw = read_claude_keychain().or_else(read_claude_credentials_file)?;
    parse_claude_oauth_payload(&raw)
}

fn read_claude_credentials_file() -> Option<String> {
    let dir = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".claude")
        });
    fs::read_to_string(dir.join(".credentials.json")).ok()
}

fn read_claude_keychain() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    if std::env::var("CLAUDE_CONFIG_DIR")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return None;
    }
    let out = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn parse_claude_oauth_payload(raw: &str) -> Option<OAuthCreds> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let o = value.get("claudeAiOauth")?;
    let access = o.get("accessToken")?.as_str().filter(|s| !s.is_empty())?;
    let refresh = o.get("refreshToken")?.as_str().filter(|s| !s.is_empty())?;
    let expires_ms = o.get("expiresAt").and_then(|v| v.as_i64()).unwrap_or(0);
    let expires_at = chrono::DateTime::from_timestamp(expires_ms.saturating_div(1000), 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    Some(OAuthCreds {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at,
        email: None,
        account_id: None,
        source: "claude-code".into(),
    })
}

fn post_json_token(url: &str, body: &Value) -> Result<Value, String> {
    match ureq::post(url)
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .timeout(StdDuration::from_secs(TOKEN_TIMEOUT_SECS))
        .send_json(body)
    {
        Ok(resp) => resp.into_json().map_err(|e| format!("token response: {e}")),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("token request failed: {code} {text}"))
        }
        Err(e) => Err(format!("token request failed: {e}")),
    }
}

// ── ChatGPT / Codex — ocx login chatgpt ─────────────────────────────

const CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CHATGPT_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
const CHATGPT_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CHATGPT_SCOPE: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CHATGPT_CALLBACK_PORT: u16 = 1455;
const CHATGPT_CALLBACK_PATH: &str = "/auth/callback";
const CODEX_IMPORT_WARNING: &str =
    "Imported Codex CLI session. Token refresh is now owned by waz; Codex CLI may need to log in again later.";

fn login_chatgpt(opts: LoginOptions) -> Result<LoginResult, String> {
    if opts.import_grok_cli && !opts.force {
        if let Some(local) = detect_codex_cli_token() {
            if !needs_refresh(&local) {
                let saved = save_provider("codex", local)?;
                return Ok(LoginResult::from_creds(
                    &saved,
                    Some(CODEX_IMPORT_WARNING.into()),
                ));
            }
            match refresh_chatgpt_token(&local.refresh_token, "codex-cli") {
                Ok(mut fresh) => {
                    fresh.source = "codex-cli".into();
                    let saved = save_provider("codex", fresh)?;
                    return Ok(LoginResult::from_creds(
                        &saved,
                        Some(CODEX_IMPORT_WARNING.into()),
                    ));
                }
                Err(e) => {
                    eprintln!("Codex CLI token found but refresh failed ({e}); starting OAuth.");
                }
            }
        }
    }
    let creds = chatgpt_browser_login()?;
    let saved = save_provider("codex", creds)?;
    Ok(LoginResult::from_creds(&saved, None))
}

fn chatgpt_browser_login() -> Result<OAuthCreds, String> {
    let redirect_uri = format!("http://localhost:{CHATGPT_CALLBACK_PORT}{CHATGPT_CALLBACK_PATH}");
    let (code, _state, verifier) = pkce_browser_code(
        CHATGPT_AUTH_URL,
        CHATGPT_CLIENT_ID,
        CHATGPT_SCOPE,
        "localhost",
        CHATGPT_CALLBACK_PORT,
        CHATGPT_CALLBACK_PATH,
        &[
            ("codex_cli_simplified_flow", "true"),
            ("originator", "opencodex"),
            ("id_token_add_organizations", "true"),
        ],
    )?;
    let payload = post_token(
        CHATGPT_TOKEN_URL,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", CHATGPT_CLIENT_ID),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &verifier),
        ],
    )?;
    creds_from_chatgpt_payload(&payload, "", "oauth")
}

fn refresh_chatgpt_token(refresh: &str, source: &str) -> Result<OAuthCreds, String> {
    let payload = post_token(
        CHATGPT_TOKEN_URL,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", CHATGPT_CLIENT_ID),
            ("refresh_token", refresh),
        ],
    )?;
    creds_from_chatgpt_payload(&payload, refresh, source)
}

fn creds_from_chatgpt_payload(
    payload: &Value,
    refresh_fallback: &str,
    source: &str,
) -> Result<OAuthCreds, String> {
    let access = payload["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "ChatGPT token response did not include an access token".to_string())?;
    let refresh = payload["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(refresh_fallback);
    if refresh.is_empty() {
        return Err("ChatGPT token response did not include a refresh token".into());
    }
    let id_token = payload["id_token"].as_str();
    let expires_in = json_u64(&payload["expires_in"]).unwrap_or(3600);
    Ok(OAuthCreds {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at: (Utc::now() + Duration::seconds(expires_in as i64)).to_rfc3339(),
        email: chatgpt_email(id_token, Some(access)),
        account_id: chatgpt_account_id(id_token, Some(access)),
        source: source_or_oauth(source),
    })
}

fn chatgpt_account_id(id_token: Option<&str>, access: Option<&str>) -> Option<String> {
    for token in [id_token, access].into_iter().flatten() {
        let Some(p) = decode_jwt_payload(token) else {
            continue;
        };
        if let Some(id) = p.get("chatgpt_account_id").and_then(|v| v.as_str()) {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
        if let Some(id) = p
            .get("https://api.openai.com/auth")
            .and_then(|v| v.get("chatgpt_account_id"))
            .and_then(|v| v.as_str())
        {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn chatgpt_email(id_token: Option<&str>, access: Option<&str>) -> Option<String> {
    for token in [id_token, access].into_iter().flatten() {
        if let Some(email) = decode_jwt_payload(token).and_then(|p| {
            p.get("email")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }) {
            if !email.is_empty() {
                return Some(email.to_ascii_lowercase());
            }
        }
    }
    None
}

fn detect_codex_cli_token() -> Option<OAuthCreds> {
    let path = std::env::var("CODEX_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|p| PathBuf::from(p).join("auth.json"))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
                .join("auth.json")
        });
    let raw = fs::read_to_string(path).ok()?;
    parse_codex_cli_auth(&raw)
}

pub(crate) fn parse_codex_cli_auth(raw: &str) -> Option<OAuthCreds> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let tokens = value.get("tokens")?;
    let access = tokens
        .get("access_token")?
        .as_str()
        .filter(|s| !s.is_empty())?;
    let refresh = tokens
        .get("refresh_token")?
        .as_str()
        .filter(|s| !s.is_empty())?;
    let id_token = tokens.get("id_token").and_then(|v| v.as_str());
    let account_id = tokens
        .get("account_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| chatgpt_account_id(id_token, Some(access)));
    let expires_at = jwt_exp_rfc3339(access)
        .or_else(|| {
            value
                .get("last_refresh")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| (d.with_timezone(&Utc) + Duration::hours(1)).to_rfc3339())
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    Some(OAuthCreds {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at,
        email: chatgpt_email(id_token, Some(access)),
        account_id,
        source: "codex-cli".into(),
    })
}

fn jwt_exp_rfc3339(token: &str) -> Option<String> {
    let exp = decode_jwt_payload(token)?.get("exp")?.as_i64()?;
    chrono::DateTime::from_timestamp(exp, 0).map(|d| d.to_rfc3339())
}

fn source_or_oauth(source: &str) -> String {
    if source.is_empty() {
        "oauth".into()
    } else {
        source.into()
    }
}

/// Shared PKCE loopback used by Anthropic and ChatGPT (Open Codex callback-server).
fn pkce_browser_code(
    authorize_url: &str,
    client_id: &str,
    scope: &str,
    redirect_host: &str,
    port: u16,
    path: &str,
    extra: &[(&str, &str)],
) -> Result<(String, String, String), String> {
    let bind = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&bind).map_err(|e| format!("could not bind {bind}: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("callback socket: {e}"))?;

    let verifier = pkce_verifier();
    let challenge = pkce_challenge(&verifier);
    let state = uuid::Uuid::new_v4().to_string();
    let redirect_uri = format!("http://{redirect_host}:{port}{path}");
    let mut params = vec![
        ("response_type".into(), "code".into()),
        ("client_id".into(), client_id.to_string()),
        ("redirect_uri".into(), redirect_uri),
        ("scope".into(), scope.to_string()),
        ("code_challenge".into(), challenge),
        ("code_challenge_method".into(), "S256".into()),
        ("state".into(), state.clone()),
    ];
    for (k, v) in extra {
        params.push(((*k).to_string(), (*v).to_string()));
    }
    let qs = params
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let url = format!("{authorize_url}?{qs}");
    eprintln!("Open this URL to sign in:\n  {url}");
    if !open_browser(&url) {
        eprintln!("Could not open a browser; paste the URL above into one.");
    }
    let target = wait_for_callback(&listener, path, BROWSER_WAIT_SECS)?;
    let (got_path, query) = split_target(&target);
    if got_path != path {
        return Err(format!("unexpected callback path {got_path}"));
    }
    if let Some(err) = query_param(query, "error") {
        let desc = query_param(query, "error_description").unwrap_or_default();
        return Err(format!("authorization denied: {err} {desc}").trim().into());
    }
    let code = query_param(query, "code").ok_or_else(|| "callback missing code".to_string())?;
    let got_state = query_param(query, "state").unwrap_or_default();
    if got_state != state {
        return Err("OAuth state mismatch".into());
    }
    Ok((code, state, verifier))
}

struct XaiDiscovery {
    authorization_endpoint: String,
    token_endpoint: String,
}

fn fallback_discovery() -> XaiDiscovery {
    XaiDiscovery {
        authorization_endpoint: XAI_AUTHORIZE_URL.to_string(),
        token_endpoint: XAI_TOKEN_URL.to_string(),
    }
}

/// Open Codex: only accept https hosts on x.ai / *.x.ai.
fn is_xai_https(raw: &str) -> bool {
    let lower = raw.trim().to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("https://") else {
        return false;
    };
    let host = rest
        .split('/')
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    host == "x.ai" || host.ends_with(".x.ai")
}

fn validate_xai_endpoint(raw: &str) -> Result<String, String> {
    if !is_xai_https(raw) {
        return Err(format!(
            "xAI OAuth discovery returned an unexpected endpoint: {raw}"
        ));
    }
    Ok(raw.to_string())
}

fn discover_xai_oauth_endpoints() -> XaiDiscovery {
    let Ok(resp) = ureq::get(XAI_OAUTH_DISCOVERY_URL)
        .set("Accept", "application/json")
        .timeout(StdDuration::from_secs(TOKEN_TIMEOUT_SECS))
        .call()
    else {
        return fallback_discovery();
    };
    let Ok(payload) = resp.into_json::<Value>() else {
        return fallback_discovery();
    };
    let Some(auth) = payload["authorization_endpoint"].as_str() else {
        return fallback_discovery();
    };
    let Some(token) = payload["token_endpoint"].as_str() else {
        return fallback_discovery();
    };
    match (validate_xai_endpoint(auth), validate_xai_endpoint(token)) {
        (Ok(authorization_endpoint), Ok(token_endpoint)) => XaiDiscovery {
            authorization_endpoint,
            token_endpoint,
        },
        _ => fallback_discovery(),
    }
}

fn login_browser() -> Result<OAuthCreds, String> {
    let discovery = discover_xai_oauth_endpoints();
    let bind = format!("{CALLBACK_HOST}:{CALLBACK_PORT}");
    let listener = TcpListener::bind(&bind)
        .map_err(|e| format!("could not bind {bind} (is another Grok login running?): {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("callback socket: {e}"))?;

    let verifier = pkce_verifier();
    let challenge = pkce_challenge(&verifier);
    let state = uuid::Uuid::new_v4().to_string();
    let nonce = uuid::Uuid::new_v4().to_string();
    let redirect_uri = format!("http://{CALLBACK_HOST}:{CALLBACK_PORT}{CALLBACK_PATH}");
    let mut params = Vec::new();
    for (k, v) in [
        ("response_type", "code"),
        ("client_id", XAI_OAUTH_CLIENT_ID),
        ("redirect_uri", redirect_uri.as_str()),
        ("scope", XAI_OAUTH_SCOPE),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", state.as_str()),
        ("nonce", nonce.as_str()),
    ] {
        params.push(format!("{k}={}", percent_encode(v)));
    }
    let url = format!("{}?{}", discovery.authorization_endpoint, params.join("&"));

    eprintln!("Open this URL to sign in with SuperGrok / X Premium+:\n  {url}");
    if !open_browser(&url) {
        eprintln!("Could not open a browser; paste the URL above into one.");
    }

    let target = wait_for_callback(&listener, CALLBACK_PATH, BROWSER_WAIT_SECS)?;
    let (path, query) = split_target(&target);
    if path != CALLBACK_PATH {
        return Err(format!("unexpected callback path {path}"));
    }
    if let Some(err) = query_param(query, "error") {
        let desc = query_param(query, "error_description").unwrap_or_default();
        return Err(format!("authorization denied: {err} {desc}").trim().into());
    }
    let code = query_param(query, "code").ok_or_else(|| "callback missing code".to_string())?;
    let got_state = query_param(query, "state").unwrap_or_default();
    if got_state != state {
        return Err("OAuth state mismatch".into());
    }

    let payload = post_token(
        &discovery.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", XAI_OAUTH_CLIENT_ID),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &verifier),
        ],
    )?;
    creds_from_token_payload(&payload, "", "oauth")
}

fn login_device() -> Result<OAuthCreds, String> {
    let discovery = discover_xai_oauth_endpoints();
    let body = form(&[
        ("client_id", XAI_OAUTH_CLIENT_ID),
        ("scope", XAI_OAUTH_SCOPE),
    ]);
    let resp: Value = ureq::post(XAI_DEVICE_CODE_URL)
        .set("Accept", "application/json")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .timeout(StdDuration::from_secs(TOKEN_TIMEOUT_SECS))
        .send_string(&body)
        .map_err(ureq_err)?
        .into_json()
        .map_err(|e| format!("device-code response: {e}"))?;

    let device_code = resp["device_code"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "xAI device-code response missing device_code".to_string())?;
    let user_code = resp["user_code"].as_str().unwrap_or("");
    let verify = resp["verification_uri_complete"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| resp["verification_uri"].as_str())
        .unwrap_or("https://accounts.x.ai");
    let expires_in = json_u64(&resp["expires_in"]).unwrap_or(900);
    let mut interval = json_u64(&resp["interval"]).unwrap_or(5).max(1);

    eprintln!("xAI Grok device login");
    eprintln!("  Open: {verify}");
    if !user_code.is_empty() {
        eprintln!("  Code: {user_code}");
    }
    if !open_browser(verify) {
        eprintln!("Open that URL in any browser and approve access.");
    } else {
        eprintln!("Waiting for approval in the browser...");
    }

    let deadline = Instant::now() + StdDuration::from_secs(expires_in.max(30));
    loop {
        if Instant::now() >= deadline {
            return Err("device authorization timed out; run `waz login grok` again".into());
        }
        std::thread::sleep(StdDuration::from_secs(interval));
        match post_token(
            &discovery.token_endpoint,
            &[
                ("grant_type", DEVICE_GRANT),
                ("device_code", device_code),
                ("client_id", XAI_OAUTH_CLIENT_ID),
            ],
        ) {
            Ok(payload) => return creds_from_token_payload(&payload, "", "oauth"),
            Err(e) => {
                if e.contains("authorization_pending") {
                    continue;
                }
                if e.contains("slow_down") {
                    interval += 5;
                    continue;
                }
                if e.contains("expired_token") {
                    return Err("device code expired; run `waz login grok --device` again".into());
                }
                if e.contains("access_denied") {
                    return Err("authorization denied".into());
                }
                return Err(e);
            }
        }
    }
}

fn refresh_token(refresh: &str, source: &str) -> Result<OAuthCreds, String> {
    let discovery = discover_xai_oauth_endpoints();
    let payload = post_token(
        &discovery.token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", XAI_OAUTH_CLIENT_ID),
            ("refresh_token", refresh),
        ],
    )?;
    creds_from_token_payload(&payload, refresh, source)
}

fn retry_delay_ms(attempt: u32) -> u64 {
    if attempt == 1 {
        100
    } else {
        250
    }
}

fn post_token(token_url: &str, fields: &[(&str, &str)]) -> Result<Value, String> {
    let body = form(fields);
    let mut last = String::from("xAI token request failed");
    for attempt in 1..=3 {
        match ureq::post(token_url)
            .set("Accept", "application/json")
            .set("Content-Type", "application/x-www-form-urlencoded")
            .timeout(StdDuration::from_secs(TOKEN_TIMEOUT_SECS))
            .send_string(&body)
        {
            Ok(resp) => {
                return resp
                    .into_json()
                    .map_err(|e| format!("xAI token response: {e}"));
            }
            Err(ureq::Error::Status(code, resp)) => {
                last = token_status_error(code, resp);
                if !(code == 429 || code >= 500) || attempt == 3 {
                    return Err(last);
                }
            }
            Err(e) => {
                last = format!("xAI token request failed: {e}");
                if attempt == 3 {
                    return Err(last);
                }
            }
        }
        std::thread::sleep(StdDuration::from_millis(retry_delay_ms(attempt)));
    }
    Err(last)
}

fn token_status_error(code: u16, resp: ureq::Response) -> String {
    let text = resp.into_string().unwrap_or_default();
    let parsed: Option<Value> = serde_json::from_str(&text).ok();
    let oauth_error = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.as_str())
        .unwrap_or("");
    let desc = parsed
        .as_ref()
        .and_then(|v| v.get("error_description"))
        .and_then(|e| e.as_str())
        .unwrap_or(text.as_str());
    format!("xAI token request failed: {code} {oauth_error} {desc}")
}

fn creds_from_token_payload(
    payload: &Value,
    refresh_fallback: &str,
    source: &str,
) -> Result<OAuthCreds, String> {
    let access = payload["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "xAI token response did not include an access token".to_string())?;
    let refresh = payload["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(refresh_fallback);
    if refresh.is_empty() {
        return Err("xAI token response did not include a refresh token".into());
    }
    let expires_in = json_u64(&payload["expires_in"]).unwrap_or(3600);
    let expires_at = (Utc::now() + Duration::seconds(expires_in as i64)).to_rfc3339();
    let id_token = payload["id_token"].as_str();
    let (account_id, email) = token_identity(access, id_token);
    Ok(OAuthCreds {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at,
        email,
        account_id,
        source: if source.is_empty() {
            "oauth".into()
        } else {
            source.into()
        },
    })
}

fn token_identity(access: &str, id_token: Option<&str>) -> (Option<String>, Option<String>) {
    let payload = id_token
        .and_then(decode_jwt_payload)
        .or_else(|| decode_jwt_payload(access));
    let Some(p) = payload else {
        return (None, None);
    };
    let account_id = p
        .get("sub")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let email = p
        .get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    (account_id, email)
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let _sig = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let bytes = b64url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

fn wait_for_callback(
    listener: &TcpListener,
    expected_path: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    let deadline = Instant::now() + StdDuration::from_secs(timeout_secs);
    loop {
        if Instant::now() >= deadline {
            return Err("timed out waiting for browser login".into());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(StdDuration::from_secs(10)));
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let target = parse_request_target(&req).unwrap_or_default();
                let (path, query) = split_target(&target);
                if path == "/favicon.ico" {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                let ok = path == expected_path && query_param(query, "code").is_some();
                let body = if ok {
                    "<!doctype html><html><body><h1>waz is signed in</h1><p>You can close this tab.</p></body></html>"
                } else {
                    "<!doctype html><html><body><h1>Login did not complete</h1><p>Return to the terminal.</p></body></html>"
                };
                let status = if ok {
                    "HTTP/1.1 200 OK"
                } else {
                    "HTTP/1.1 400 Bad Request"
                };
                let resp = format!(
                    "{status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                if ok || query_param(query, "error").is_some() {
                    return Ok(target);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(StdDuration::from_millis(150));
            }
            Err(e) => return Err(format!("callback accept: {e}")),
        }
    }
}

fn parse_request_target(req: &str) -> Option<String> {
    let line = req.lines().next()?;
    let mut parts = line.split_whitespace();
    let _method = parts.next()?;
    Some(parts.next()?.to_string())
}

fn split_target(target: &str) -> (&str, &str) {
    match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    }
}

fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if percent_decode(k) == key {
            return Some(percent_decode(v));
        }
    }
    None
}

fn open_browser(url: &str) -> bool {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
    } else {
        std::process::Command::new("xdg-open").arg(url).status()
    };
    matches!(result, Ok(s) if s.success())
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn json_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_f64().map(|f| f as u64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn ureq_err(err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, resp) => {
            let text = resp.into_string().unwrap_or_default();
            format!("HTTP {code}: {text}")
        }
        other => other.to_string(),
    }
}

fn pkce_verifier() -> String {
    // Open Codex uses 96 random bytes, base64url (RFC 7636 max length).
    let mut raw = [0u8; 96];
    for chunk in raw.chunks_mut(16) {
        chunk.copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    b64url_nopad(&raw)
}

fn pkce_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    b64url_nopad(&hasher.finalize())
}

fn b64url_nopad(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((data.len() * 4 + 2) / 3);
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    let rest = data.len() - i;
    if rest == 1 {
        let n = (data[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
    } else if rest == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
    }
    out
}

fn b64url_decode(input: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len().saturating_mul(3) / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in bytes {
        if c == b'=' {
            break;
        }
        buf = (buf << 6) | u32::from(val(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636() {
        // RFC 7636 Appendix B
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn b64url_roundtrip() {
        let data = b"hello world!!";
        let enc = b64url_nopad(data);
        let dec = b64url_decode(&enc).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn parse_grok_cli_auth_extracts_tokens() {
        let raw = r#"{
            "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                "key": "access-test",
                "refresh_token": "refresh-test",
                "expires_at": "2099-01-01T00:00:00Z",
                "email": "User@Example.com",
                "user_id": "abc-123"
            }
        }"#;
        let creds = parse_grok_cli_auth(raw).unwrap();
        assert_eq!(creds.access_token, "access-test");
        assert_eq!(creds.refresh_token, "refresh-test");
        assert_eq!(creds.email.as_deref(), Some("user@example.com"));
        assert_eq!(creds.account_id.as_deref(), Some("abc-123"));
        assert_eq!(creds.source, "grok-cli");
    }

    #[test]
    fn parse_grok_cli_auth_ignores_unrelated_keys() {
        let raw = r#"{"https://example.com::x": {"key": "nope"}}"#;
        assert!(parse_grok_cli_auth(raw).is_none());
    }

    #[test]
    fn creds_from_payload_requires_access_and_refresh() {
        let payload = serde_json::json!({
            "access_token": "a",
            "refresh_token": "r",
            "expires_in": 60
        });
        let creds = creds_from_token_payload(&payload, "", "oauth").unwrap();
        assert_eq!(creds.access_token, "a");
        assert_eq!(creds.refresh_token, "r");
        assert!(needs_refresh(&creds));
    }

    #[test]
    fn creds_from_payload_keeps_refresh_fallback() {
        let payload = serde_json::json!({ "access_token": "a", "expires_in": "3600" });
        let creds = creds_from_token_payload(&payload, "old-refresh", "oauth").unwrap();
        assert_eq!(creds.refresh_token, "old-refresh");
        assert!(!needs_refresh(&creds));
    }

    #[test]
    fn jwt_identity_from_payload() {
        // {"sub":"user-1","email":"a@b.com"}
        let payload = b64url_nopad(br#"{"sub":"user-1","email":"a@b.com"}"#);
        let jwt = format!("hdr.{payload}.sig");
        let (id, email) = token_identity(&jwt, None);
        assert_eq!(id.as_deref(), Some("user-1"));
        assert_eq!(email.as_deref(), Some("a@b.com"));
    }

    #[test]
    fn query_param_decodes_values() {
        let q = "code=abc%2Fde&state=s+1";
        assert_eq!(query_param(q, "code").as_deref(), Some("abc/de"));
        assert_eq!(query_param(q, "state").as_deref(), Some("s 1"));
    }

    #[test]
    fn canonical_provider_aliases() {
        assert_eq!(canonical_provider("xAI"), "grok");
        assert_eq!(canonical_provider("grok-oauth"), "grok");
        assert_eq!(canonical_provider("claude"), "anthropic");
        assert_eq!(canonical_provider("chatgpt"), "codex");
        assert_eq!(canonical_provider("openai"), "openai");
    }

    #[test]
    fn parse_claude_code_credentials() {
        let raw = r#"{
            "claudeAiOauth": {
                "accessToken": "at",
                "refreshToken": "rt",
                "expiresAt": 4092518400000
            }
        }"#;
        let creds = parse_claude_oauth_payload(raw).unwrap();
        assert_eq!(creds.access_token, "at");
        assert_eq!(creds.refresh_token, "rt");
        assert_eq!(creds.source, "claude-code");
        assert!(!needs_refresh(&creds));
    }

    #[test]
    fn parse_codex_cli_auth_tokens() {
        let raw = r#"{
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "codex-access",
                "refresh_token": "codex-refresh",
                "account_id": "acct-1"
            },
            "last_refresh": "2099-01-01T00:00:00Z"
        }"#;
        let creds = parse_codex_cli_auth(raw).unwrap();
        assert_eq!(creds.access_token, "codex-access");
        assert_eq!(creds.refresh_token, "codex-refresh");
        assert_eq!(creds.account_id.as_deref(), Some("acct-1"));
        assert_eq!(creds.source, "codex-cli");
    }

    #[test]
    fn xai_endpoint_validation_matches_opencodex() {
        assert!(is_xai_https("https://auth.x.ai/oauth2/token"));
        assert!(is_xai_https("https://accounts.x.ai/sign-in"));
        assert!(!is_xai_https("http://auth.x.ai/oauth2/token"));
        assert!(!is_xai_https("https://evil.example/x.ai"));
        assert!(!is_xai_https("https://auth.x.ai.evil.com/token"));
        assert!(validate_xai_endpoint("https://not-xai.example/token").is_err());
    }

    #[test]
    fn pkce_verifier_is_rfc7636_length() {
        let v = pkce_verifier();
        assert!(v.len() >= 43 && v.len() <= 128, "len={}", v.len());
        assert!(v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }
}
