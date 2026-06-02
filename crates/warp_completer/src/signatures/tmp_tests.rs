use super::*;
use std::fs::File;
use tempfile::tempdir;

#[test]
fn test_build_assembled_command() {
    let entry = CommandEntry {
        command: "git checkout <branch>".to_string(),
        description: "Checkout branch".to_string(),
        group: "git".to_string(),
        tokens: vec![TokenDef {
            name: "branch".to_string(),
            description: "Branch name".to_string(),
            required: true,
            flag: None,
            token_type: TokenType::Enum,
            default: None,
            values: Some(vec!["main".to_string(), "dev".to_string()]),
            data_source: None,
        }],
        verified: false,
    };

    let vals = vec!["main".to_string()];
    let cmd = build_assembled_command(&entry, &vals, false);
    assert_eq!(cmd, "git checkout main");

    let preview = build_assembled_command(&entry, &["".to_string()], true);
    assert_eq!(preview, "git checkout <branch>");

    let empty = build_assembled_command(&entry, &["".to_string()], false);
    assert_eq!(empty, "git checkout");
}

#[test]
fn test_extract_token_values() {
    let tokens = vec![
        TokenDef {
            name: "branch".to_string(),
            description: "Branch".to_string(),
            required: true,
            flag: None,
            token_type: TokenType::Enum,
            default: None,
            values: None,
            data_source: None,
        }
    ];
    let values = extract_token_values("git checkout <branch>", &tokens, "git checkout main");
    assert_eq!(values, vec!["main".to_string()]);
}

#[test]
fn test_should_load_schema() {
    let meta_cwd = SchemaMeta {
        tool: "npm".to_string(),
        keywords: vec!["node".to_string()],
        requires_file: Some("package.json".to_string()),
        ..Default::default()
    };
    let meta_always = SchemaMeta {
        tool: "git".to_string(),
        keywords: vec![],
        requires_file: None,
        ..Default::default()
    };

    let dir = tempdir().unwrap();
    let file_path = dir.path().join("package.json");
    File::create(&file_path).unwrap();

    assert!(should_load_schema(&meta_cwd, dir.path().to_str().unwrap()));
    assert!(!should_load_schema(&meta_cwd, "/tmp"));
    assert!(should_load_schema(&meta_always, "/tmp"));
}

#[test]
fn test_load_all_schemas_from_config() {
    let dir = schemas_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let content = std::fs::read_to_string(&path).unwrap();
                let res = serde_json::from_str::<SchemaFile>(&content);
                assert!(res.is_ok(), "Failed to parse schema at {:?}: {:?}", path, res.err());
            }
        }
    }
}

#[test]
fn test_find_git_checkout_command() {
    let cwd = "/Volumes/goldcoders/zap";
    let matched = find_matching_tmp_command("git checkout ", cwd);
    assert!(matched.is_some());
}

#[test]
fn test_build_assembled_command_no_placeholders() {
    let entry = CommandEntry {
        command: "git checkout".to_string(),
        description: "Checkout branch".to_string(),
        group: "git".to_string(),
        tokens: vec![
            TokenDef {
                name: "branch".to_string(),
                description: "Branch name".to_string(),
                required: true,
                flag: None,
                token_type: TokenType::Enum,
                default: None,
                values: Some(vec!["main".to_string(), "dev".to_string()]),
                data_source: None,
            },
            TokenDef {
                name: "create".to_string(),
                description: "Create new branch".to_string(),
                required: false,
                flag: Some("-b".to_string()),
                token_type: TokenType::Boolean,
                default: Some("false".to_string()),
                values: None,
                data_source: None,
            },
        ],
        verified: false,
    };

    let vals = vec!["main".to_string(), "false".to_string()];
    let cmd = build_assembled_command(&entry, &vals, false);
    assert_eq!(cmd, "git checkout main");

    let cmd_create = build_assembled_command(&entry, &["main".to_string(), "true".to_string()], false);
    assert_eq!(cmd_create, "git checkout -b main");

    let preview = build_assembled_command(&entry, &["".to_string(), "false".to_string()], true);
    assert_eq!(preview, "git checkout <branch>");

    let empty = build_assembled_command(&entry, &["".to_string(), "false".to_string()], false);
    assert_eq!(empty, "git checkout");
}

