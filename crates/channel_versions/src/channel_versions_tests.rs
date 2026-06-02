use chrono::NaiveDate;

use super::*;

#[test]
fn test_parse_version_string() {
    let version_string = "v0.2023.05.15.08.04.stable_01";
    let parsed_version: ParsedVersion = version_string
        .try_into()
        .expect("version string is parsable");
    assert_eq!(parsed_version.major, 0);
    assert_eq!(
        parsed_version.date,
        NaiveDate::from_ymd_opt(2023, 5, 15)
            .unwrap()
            .and_hms_opt(8, 4, 0)
            .unwrap()
    );
    assert_eq!(parsed_version.patch, 1);
}

#[test]
fn test_major_versions_compare_correctly() {
    let older_version: ParsedVersion = "v0.2023.05.15.08.04.stable_01"
        .try_into()
        .expect("older_version is parsable");
    let newer_version: ParsedVersion = "v1.2023.05.15.08.04.stable_01"
        .try_into()
        .expect("newer_version is parsable");
    assert!(newer_version > older_version);
}

#[test]
fn test_dates_compare_correctly() {
    let older_version: ParsedVersion = "v0.2023.05.15.08.04.stable_01"
        .try_into()
        .expect("older_version is parsable");
    let newer_version: ParsedVersion = "v0.2023.05.22.08.04.stable_00"
        .try_into()
        .expect("newer_version is parsable");
    assert!(newer_version > older_version);
}

#[test]
fn test_patches_compare_correctly() {
    let older_version: ParsedVersion = "v0.2023.05.15.08.04.stable_00"
        .try_into()
        .expect("older_version is parsable");
    let newer_version: ParsedVersion = "v0.2023.05.15.08.04.stable_01"
        .try_into()
        .expect("newer_version is parsable");
    assert!(newer_version > older_version);
}

#[test]
fn test_ignores_unknown_channels() {
    // We no longer support or parse-out beta and canary versions, but we
    // need to be able to parse a JSON file that still contains them.
    let channel_version_string = r#"{
        "beta": {
          "version": "v0.2024.01.30.16.52.beta_00"
        },
        "canary": {
          "version": "v0.2022.09.29.08.08.canary_00"
        },
        "dev": {
          "version": "v0.2024.01.30.20.34.dev_00"
        },
        "preview": {
          "version": "v0.2024.01.30.20.34.preview_00"
        },
        "stable": {
          "version": "v0.2024.01.16.16.31.stable_01"
        }
      }"#;

    let channel_versions: ChannelVersions = serde_json::from_str(channel_version_string)
        .expect("Should be able to parse channel versions");
    assert_eq!(
        channel_versions.stable.version_info().version,
        "v0.2024.01.16.16.31.stable_01"
    );
}

// openWarp (OSS) uses a simplified tag format of vYYYY.MM.DD.N. The following tests ensure that:
// 1. This format can be parsed by `ParsedVersion` (otherwise `is_current_version_ahead_of_latest_version`
//    will always return `Err`, causing the user to be incorrectly guided to "upgrade" to a rolled-back version).
// 2. The size comparison is monotonic on the (major=0, date, patch) triple.
#[test]
fn test_oss_version_parses() {
    let parsed: ParsedVersion = "v2026.05.26.2"
        .try_into()
        .expect("OSS 4-segment tag should parse");
    assert_eq!(parsed.major, 0);
    assert_eq!(
        parsed.date,
        NaiveDate::from_ymd_opt(2026, 5, 26)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
    );
    assert_eq!(parsed.patch, 2);
}

#[test]
fn test_oss_version_without_patch_parses() {
    // Early OSS tags might not have the 4th segment (index).
    let parsed: ParsedVersion = "v2026.05.26"
        .try_into()
        .expect("OSS 3-segment tag should parse");
    assert_eq!(parsed.patch, 0);
}

#[test]
fn test_oss_version_without_v_prefix_parses() {
    let parsed: ParsedVersion = "2026.05.26.2"
        .try_into()
        .expect("OSS tag without v prefix should parse");
    assert_eq!(parsed.patch, 2);
}

#[test]
fn test_oss_version_rollback_detected() {
    // Remote rolled back: the newly released release tag is earlier than the current local version, so
    // `is_current_version_ahead_of_latest_version` should be recognized as true, thus avoiding incorrectly displaying the rolled-back version as an "upgrade".
    let local: ParsedVersion = "v2026.05.26.2".try_into().unwrap();
    let rolled_back_remote: ParsedVersion = "v2026.05.20.1".try_into().unwrap();
    assert!(local > rolled_back_remote);
}

#[test]
fn test_oss_version_newer_patch_wins() {
    let older: ParsedVersion = "v2026.05.26.1".try_into().unwrap();
    let newer: ParsedVersion = "v2026.05.26.2".try_into().unwrap();
    assert!(newer > older);
}

#[test]
fn test_oss_version_newer_date_wins() {
    let older: ParsedVersion = "v2026.05.26.9".try_into().unwrap();
    let newer: ParsedVersion = "v2026.05.27.0".try_into().unwrap();
    assert!(newer > older);
}
