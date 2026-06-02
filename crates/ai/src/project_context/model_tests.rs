use super::*;
use std::path::PathBuf;

#[test]
fn test_find_applicable_rules_empty_rules() {
    let rules = ProjectRules { rules: vec![] };
    let path = PathBuf::from("/a/b/c/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert!(result.is_empty());
}

#[test]
fn test_find_applicable_rules_no_matching_rules() {
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("/x/y/WARP.md"), "content1".to_string());
    rules.upsert_rule(Path::new("/z/AGENTS.md"), "content2".to_string());

    let path = PathBuf::from("/a/b/c/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert!(result.is_empty());
}

#[test]
fn test_find_applicable_rules_single_matching_rule() {
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("/a/WARP.md"), "content1".to_string());
    rules.upsert_rule(Path::new("/x/AGENTS.md"), "content2".to_string());

    let path = PathBuf::from("/a/b/c/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("/a/WARP.md"));
}

#[test]
fn test_find_applicable_rules_includes_all_ancestor_rules() {
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("/a/WARP.md"), "root_warp".to_string());
    rules.upsert_rule(Path::new("/a/b/WARP.md"), "nested_warp".to_string());
    rules.upsert_rule(Path::new("/a/b/c/WARP.md"), "deep_warp".to_string());

    let path = PathBuf::from("/a/b/c/d/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 3);

    // All should be WARP.md files (same priority), order is not specified by depth
    // Just verify all expected rules are present
    let paths: Vec<PathBuf> = result.iter().map(|r| r.path.clone()).collect();
    assert!(paths.contains(&PathBuf::from("/a/WARP.md")));
    assert!(paths.contains(&PathBuf::from("/a/b/WARP.md")));
    assert!(paths.contains(&PathBuf::from("/a/b/c/WARP.md")));
}

#[test]
fn test_find_applicable_rules_multiple_patterns() {
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("/a/b/AGENTS.md"), "agents_content".to_string());
    rules.upsert_rule(Path::new("/a/WARP.md"), "warp_content".to_string());

    let path = PathBuf::from("/a/b/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 2);

    assert_eq!(result[0].path, PathBuf::from("/a/b/AGENTS.md"));
    assert_eq!(result[0].content, "agents_content");
    assert_eq!(result[1].path, PathBuf::from("/a/WARP.md"));
    assert_eq!(result[1].content, "warp_content");
}

#[test]
fn test_find_applicable_rules_exact_path_match() {
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("/a/b/WARP.md"), "exact_match".to_string());

    let path = PathBuf::from("/a/b/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("/a/b/WARP.md"));
    assert_eq!(result[0].content, "exact_match");
}

#[test]
fn test_find_applicable_rules_ignores_deeper_paths() {
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("/a/WARP.md"), "applicable".to_string());
    rules.upsert_rule(Path::new("/a/b/c/d/e/WARP.md"), "too_deep".to_string()); // Path doesn't contain /a/b

    let path = PathBuf::from("/a/b/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("/a/WARP.md"));
    assert_eq!(result[0].content, "applicable");
}

#[test]
fn test_find_applicable_rules_handles_root_path() {
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("/WARP.md"), "root_rule".to_string());

    let path = PathBuf::from("/a/b/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("/WARP.md"));
    assert_eq!(result[0].content, "root_rule");
}

#[test]
fn test_find_applicable_rules_complex_scenario() {
    // This test covers the example from the original request:
    // For path /a/b/c/file.rs with rules:
    // - /a/WARP.md
    // - /a/AGENTS.md
    // - /a/b/WARP.md
    // - /a/b/AGENTS.md
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("/a/WARP.md"), "a_warp".to_string());
    rules.upsert_rule(Path::new("/a/AGENTS.md"), "a_agents".to_string());
    rules.upsert_rule(Path::new("/a/b/WARP.md"), "ab_warp".to_string());
    rules.upsert_rule(Path::new("/a/b/AGENTS.md"), "ab_agents".to_string());
    rules.upsert_rule(Path::new("/x/WARP.md"), "irrelevant".to_string()); // Should be ignored

    let path = PathBuf::from("/a/b/c/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 2);

    // Expect only WARP.md files to be included as they have higher priority.
    assert_eq!(result[0].path, PathBuf::from("/a/WARP.md"));
    assert_eq!(result[0].content, "a_warp");
    assert_eq!(result[1].path, PathBuf::from("/a/b/WARP.md"));
    assert_eq!(result[1].content, "ab_warp");
}

#[test]
fn test_find_applicable_rules_handles_unknown_file_patterns() {
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("/a/WARP.md"), "known_pattern".to_string());
    rules.upsert_rule(Path::new("/a/UNKNOWN.md"), "unknown_pattern".to_string());
    let path = PathBuf::from("/a/file.rs");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 1);

    assert_eq!(result[0].path, PathBuf::from("/a/WARP.md"));
    assert_eq!(result[0].content, "known_pattern");
}