#[test]
fn test_extract_token_values_no_placeholders() {
    let tokens = vec![
        TokenDef {
            name: "branch".to_string(),
            description: "Branch name".to_string(),
            required: true,
            flag: None,
            token_type: TokenType::Enum,
            default: None,
            values: Some(vec!["main".to_string(), "dev".to_string()]),
            data_source: None,
        },
        TokenDef {
            name: "create".to_string(),
            description: "Create new branch".to_string(),
            required: false,
            flag: Some("-b".to_string()),
            token_type: TokenType::Boolean,
            default: Some("false".to_string()),
            values: None,
            data_source: None,
        },
    ];

    let vals = extract_token_values("git checkout", &tokens, "git checkout main");
    assert_eq!(vals, vec!["main".to_string(), "false".to_string()]);

    let vals_create = extract_token_values("git checkout", &tokens, "git checkout -b main");
    assert_eq!(vals_create, vec!["main".to_string(), "true".to_string()]);

    let vals_empty = extract_token_values("git checkout", &tokens, "git checkout");
    assert_eq!(vals_empty, vec!["".to_string(), "false".to_string()]);
}

#[test]
fn test_resolve_command_data_source() {
    let mut entry = CommandEntry {
        command: "git checkout <branch>".to_string(),
        description: "Checkout branch".to_string(),
        group: "git".to_string(),
        tokens: vec![TokenDef {
            name: "branch".to_string(),
            description: "Branch name".to_string(),
            required: true,
            flag: None,
            token_type: TokenType::Enum,
            default: None,
            values: None,
            data_source: Some(DataSource {
                command: Some("echo \"first\nsecond\"".to_string()),
                resolver: None,
                parse: "lines".to_string(),
            }),
        }],
        verified: false,
    };

    resolve_data_sources(&mut entry, "/tmp");
    
    // On non-WASM targets, we expect the command to be executed and values populated.
    #[cfg(not(target_family = "wasm"))]
    {
        assert_eq!(
            entry.tokens[0].values,
            Some(vec!["first".to_string(), "second".to_string()])
        );
        assert_eq!(entry.tokens[0].token_type, TokenType::Enum);
    }
}

#[test]
fn test_resolve_command_data_source_words() {
    let mut entry = CommandEntry {
        command: "git checkout <branch>".to_string(),
        description: "Checkout branch".to_string(),
        group: "git".to_string(),
        tokens: vec![TokenDef {
            name: "branch".to_string(),
            description: "Branch name".to_string(),
            required: true,
            flag: None,
            token_type: TokenType::Enum,
            default: None,
            values: None,
            data_source: Some(DataSource {
                command: Some("echo \"one two\"".to_string()),
                resolver: None,
                parse: "words".to_string(),
            }),
        }],
        verified: false,
    };

    resolve_data_sources(&mut entry, "/tmp");
    
    #[cfg(not(target_family = "wasm"))]
    {
        assert_eq!(
            entry.tokens[0].values,
            Some(vec!["one".to_string(), "two".to_string()])
        );
        assert_eq!(entry.tokens[0].token_type, TokenType::Enum);
    }
}

#[test]
fn test_git_resolve_status_files() {
    #[cfg(not(target_family = "wasm"))]
    {
        let dir = tempdir().unwrap();
        let path = dir.path();

        // Init git repo
        let run_git = |args: &[&str]| {
            command::blocking::Command::new("git")
                .args(args)
                .current_dir(path)
                .output()
                .expect("Failed to run git");
        };

        run_git(&["init"]);
        run_git(&["config", "user.email", "test@example.com"]);
        run_git(&["config", "user.name", "Test User"]);

        // Create a file and commit it
        let file1 = path.join("file1.txt");
        std::fs::write(&file1, "hello").unwrap();
        run_git(&["add", "file1.txt"]);
        run_git(&["commit", "-m", "initial"]);

        // Modify file1.txt (Modified M)
        std::fs::write(&file1, "hello world").unwrap();

        // Create file2.txt (Untracked ??)
        let file2 = path.join("file2.txt");
        std::fs::write(&file2, "untracked").unwrap();

        // Create file3.txt, commit it, then rename it (Renamed R)
        let file3 = path.join("file3.txt");
        std::fs::write(&file3, "rename me").unwrap();
        run_git(&["add", "file3.txt"]);
        run_git(&["commit", "-m", "add file3"]);
        run_git(&["mv", "file3.txt", "file4.txt"]);

        // Create an untracked file with space
        let file_space = path.join("file space.txt");
        std::fs::write(&file_space, "space").unwrap();

        // Create another untracked file that starts with 'a' to test sorting
        let file_a = path.join("a_file.txt");
        std::fs::write(&file_a, "starts with a").unwrap();

        // Resolve status files
        let files = git_resolve_status_files(path.to_str().unwrap()).unwrap();
        
        let expected = vec![
            "a_file.txt".to_string(),
            "file space.txt".to_string(),
            "file1.txt".to_string(),
            "file2.txt".to_string(),
            "file4.txt".to_string(),
        ];
        
        // Assert direct equality. This verifies that the function itself
        // returned the paths sorted and deduplicated.
        assert_eq!(files, expected);
    }
}

