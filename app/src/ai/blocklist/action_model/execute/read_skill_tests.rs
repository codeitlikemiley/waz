use super::*;
use crate::ai::agent::task::TaskId;
use crate::ai::agent::AIAgentActionResultType;
use crate::ai::agent::ReadSkillRequest;
use crate::ai::agent::ReadSkillResult;
use crate::ai::agent::{AIAgentAction, AIAgentActionId, AIAgentActionType};
use crate::ai::blocklist::action_model::AIConversationId;
use crate::ai::skills::SkillManager;
use crate::warp_managed_paths_watcher::WarpManagedPathsWatcher;
use ai::agent::action_result::AnyFileContent;
use ai::skills::{parse_skill, SkillReference};
use repo_metadata::{
    repositories::DetectedRepositories, watcher::DirectoryWatcher, RepoMetadataModel,
};
use std::fs;
use std::io::Write;
use tempfile::TempDir;
use warpui::App;
use watcher::HomeDirectoryWatcher;

fn initialize_app(app: &mut App) {
    app.add_singleton_model(DirectoryWatcher::new);
    app.add_singleton_model(|_| DetectedRepositories::default());
    app.add_singleton_model(RepoMetadataModel::new);
    app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
    app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
    app.add_singleton_model(SkillManager::new);
}

fn create_test_skill_file(dir: &TempDir, name: &str, description: &str) -> std::path::PathBuf {
    let skill_content = format!(
        r#"---
name: {}
description: {}
---

# {}

## Instructions
Test instructions for this skill.

## Examples
Example usage of the skill.
"#,
        name, description, name
    );

    let skill_dir = dir.path().join(format!(".claude/skills/{}", name));
    fs::create_dir_all(&skill_dir).unwrap();
    let skill_path = skill_dir.join("SKILL.md");
    let mut file = fs::File::create(&skill_path).unwrap();
    file.write_all(skill_content.as_bytes()).unwrap();
    file.flush().unwrap();

    skill_path
}

#[test]
fn test_read_skill_executor_success() {
    let temp_dir = TempDir::new().unwrap();
    let skill_path = create_test_skill_file(&temp_dir, "test-skill", "A test skill");

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Populate SkillManager cache with the test skill
        let parsed_skill = parse_skill(&skill_path).expect("Failed to parse test skill");
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_skill_for_testing(parsed_skill);
        });

        let executor_handle = app.add_model(|_| ReadSkillExecutor::new());

        let action = AIAgentAction {
            id: AIAgentActionId::from("test-action-id".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(skill_path.clone()),
            }),
            task_id: TaskId::new("test-task-id".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();

            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Success { content },
                )) => {
                    assert_eq!(content.file_name, skill_path.to_string_lossy().to_string());
                }
                _ => panic!("Successfully read skill file; should return ReadSkillResult::Success"),
            }
        });
    });
}

#[test]
fn test_read_skill_executor_file_not_found() {
    let temp_dir = TempDir::new().unwrap();
    // Don't create the SKILL.md file
    let skill_path = temp_dir.path().join("SKILL.md");

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let executor_handle = app.add_model(|_| ReadSkillExecutor::new());

        let action = AIAgentAction {
            id: AIAgentActionId::from("test-action-id".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(skill_path),
            }),
            task_id: TaskId::new("test-task-id".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();

            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Error(error_msg),
                )) => {
                    // Should contain an error about file not found or I/O error
                    assert!(!error_msg.is_empty());
                }
                _ => panic!(
                    "Nonexistent SKILL.md file at given path; should return ReadSkillResult::Error"
                ),
            }
        });
    });
}

/// Issue #99 Fallback: on cache miss, if SkillReference::Path points to a valid-shaped skill file,
/// directly read the disk and return successfully (taking the Async branch).
#[test]
fn test_read_skill_executor_fallback_reads_disk_on_cache_miss() {
    let temp_dir = TempDir::new().unwrap();
    let skill_path = create_test_skill_file(&temp_dir, "fallback-skill", "Read from disk");

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        // Note: Do not call add_skill_for_testing to simulate a cache miss.
        let executor_handle = app.add_model(|_| ReadSkillExecutor::new());

        let action = AIAgentAction {
            id: AIAgentActionId::from("fallback-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(skill_path.clone()),
            }),
            task_id: TaskId::new("fallback-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        let execution = executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!("Cache miss with valid skill path should produce Async execution");
        };

        let async_result = execute_future.await;
        let result = app.update(|ctx| on_complete(async_result, ctx));

        match result {
            AIAgentActionResultType::ReadSkill(ReadSkillResult::Success { content }) => {
                assert_eq!(content.file_name, skill_path.to_string_lossy().to_string());
                let body = match &content.content {
                    AnyFileContent::StringContent(s) => s.clone(),
                    AnyFileContent::BinaryContent(_) => {
                        panic!("SKILL.md should be parsed as text")
                    }
                };
                assert!(body.contains("fallback-skill"));
            }
            other => panic!("Fallback should return Success, got: {other:?}"),
        }
    });
}

