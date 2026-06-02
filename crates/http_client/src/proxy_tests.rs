//! Unit tests for the `proxy` module.
//!
//! reqwest 0.12 does not provide a public API for us to query registered `Proxy` objects on `ClientBuilder`,
//! so we can only do minimal validation based on observable behavior (whether the constructed `Client` succeeds after `apply`).
//! Finer verification of "whether the proxy is actually used" is left to integration tests (which require launching a local mitm).
//!
//! Note: Under the `rustls-tls-native-roots-no-provider` feature, reqwest's `.build()` requires
//! a global crypto provider to be installed, otherwise it panics. In production, this is installed by
//! `app/src/lib.rs::init_common`; in the unit test process, we need to install it ourselves.

use super::*;
use std::sync::Once;

static INSTALL_CRYPTO_PROVIDER: Once = Once::new();

/// Called before running tests that invoke reqwest `.build()`, only takes effect on the first call.
fn ensure_crypto_provider() {
    INSTALL_CRYPTO_PROVIDER.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Constructs a builder with native CA loading disabled, to avoid build failures in environments where system certificates are hard to obtain.
fn test_builder() -> reqwest::ClientBuilder {
    ensure_crypto_provider();
    reqwest::ClientBuilder::new()
        .tls_built_in_native_certs(false)
        .tls_built_in_root_certs(false)
}

#[test]
fn proxy_mode_from_str_lenient_handles_variants() {
    assert_eq!(ProxyMode::from_str_lenient("system"), ProxyMode::System);
    assert_eq!(ProxyMode::from_str_lenient("SYSTEM"), ProxyMode::System);
    assert_eq!(ProxyMode::from_str_lenient("custom"), ProxyMode::Custom);
    assert_eq!(ProxyMode::from_str_lenient("off"), ProxyMode::Off);
    assert_eq!(ProxyMode::from_str_lenient("disabled"), ProxyMode::Off);
    assert_eq!(ProxyMode::from_str_lenient("none"), ProxyMode::Off);
    // Unknown values fall back to Off, consistent with the default item, avoiding accidental use of system proxy.
    assert_eq!(ProxyMode::from_str_lenient("wat"), ProxyMode::Off);
}

#[test]
fn proxy_mode_as_str_roundtrip() {
    for mode in [ProxyMode::System, ProxyMode::Custom, ProxyMode::Off] {
        let s = mode.as_str();
        assert_eq!(ProxyMode::from_str_lenient(s), mode);
    }
}

#[test]
fn apply_system_returns_default_builder() {
    let cfg = ProxyConfig {
        mode: ProxyMode::System,
        ..Default::default()
    };
    // Verify that it does not panic and builds successfully.
    let builder = cfg.apply(test_builder()).no_proxy();
    // Layering another no_proxy on top is only to avoid actually resolving system proxy during build;
    // the core assertion is that apply does not panic.
    let _client = builder.build().expect("System mode should build successfully");
}

#[test]
fn apply_off_disables_proxy_without_error() {
    let cfg = ProxyConfig {
        mode: ProxyMode::Off,
        ..Default::default()
    };
    let builder = cfg.apply(test_builder());
    let _client = builder.build().expect("Off mode should build successfully");
}

#[test]
fn apply_custom_with_valid_url_succeeds() {
    let cfg = ProxyConfig {
        mode: ProxyMode::Custom,
        url: "http://proxy.corp:8080".to_string(),
        ..Default::default()
    };
    let builder = cfg.apply(test_builder());
    let _client = builder
        .build()
        .expect("Custom mode + valid URL should build successfully");
}

#[test]
fn apply_custom_with_basic_auth_succeeds() {
    let cfg = ProxyConfig {
        mode: ProxyMode::Custom,
        url: "http://proxy.corp:8080".to_string(),
        username: "alice".to_string(),
        password: "s3cret".to_string(),
        ..Default::default()
    };
    let builder = cfg.apply(test_builder());
    let _client = builder.build().expect("Custom + auth should build successfully");
}

#[test]
fn apply_custom_with_no_proxy_list_succeeds() {
    let cfg = ProxyConfig {
        mode: ProxyMode::Custom,
        url: "http://proxy.corp:8080".to_string(),
        no_proxy: "localhost,127.0.0.1,.internal".to_string(),
        ..Default::default()
    };
    let builder = cfg.apply(test_builder());
    let _client = builder.build().expect("Custom + no_proxy should build successfully");
}

#[test]
fn apply_custom_with_empty_url_falls_back_silently() {
    let cfg = ProxyConfig {
        mode: ProxyMode::Custom,
        url: String::new(),
        ..Default::default()
    };
    // Should not panic, equivalent to falling back to System (reqwest default).
    let builder = cfg.apply(test_builder()).no_proxy();
    let _client = builder.build().expect("empty URL should fall back silently");
}

#[test]
fn apply_custom_with_invalid_url_falls_back_silently() {
    let cfg = ProxyConfig {
        mode: ProxyMode::Custom,
        url: "://not a url".to_string(),
        ..Default::default()
    };
    let builder = cfg.apply(test_builder()).no_proxy();
    let _client = builder.build().expect("invalid URL should fall back silently");
}

#[test]
fn set_and_read_global_config_roundtrip() {
    // Note: OnceLock is global, tests cannot assume isolation; here we only verify that what is read back after set is what was written.
    let cfg = ProxyConfig {
        mode: ProxyMode::Custom,
        url: "http://test-proxy:1234".to_string(),
        username: "u".to_string(),
        password: "p".to_string(),
        no_proxy: "a,b".to_string(),
    };
    set_global_proxy_config(cfg.clone());
    let read_back = current_proxy_config();
    assert_eq!(read_back.mode, cfg.mode);
    assert_eq!(read_back.url, cfg.url);
    assert_eq!(read_back.username, cfg.username);
    assert_eq!(read_back.password, cfg.password);
    assert_eq!(read_back.no_proxy, cfg.no_proxy);

    // Reset back to default to avoid contaminating other tests.
    set_global_proxy_config(ProxyConfig::default());
}
