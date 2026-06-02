use super::ChannelState;

// Waz Wave 5-5: `derive_http_origin_from_ws_url` call + 3 wss/ws path tests are
// physically deleted along with `ChannelState::rtc_http_url()`.

/// `ChannelState::init()` (the static default for OSS builds) must satisfy
/// the cloud-disabled predicate; the cloud-removal plan's Phase 5 short-circuit
/// depends on this invariant.
#[test]
fn default_oss_state_is_cloud_disabled() {
    assert!(ChannelState::is_cloud_disabled());
}
