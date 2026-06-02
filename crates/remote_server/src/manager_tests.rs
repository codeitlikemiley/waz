//! Pure function-level unit tests for `manager.rs`.
//!
//! These only cover pure function helpers without touching `RemoteServerManager` itself,
//! because the latter depends on `warpui::Entity` / `ModelContext` and requires a full App
//! context, which is more suitable for the integration testing framework.

use super::*;

// ---------------------------------------------------------------------------
// version_is_compatible
// ---------------------------------------------------------------------------

#[test]
fn version_compat_both_tagged_and_equal() {
    assert!(version_is_compatible(
        Some("v0.2026.05.10.stable"),
        "v0.2026.05.10.stable",
    ));
}

#[test]
fn version_compat_both_tagged_and_different() {
    assert!(!version_is_compatible(
        Some("v0.2026.05.10.stable"),
        "v0.2026.05.10.preview",
    ));
}

#[test]
fn version_compat_both_untagged() {
    // Client has no `GIT_RELEASE_TAG` (e.g. running via cargo run), and server reports an empty string
    // (dev deployment via `script/deploy_remote_server`): treat as compatible so that
    // the local development cycle is not disrupted.
    assert!(version_is_compatible(None, ""));
}

#[test]
fn version_compat_client_tagged_server_untagged() {
    // Client is a release build, server is a dev deployment -> treat as incompatible, which normally
    // triggers the reinstall flow.
    assert!(!version_is_compatible(Some("v0.2026.05.10.stable"), ""));
}

#[test]
fn version_compat_client_untagged_server_tagged() {
    // **Critical Scenario**: Waz client has no tag (cargo build), while
    // the server is a release from the official CDN (with tag). The original helper would determine
    // them incompatible and trigger `remove_remote_server_binary` -> infinite loop.
    // This test only records that `version_is_compatible` itself behaves the same,
    // and the actual skipping of checks is handled by [`should_enforce_remote_version_check`].
    assert!(!version_is_compatible(None, "v0.2026.05.10.stable"));
}

// ---------------------------------------------------------------------------
// should_enforce_remote_version_check
// ---------------------------------------------------------------------------

#[test]
fn enforce_version_check_skipped_on_oss() {
    // When Waz temporarily reuses the official release binary, the client and server versions
    // will never match, so strict checks must be skipped.
    assert!(!should_enforce_remote_version_check(Channel::Oss));
}

#[test]
fn enforce_version_check_kept_on_official_channels() {
    // On official channels, the client and server either both come from the same release CI,
    // or both come from a local deployment of `script/deploy_remote_server`. Therefore, strict
    // checks are still necessary to preserve the self-healing path for stale binaries.
    for channel in [
        Channel::Stable,
        Channel::Preview,
        Channel::Dev,
        Channel::Local,
        Channel::Integration,
    ] {
        assert!(
            should_enforce_remote_version_check(channel),
            "channel {channel:?} should still enforce version check"
        );
    }
}