#[test]
fn test_find_applicable_rules_with_relative_paths() {
    let mut rules = ProjectRules::default();

    rules.upsert_rule(Path::new("src/WARP.md"), "src_warp".to_string());
    rules.upsert_rule(
        Path::new("src/components/WARP.md"),
        "components_warp".to_string(),
    );

    let path = PathBuf::from("src/components/Button.tsx");

    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 2);

    // Both are WARP.md files (same priority), order within same priority is not guaranteed
    // Just verify both rules are present
    let paths: Vec<PathBuf> = result.iter().map(|r| r.path.clone()).collect();
    assert!(paths.contains(&PathBuf::from("src/WARP.md")));
    assert!(paths.contains(&PathBuf::from("src/components/WARP.md")));
}

// ---------------------------------------------------------------------------
// Fast-path tests (aimed at ProjectContextModel::scan_fast_path + fast_path_entry_still_valid)
// ---------------------------------------------------------------------------
//
// These tests run on real fs (temporary directory) and do not depend on ModelContext. Covers:
//   - cwd itself has AGENTS.md -> hit
//   - WARP.md takes priority over AGENTS.md (same directory)
//   - ancestor directory rules can be found by findUp
//   - no rules -> returns None
//   - invalidation check: modified file mtime -> still_valid returns false
//   - invalidation check: newly added rule file in walked directory -> still_valid returns false

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_finds_agents_md_in_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "hello agents").unwrap();

    let entry = ProjectContextModel::scan_fast_path(&cwd);
    assert_eq!(entry.rules.len(), 1, "expected to match 1 rule");
    assert_eq!(entry.rules[0].content, "hello agents");
    assert_eq!(entry.rules[0].path, cwd.join("AGENTS.md"));
    assert_eq!(entry.root_path, cwd);
    assert_eq!(entry.stamps.len(), 1);
}

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_warp_md_takes_priority_over_agents_md() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    std::fs::write(cwd.join("WARP.md"), "warp wins").unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "agents loses").unwrap();

    let entry = ProjectContextModel::scan_fast_path(&cwd);
    assert_eq!(
        entry.rules.len(),
        1,
        "Only 1 of the two rule files in the same directory is taken (aligned with RuleAtPath::respected_rule)"
    );
    assert_eq!(entry.rules[0].content, "warp wins");
    assert_eq!(entry.rules[0].path, cwd.join("WARP.md"));
}

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_finds_rule_in_ancestor_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let sub = root.join("a").join("b").join("c");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(root.join("AGENTS.md"), "ancestor rule").unwrap();

    let entry = ProjectContextModel::scan_fast_path(&sub);
    assert_eq!(entry.rules.len(), 1);
    assert_eq!(entry.rules[0].content, "ancestor rule");
    assert_eq!(entry.root_path, root);
}

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_returns_empty_when_no_rules_anywhere() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();

    let entry = ProjectContextModel::scan_fast_path(&cwd);
    assert!(entry.rules.is_empty());
    // root_path falls back to cwd (semantically aligned with None return of find_applicable_rules)
    assert_eq!(entry.root_path, cwd);
    // walked_dir_stamps is not empty (at least walked the cwd itself, negative cache can take effect)
    assert!(!entry.walked_dir_stamps.is_empty());
}

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_still_valid_when_nothing_changed() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "stable").unwrap();

    let entry = ProjectContextModel::scan_fast_path(&cwd);
    assert!(ProjectContextModel::fast_path_entry_still_valid(&entry));
}

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_invalidated_when_rule_file_mtime_changes() {
    use filetime::{set_file_mtime, FileTime};

    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    let rule = cwd.join("AGENTS.md");
    std::fs::write(&rule, "v1").unwrap();

    let entry = ProjectContextModel::scan_fast_path(&cwd);
    assert!(ProjectContextModel::fast_path_entry_still_valid(&entry));

    // bump mtime by 10s -> cache should be detected as invalid
    let stamp = entry.stamps[0].1;
    let new_mtime = FileTime::from_system_time(stamp + std::time::Duration::from_secs(10));
    set_file_mtime(&rule, new_mtime).unwrap();
    assert!(!ProjectContextModel::fast_path_entry_still_valid(&entry));
}

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_invalidated_when_new_rule_file_appears_in_walked_dir() {
    use filetime::{set_file_mtime, FileTime};

    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();

    // First scan: no rules matched (negative cache)
    let entry = ProjectContextModel::scan_fast_path(&cwd);
    assert!(entry.rules.is_empty());

    // Record original directory mtime, later manually advance it to trigger invalidation detection.
    // Create the file only here — but some file systems do not immediately update directory mtime upon file creation.
    // For test stability, call set_file_mtime explicitly after creating the file to ensure the directory mtime differs from the stamp.
    std::fs::write(cwd.join("AGENTS.md"), "new!").unwrap();
    let original_dir_mtime = entry.walked_dir_stamps[0].1;
    let bumped =
        FileTime::from_system_time(original_dir_mtime + std::time::Duration::from_secs(10));
    set_file_mtime(&cwd, bumped).unwrap();

    assert!(!ProjectContextModel::fast_path_entry_still_valid(&entry));
}

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_walk_depth_bounded() {
    // Verify that MAX_WALK_DEPTH takes effect: directories with depth exceeding the upper limit will not stat the top-level rule file.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    // Construct >=7 levels of subdirectories (MAX_WALK_DEPTH = 6)
    let mut deep = root.clone();
    for seg in ["a", "b", "c", "d", "e", "f", "g"] {
        deep.push(seg);
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(root.join("AGENTS.md"), "top").unwrap();

    let entry = ProjectContextModel::scan_fast_path(&deep);
    // Cannot reach the top level, unable to retrieve rules
    assert!(entry.rules.is_empty(), "should not stat to the top-level rule file after depth limit exceeded");
    // walked_dir_stamps does not exceed MAX_WALK_DEPTH
    assert!(entry.walked_dir_stamps.len() <= 6);
}

// ---------------------------------------------------------------------------
// Special test for CLAUDE.md default recognition
// ---------------------------------------------------------------------------

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_finds_claude_md() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    std::fs::write(cwd.join("CLAUDE.md"), "claude rules").unwrap();

    let entry = ProjectContextModel::scan_fast_path(&cwd);
    assert_eq!(entry.rules.len(), 1, "CLAUDE.md should be recognized by default");
    assert_eq!(entry.rules[0].content, "claude rules");
    assert_eq!(entry.rules[0].path, cwd.join("CLAUDE.md"));
}

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_warp_md_priority_over_claude_md() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    std::fs::write(cwd.join("WARP.md"), "warp wins").unwrap();
    std::fs::write(cwd.join("CLAUDE.md"), "claude loses").unwrap();

    let entry = ProjectContextModel::scan_fast_path(&cwd);
    assert_eq!(entry.rules.len(), 1);
    assert_eq!(entry.rules[0].content, "warp wins");
    assert_eq!(entry.rules[0].path, cwd.join("WARP.md"));
}