/// Issue #99 Fallback Failure Path: on cache miss, if the path shape is valid but the file does not exist on disk
/// (e.g., a race condition where it is deleted after validation), parse_skill in the Async branch fails and on_complete should return an Error.
#[test]
fn test_read_skill_executor_fallback_returns_error_when_file_missing() {
    let temp_dir = TempDir::new().unwrap();
    // The path shape is valid, but SKILL.md was never created.
    let skill_path = temp_dir
        .path()
        .join(".agents/skills/missing-skill/SKILL.md");

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let executor_handle = app.add_model(|_| ReadSkillExecutor::new());

        let action = AIAgentAction {
            id: AIAgentActionId::from("missing-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(skill_path),
            }),
            task_id: TaskId::new("missing-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        let execution = executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!("Legal-shaped skill path should still produce Async execution before disk check");
        };

        let async_result = execute_future.await;
        let result = app.update(|ctx| on_complete(async_result, ctx));

        match result {
            AIAgentActionResultType::ReadSkill(ReadSkillResult::Error(msg)) => {
                assert!(msg.starts_with("Skill not found"));
            }
            other => panic!("Missing file should resolve to Error, got: {other:?}"),
        }
    });
}

/// When the BYOP `read_skill` tool is called with a name:
/// `from_args` packs the name into `SkillReference::SkillPath(name)`,
/// and the executor successfully resolves it by name on cache miss and returns Sync Success.
#[test]
fn test_read_skill_executor_resolves_by_name() {
    let temp_dir = TempDir::new().unwrap();
    let skill_path = create_test_skill_file(&temp_dir, "byop-named-skill", "Lookup by name");

    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let parsed_skill = parse_skill(&skill_path).expect("Failed to parse test skill");
        SkillManager::handle(&app).update(&mut app, |manager, _ctx| {
            manager.add_skill_for_testing(parsed_skill);
        });

        let executor_handle = app.add_model(|_| ReadSkillExecutor::new());

        // Simulate BYOP from_args: pass the name as a path.
        let action = AIAgentAction {
            id: AIAgentActionId::from("name-lookup-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(std::path::PathBuf::from("byop-named-skill")),
            }),
            task_id: TaskId::new("name-lookup-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Success { content },
                )) => {
                    assert_eq!(content.file_name, skill_path.to_string_lossy().to_string());
                }
                _ => panic!("Lookup by name should succeed via Sync Success"),
            }
        });
    });
}

/// For an unknown name (not in the SkillManager index) after all fallbacks:
/// `name_candidate` hits but `find_skill_by_name` returns None, continuing to the fs fallback -
/// since the path shape is invalid here (a plain name containing no `/`), it returns Sync Error directly.
#[test]
fn test_read_skill_executor_rejects_unknown_name() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let executor_handle = app.add_model(|_| ReadSkillExecutor::new());

        let action = AIAgentAction {
            id: AIAgentActionId::from("unknown-name-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(std::path::PathBuf::from("no-such-skill")),
            }),
            task_id: TaskId::new("unknown-name-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Error(msg),
                )) => {
                    assert!(msg.starts_with("Skill not found"), "msg={msg}");
                }
                _ => panic!("Unknown name should resolve to Sync Error"),
            }
        });
    });
}

/// Issue #99 Safety Gate: on cache miss, if the path does not match the skill file shape,
/// it directly takes the Sync Error branch without triggering any disk read.
#[test]
fn test_read_skill_executor_rejects_non_skill_path_on_cache_miss() {
    let temp_dir = TempDir::new().unwrap();
    // A random markdown file that is not structured under `.<provider>/skills/<name>/SKILL.md`.
    // Even if this file exists, the fallback should not read it - extract_skill_parent_directory will reject it.
    let non_skill_path = temp_dir.path().join("random.md");
    fs::write(&non_skill_path, "not a skill").unwrap();

    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let executor_handle = app.add_model(|_| ReadSkillExecutor::new());

        let action = AIAgentAction {
            id: AIAgentActionId::from("non-skill-action".to_string()),
            action: AIAgentActionType::ReadSkill(ReadSkillRequest {
                skill: SkillReference::Path(non_skill_path),
            }),
            task_id: TaskId::new("non-skill-task".to_string()),
            requires_result: false,
        };

        let input = ExecuteActionInput {
            action: &action,
            conversation_id: AIConversationId::new(),
        };

        executor_handle.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            match result {
                AnyActionExecution::Sync(AIAgentActionResultType::ReadSkill(
                    ReadSkillResult::Error(msg),
                )) => {
                    assert!(msg.starts_with("Skill not found"));
                }
                _ => panic!(
                    "Non-skill path on cache miss should return Sync Error, not Async fallback"
                ),
            }
        });
    });
}
