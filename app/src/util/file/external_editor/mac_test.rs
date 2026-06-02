use super::is_waz_bundle;

#[test]
fn is_waz_bundle_recognises_waz_channels() {
    // OSS (Waz) itself.
    assert!(is_waz_bundle("dev.goldcoders.waz"));
    // Each channel of the upstream Warp - is also regarded as this application family, allowing default-app redirection.
    assert!(is_waz_bundle("dev.warp.Waz"));
    assert!(is_waz_bundle("dev.warp.WarpDev"));
    assert!(is_waz_bundle("dev.warp.WarpPreview"));
    assert!(is_waz_bundle("dev.warp.WarpOss"));
}

#[test]
fn is_waz_bundle_rejects_other_apps() {
    assert!(!is_waz_bundle("com.microsoft.VSCode"));
    assert!(!is_waz_bundle("com.apple.TextEdit"));
    assert!(!is_waz_bundle("dev.zed.Zed"));
    assert!(!is_waz_bundle("invalid"));
    assert!(!is_waz_bundle(""));
}