#[cfg(feature = "local_fs")]
#[test]
fn fast_path_agents_md_priority_over_claude_md() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().canonicalize().unwrap();
    std::fs::write(cwd.join("AGENTS.md"), "agents wins").unwrap();
    std::fs::write(cwd.join("CLAUDE.md"), "claude loses").unwrap();

    let entry = ProjectContextModel::scan_fast_path(&cwd);
    assert_eq!(entry.rules.len(), 1);
    assert_eq!(entry.rules[0].content, "agents wins");
    assert_eq!(entry.rules[0].path, cwd.join("AGENTS.md"));
}

#[test]
fn upsert_rule_recognizes_claude_md() {
    // Pure in-memory path (no fs interaction) to verify that ProjectRules::upsert_rule recognizes CLAUDE.md
    let mut rules = ProjectRules::default();
    rules.upsert_rule(Path::new("/a/CLAUDE.md"), "claude in /a".to_string());

    let path = PathBuf::from("/a/sub/file.rs");
    let result = rules.find_active_or_applicable_rules(&path).active_rules;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("/a/CLAUDE.md"));
    assert_eq!(result[0].content, "claude in /a");
}

#[test]
fn upsert_rule_priority_three_way() {
    // WARP / AGENTS / CLAUDE coexisting in the same directory -> only take WARP (highest priority)
    let mut rules = ProjectRules::default();
    rules.upsert_rule(Path::new("/a/WARP.md"), "warp".to_string());
    rules.upsert_rule(Path::new("/a/AGENTS.md"), "agents".to_string());
    rules.upsert_rule(Path::new("/a/CLAUDE.md"), "claude".to_string());

    let result = rules
        .find_active_or_applicable_rules(&PathBuf::from("/a/x.rs"))
        .active_rules;
    assert_eq!(result.len(), 1, "Only take the highest priority rule when multiple exist in the same directory");
    assert_eq!(result[0].path, PathBuf::from("/a/WARP.md"));
}

