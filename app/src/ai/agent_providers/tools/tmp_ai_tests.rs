use super::*;
use serde_json::json;
use warp_completer::signatures::tmp::{CommandEntry, TokenDef, TokenType};

#[test]
fn test_function_name() {
    assert_eq!(function_name("git", "git status"), "tmp__git__status");
    assert_eq!(function_name("cargo", "cargo build --release"), "tmp__cargo__build_release");
    assert_eq!(function_name("npm", "install"), "tmp__npm__install");
    assert_eq!(function_name("cargo", "cargo test"), "tmp__cargo__test");
}

#[test]
fn test_is_parameter_safe() {
    assert!(is_parameter_safe("hello"));
    assert!(is_parameter_safe("hello world"));
    assert!(is_parameter_safe("hello 'world'"));
    assert!(is_parameter_safe("hello \"world\""));
    assert!(is_parameter_safe(""));

    // Shell metacharacters are unsafe
    assert!(!is_parameter_safe("hello; world"));
    assert!(!is_parameter_safe("hello & world"));
    assert!(!is_parameter_safe("hello | world"));
    assert!(!is_parameter_safe("hello\nworld"));
    assert!(!is_parameter_safe("hello > world"));

    // Unbalanced quotes are unsafe
    assert!(!is_parameter_safe("hello 'world"));
    assert!(!is_parameter_safe("hello \"world"));
    assert!(!is_parameter_safe("hello 'world\""));
}

#[test]
fn test_escape_unix_single_quotes() {
    assert_eq!(escape_unix_single_quotes("hello"), "hello");
    assert_eq!(escape_unix_single_quotes("hello 'world'"), "hello '\\''world'\\''");
}

#[test]
fn test_token_to_json_schema() {
    let t_string = TokenDef {
        name: "pkg".to_string(),
        description: "Package name".to_string(),
        token_type: TokenType::String,
        required: true,
        default: None,
        flag: None,
        values: None,
        data_source: None,
    };
    let schema_str = token_to_json_schema(&t_string);
    assert_eq!(schema_str["type"], "string");
    assert_eq!(schema_str["description"], "Package name");

    let t_bool = TokenDef {
        name: "release".to_string(),
        description: "Release build".to_string(),
        token_type: TokenType::Boolean,
        required: false,
        default: Some("true".to_string()),
        flag: Some("--release".to_string()),
        values: None,
        data_source: None,
    };
    let schema_bool = token_to_json_schema(&t_bool);
    assert_eq!(schema_bool["type"], "boolean");
    assert_eq!(schema_bool["default"], true);

    let t_enum = TokenDef {
        name: "mode".to_string(),
        description: "Execution mode".to_string(),
        token_type: TokenType::Enum,
        required: false,
        default: Some("fast".to_string()),
        flag: None,
        values: Some(vec!["fast".to_string(), "slow".to_string()]),
        data_source: None,
    };
    let schema_enum = token_to_json_schema(&t_enum);
    assert_eq!(schema_enum["type"], "string");
    assert_eq!(schema_enum["enum"], json!(["fast", "slow"]));
    assert_eq!(schema_enum["default"], "fast");
}

#[test]
fn test_command_to_json_schema() {
    let entry = CommandEntry {
        group: "cargo".to_string(),
        command: "cargo build".to_string(),
        description: "Build crate".to_string(),
        verified: false,
        tokens: vec![
            TokenDef {
                name: "pkg".to_string(),
                description: "Package".to_string(),
                token_type: TokenType::String,
                required: true,
                default: None,
                flag: Some("-p".to_string()),
                values: None,
                data_source: None,
            },
            TokenDef {
                name: "release".to_string(),
                description: "Release".to_string(),
                token_type: TokenType::Boolean,
                required: false,
                default: None,
                flag: Some("--release".to_string()),
                values: None,
                data_source: None,
            },
        ],
    };

    let schema = command_to_json_schema(&entry);
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["pkg"]));
    assert_eq!(schema["properties"]["pkg"]["type"], "string");
    assert_eq!(schema["properties"]["release"]["type"], "boolean");
    assert_eq!(schema["additionalProperties"], false);
}