#[test]
fn upsert_rule_priority_agents_beats_claude() {
    // AGENTS + CLAUDE in the same directory -> take AGENTS
    let mut rules = ProjectRules::default();
    rules.upsert_rule(Path::new("/a/AGENTS.md"), "agents".to_string());
    rules.upsert_rule(Path::new("/a/CLAUDE.md"), "claude".to_string());

    let result = rules
        .find_active_or_applicable_rules(&PathBuf::from("/a/x.rs"))
        .active_rules;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("/a/AGENTS.md"));
}

#[test]
fn remove_rule_recognizes_claude_md() {
    let mut rules = ProjectRules::default();
    rules.upsert_rule(Path::new("/a/CLAUDE.md"), "x".to_string());
    rules.upsert_rule(Path::new("/a/AGENTS.md"), "y".to_string());

    let removed = rules.remove_rule(Path::new("/a/CLAUDE.md"));
    assert!(removed.is_some(), "able to remove CLAUDE.md");

    // After removing CLAUDE, AGENTS remains as the active rule for this directory
    let result = rules
        .find_active_or_applicable_rules(&PathBuf::from("/a/x.rs"))
        .active_rules;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("/a/AGENTS.md"));
}

#[test]
fn upsert_rule_case_insensitive_filename() {
    // Case-insensitive: claude.md / Agents.MD should also be recognized
    let mut rules = ProjectRules::default();
    rules.upsert_rule(Path::new("/a/claude.md"), "lower".to_string());

    let result = rules
        .find_active_or_applicable_rules(&PathBuf::from("/a/x.rs"))
        .active_rules;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, PathBuf::from("/a/claude.md"));
}