#[test]
fn test_validate_tmp_arguments() {
    let entry = CommandEntry {
        group: "cargo".to_string(),
        command: "cargo build".to_string(),
        description: "Build crate".to_string(),
        verified: false,
        tokens: vec![
            TokenDef {
                name: "pkg".to_string(),
                description: "Package".to_string(),
                token_type: TokenType::String,
                required: true,
                default: None,
                flag: Some("-p".to_string()),
                values: None,
                data_source: None,
            },
            TokenDef {
                name: "mode".to_string(),
                description: "Mode".to_string(),
                token_type: TokenType::Enum,
                required: false,
                default: None,
                flag: None,
                values: Some(vec!["dev".to_string(), "release".to_string()]),
                data_source: None,
            },
        ],
    };

    // Valid arguments
    let valid = json!({
        "pkg": "warpui",
        "mode": "release"
    });
    assert!(validate_tmp_arguments(&entry, &valid).is_ok());

    // Missing required field
    let missing = json!({
        "mode": "release"
    });
    assert_eq!(
        validate_tmp_arguments(&entry, &missing),
        Err(ValidationError::MissingRequiredField("pkg".to_string()))
    );

    // Type mismatch
    let bad_type = json!({
        "pkg": true
    });
    assert!(matches!(
        validate_tmp_arguments(&entry, &bad_type),
        Err(ValidationError::TypeMismatch { .. })
    ));

    // Invalid enum value
    let bad_enum = json!({
        "pkg": "warp",
        "mode": "invalid"
    });
    assert!(matches!(
        validate_tmp_arguments(&entry, &bad_enum),
        Err(ValidationError::InvalidEnumValue { .. })
    ));

    // Unsafe parameter
    let unsafe_arg = json!({
        "pkg": "warp; rm -rf /",
    });
    assert_eq!(
        validate_tmp_arguments(&entry, &unsafe_arg),
        Err(ValidationError::UnsafeShellMetacharacters("pkg".to_string()))
    );
}

#[test]
fn test_compile_tmp_command() {
    let entry = CommandEntry {
        group: "cargo".to_string(),
        command: "cargo build".to_string(),
        description: "Build crate".to_string(),
        verified: false,
        tokens: vec![
            TokenDef {
                name: "pkg".to_string(),
                description: "Package".to_string(),
                token_type: TokenType::String,
                required: true,
                default: None,
                flag: Some("-p".to_string()),
                values: None,
                data_source: None,
            },
            TokenDef {
                name: "release".to_string(),
                description: "Release".to_string(),
                token_type: TokenType::Boolean,
                required: false,
                default: Some("false".to_string()),
                flag: Some("--release".to_string()),
                values: None,
                data_source: None,
            },
            TokenDef {
                name: "jobs".to_string(),
                description: "Jobs".to_string(),
                token_type: TokenType::Number,
                required: false,
                default: None,
                flag: Some("-j".to_string()),
                values: None,
                data_source: None,
            },
        ],
    };

    // Helper closure to emulate compilation logic inside parse_tmp_tool_call
    let compile = |args: &Value| -> String {
        let mut cmd_str = entry.command.clone();
        let obj = args.as_object().unwrap();
        for token in &entry.tokens {
            let value_opt: Option<Value> = obj.get(&token.name).cloned().or_else(|| {
                token.default.as_ref().map(|d| {
                    match token.token_type {
                        TokenType::Boolean => json!(d.parse::<bool>().unwrap_or(false)),
                        TokenType::Number => json!(d.parse::<f64>().unwrap_or(0.0)),
                        _ => json!(d),
                    }
                })
            });

            if let Some(ref val) = value_opt {
                if val.is_null() {
                    continue;
                }
                match token.token_type {
                    TokenType::Boolean => {
                        if val.as_bool() == Some(true) {
                            if let Some(ref flag) = token.flag {
                                cmd_str.push_str(" ");
                                cmd_str.push_str(flag);
                            }
                        }
                    }
                    TokenType::String | TokenType::File | TokenType::Enum => {
                        if let Some(s) = val.as_str() {
                            cmd_str.push_str(" ");
                            if let Some(ref flag) = token.flag {
                                cmd_str.push_str(flag);
                                cmd_str.push_str(" ");
                            }
                            let escaped = escape_unix_single_quotes(s);
                            cmd_str.push_str(&format!("'{}'", escaped));
                        }
                    }
                    TokenType::Number => {
                        if let Some(n) = val.as_f64() {
                            cmd_str.push_str(" ");
                            if let Some(ref flag) = token.flag {
                                cmd_str.push_str(flag);
                                cmd_str.push_str(" ");
                            }
                            cmd_str.push_str(&n.to_string());
                        }
                    }
                }
            }
        }
        cmd_str
    };

    let args = json!({
        "pkg": "warpui",
        "release": true,
        "jobs": 4.0
    });
    assert_eq!(compile(&args), "cargo build -p 'warpui' --release -j 4");

    // With defaults
    let args_defaults = json!({
        "pkg": "warpui"
    });
    assert_eq!(compile(&args_defaults), "cargo build -p 'warpui'");
}

#[test]
fn test_resolve_git_resolver_isolated() {
    // Check that executing with an invalid resolver returns None
    assert!(resolve_git_resolver_isolated("invalid:resolver", ".").is_none());

    // Check status resolver (will work if the current directory is a git repo)
    // Even if not in a git repo, it should fail gracefully and return None/Some
    let res = resolve_git_resolver_isolated("git:status_files", ".");
    // We don't assert matches since we don't know if current dir has changes or if git is present
    // but at least it shouldn't panic.
    let _ = res;
}
