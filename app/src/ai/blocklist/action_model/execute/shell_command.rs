use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use futures::{select, FutureExt};
use futures_lite::pin;
use itertools::Itertools;
use parking_lot::FairMutex;
use warp_core::command::ExitCode;
use warp_core::execution_mode::AppExecutionMode;
use warp_util::path::ShellFamily;
use warpui::r#async::{Spawnable, Timer};
use warpui::{Entity, EntityId, ModelContext, ModelHandle, SingletonEntity};

use crate::ai::agent::{
    AIAgentActionId, AIAgentActionType, AIAgentPtyWriteMode, ReadShellCommandOutputResult,
    RequestCommandOutputResult, ShellCommandDelay, ShellCommandError,
    TransferShellCommandControlToUserResult, WriteToLongRunningShellCommandResult,
};
use crate::ai::blocklist::permissions::CommandExecutionPermission;
use crate::ai::blocklist::BlocklistAIPermissions;
use crate::ai::execution_profiles::WriteToPtyPermission;
use crate::terminal::event::BlockMetadataReceivedEvent;
use crate::terminal::model::block::{
    formatted_terminal_contents_for_input, Block, BlockId, CURSOR_MARKER,
};
use crate::terminal::shell::ShellType;
use crate::terminal::ssh::util::parse_interactive_ssh_command;
use crate::{
    ai::agent::AIAgentActionResultType,
    terminal::{
        model::session::active_session::ActiveSession,
        model_events::{ModelEvent, ModelEventDispatcher},
        TerminalModel,
    },
};
use crate::{send_telemetry_from_ctx, TelemetryEvent};

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};

/// Text returned to the agent for `run_shell_command` / related tools.
///
/// Prefer unobfuscated grid text; some shells / timing paths leave the primary serialization empty
/// while `output_to_string()` (displayed-output path) or the force-full path still has bytes.
///
/// Fallback: when output grids are empty (e.g. missing preexec / early-complete timing), extract
/// the stdout portion from the command grid (everything after the first line).
fn agent_shell_command_block_output(block: &Block) -> String {
    let primary = block.output_with_secrets_unobfuscated();
    if !primary.trim().is_empty() {
        return primary;
    }

    let displayed = block.output_to_string();
    if !displayed.trim().is_empty() {
        return displayed;
    }

    let forced = block.output_to_string_force_full_grid_contents();
    if !forced.trim().is_empty() {
        return forced;
    }

    let command_grid = block.command_with_secrets_unobfuscated(false);
    command_grid
        .split_once('\n')
        .map(|(_, output)| output.to_owned())
        .filter(|output| !output.trim().is_empty())
        .unwrap_or_default()
}

pub struct ShellCommandExecutor {
    active_session: ModelHandle<ActiveSession>,
    block_finished_senders: HashMap<BlockSelector, oneshot::Sender<()>>,
    /// Senders used by the `Check now` affordance to force a long-running shell command's
    /// pending poll future to resolve immediately with a fresh snapshot, bypassing the
    /// agent-set timeout.
    force_refresh_senders: HashMap<BlockSelector, oneshot::Sender<()>>,
    terminal_model: Arc<FairMutex<TerminalModel>>,
    terminal_view_id: EntityId,
    /// Sender to notify when user hands control back to agent after TransferShellCommandControlToUser.
    control_handback_sender: Option<oneshot::Sender<()>>,
}

impl ShellCommandExecutor {
    pub const MAX_WAIT_DURATION: Duration = Duration::from_secs(2);
    /// Maximum delay we will honor for any agent-requested wait. Applies both  
    /// to finite `ShellCommandDelay::Duration` requests and to  
    /// `ShellCommandDelay::OnCompletion`, which would otherwise wait indefinitely.  
    pub const MAX_AGENT_DELAY_DURATION: Duration = Duration::from_secs(120);
    /// "Pager hang defense": The final fallback timeout for the `wait_until_completion=true`
    /// (`ActionResultDelay::UntilCompletion`) path, used solely to prevent the agent from hanging
    /// indefinitely if `turn_off_pager_for_command` is bypassed by the user's shell configuration
    /// (e.g., `export PAGER=less` in `~/.zshrc`, `git config --global core.pager less`, etc.).
    ///
    /// This is **not** a general command timeout: 30 minutes is intentionally much longer than
    /// `MAX_AGENT_DELAY_DURATION` to avoid interrupting valid long tasks like `cargo build --release`,
    /// `docker build`, or large `npm install` runs.
    /// When triggered, it flags the snapshot as preempted via `is_preempted=true` rather than "command completed".
    pub const MAX_UNTIL_COMPLETION_DURATION: Duration = Duration::from_secs(30 * 60);

    pub fn new(
        active_session: ModelHandle<ActiveSession>,
        terminal_model: Arc<FairMutex<TerminalModel>>,
        model_event_dispatcher: &ModelHandle<ModelEventDispatcher>,
        terminal_view_id: EntityId,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(model_event_dispatcher, Self::handle_terminal_model_event);

        Self {
            active_session,
            terminal_model,
            block_finished_senders: HashMap::new(),
            force_refresh_senders: HashMap::new(),
            terminal_view_id,
            control_handback_sender: None,
        }
    }

    fn handle_terminal_model_event(&mut self, event: &ModelEvent, _ctx: &mut ModelContext<Self>) {
        // We wait for precmd for the block _after_ the requested command's block so that
        // downstream checks for current working directory are fresh. The precmd hook is when
        // the shell relays current working directory to warp.
        if let ModelEvent::BlockMetadataReceived(BlockMetadataReceivedEvent { .. }) = event {
            let model = self.terminal_model.lock();
            let block_finished_senders = self.block_finished_senders.drain().collect_vec();
            for (block_selector, block_finished_tx) in block_finished_senders.into_iter() {
                if let Some(block) = block_selector.get_block(&model) {
                    if block.is_command_finished() {
                        if let Err(e) = block_finished_tx.send(()) {
                            log::warn!(
                                "Failed to notify block completion for running requested command: {e:?}"
                            )
                        }
                    } else {
                        self.block_finished_senders
                            .insert(block_selector, block_finished_tx);
                    }
                }
            }
        }
    }

    pub(super) fn should_autoexecute(
        &self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let blocklist_permissions = BlocklistAIPermissions::as_ref(ctx);
        match &input.action.action {
            AIAgentActionType::RequestCommandOutput {
                command,
                is_read_only,
                is_risky,
                ..
            } => {
                let Some(escape_char) = self
                    .active_session
                    .as_ref(ctx)
                    .shell_type(ctx)
                    .map(|s| ShellFamily::from(s).escape_char())
                else {
                    return false;
                };
                let autoexecution_permission = blocklist_permissions.can_autoexecute_command(
                    &input.conversation_id,
                    command,
                    escape_char,
                    is_read_only.unwrap_or(false),
                    *is_risky,
                    Some(self.terminal_view_id),
                    ctx,
                );
                if let CommandExecutionPermission::Allowed(reason) = autoexecution_permission {
                    send_telemetry_from_ctx!(
                        TelemetryEvent::AutoexecutedAgentModeRequestedCommand { reason },
                        ctx
                    );
                } else if let CommandExecutionPermission::Denied(reason) = autoexecution_permission
                {
                    if AppExecutionMode::as_ref(ctx).is_autonomous() {
                        log::warn!(
                            "Command denied during autonomous execution, reason: {reason:?}"
                        );
                    }
                }
                autoexecution_permission.is_allowed()
            }
            AIAgentActionType::WriteToLongRunningShellCommand { block_id, .. } => {
                let terminal_model = self.terminal_model.lock();
                let block = terminal_model.block_list().block_with_id(block_id);

                if block.is_none_or(|block| block.finished()) {
                    // If the block is already finished, allow auto-execution - the finished output
                    // will be returned.
                    true
                } else {
                    let should_autoexecute = match blocklist_permissions.can_write_to_pty(
                        &input.conversation_id,
                        Some(self.terminal_view_id),
                        ctx,
                    ) {
                        WriteToPtyPermission::AlwaysAllow => true,
                        WriteToPtyPermission::AskOnFirstWrite => terminal_model
                            .block_list()
                            .active_block()
                            .has_agent_written_to_block(),
                        _ => false,
                    };

                    if should_autoexecute {
                        send_telemetry_from_ctx!(
                            TelemetryEvent::CLISubagentActionExecuted {
                                conversation_id: input.conversation_id,
                                block_id: block_id.clone(),
                                is_autoexecuted: true,
                            },
                            ctx
                        );
                    }

                    should_autoexecute
                }
            }
            AIAgentActionType::ReadShellCommandOutput { .. } => true,
            AIAgentActionType::TransferShellCommandControlToUser { .. } => false,
            _ => false,
        }
    }

    /// Wrap the command with a set of common pager environment variables to prevent the command from invoking a pager while **preserving the true exit code**.
    ///
    /// The previous implementation was `(cmd) | cat`. Although this prevents stdout from being a tty (thus git/man/less etc. do not invoke a pager),
    /// `$?` in bash/zsh would be overwritten by `cat`'s exit code (almost always 0), causing the agent to see `exit_code=0` even when `cargo check`
    /// fails, leading to incorrect decisions.
    ///
    /// Here, we instead use `PAGER=cat GIT_PAGER=cat MANPAGER=cat` and execute it in a subshell/script block.
    /// This suppresses the pager behavior for the vast majority of CLIs (git, man, bat, kubectl, psql, gh, etc.) while allowing the outer `$?` or
    /// `$LASTEXITCODE` to be taken directly from the command itself.
    ///
    /// **Two hardening rules** (working alongside the fact that `ActionResultDelay::UntilCompletion` has no short timeout, see #138):
    /// 1. Perform `unset` before `export` (using the equivalent syntax of the shell) to clear any inherited `PAGER=less` etc.
    ///    exported from the user's `~/.zshrc` / `~/.bashrc` before assigning them to `cat`. A simple `export` can still be
    ///    overwritten by a subsequent `.zshenv` in some edge cases.
    /// 2. Inject `GIT_CONFIG_COUNT=1 / GIT_CONFIG_KEY_0=core.pager / GIT_CONFIG_VALUE_0=cat`
    ///    as a double insurance: tests show that in git 2.54, the `GIT_PAGER` env var already takes precedence over
    ///    `git config --global core.pager less` in `~/.gitconfig`, but using git ≥ 2.31's `GIT_CONFIG_COUNT`
    ///    mechanism to stack an in-process config override blocks future git versions modifying precedence or edge cases
    ///    in third-party pager wrappers. This is completely harmless to non-git commands, so we don't need to detect the first token.
    ///
    /// Even if all of the above fail, `action_result_future`'s `MAX_UNTIL_COMPLETION_DURATION` fallback
    /// ensures that the agent will not hang **permanently**.
    fn turn_off_pager_for_command(&self, command: &String, ctx: &mut ModelContext<Self>) -> String {
        match self.active_session.as_ref(ctx).shell_type(ctx) {
            // Export in a subshell, exit code of subshell = exit code of the last command, preserving the real $?.
            // First unset to clean up inherited PAGER/GIT_PAGER/MANPAGER from parent shell, then export=cat.
            Some(ShellType::Zsh) | Some(ShellType::Bash) => format!(
                "(unset PAGER GIT_PAGER MANPAGER; export PAGER=cat GIT_PAGER=cat MANPAGER=cat GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.pager GIT_CONFIG_VALUE_0=cat; {command})"
            ),
            // fish: `set -lx` is a local export within the begin/end block, `$status` captures the last command.
            // Use `set -e` to clean inherited variables, then `set -lx` to assign cat.
            Some(ShellType::Fish) => format!(
                "begin; set -e PAGER; set -e GIT_PAGER; set -e MANPAGER; set -lx PAGER cat; set -lx GIT_PAGER cat; set -lx MANPAGER cat; set -lx GIT_CONFIG_COUNT 1; set -lx GIT_CONFIG_KEY_0 core.pager; set -lx GIT_CONFIG_VALUE_0 cat; {command}; end"
            ),
            // pwsh: script block local $env: does not pollute outer session, $LASTEXITCODE propagates.
            // Remove-Item Env: cleans inherited values, then assign cat; use -ErrorAction SilentlyContinue for non-existent variables.
            Some(ShellType::PowerShell) => format!(
                "& {{ Remove-Item Env:PAGER -ErrorAction SilentlyContinue; Remove-Item Env:GIT_PAGER -ErrorAction SilentlyContinue; Remove-Item Env:MANPAGER -ErrorAction SilentlyContinue; $env:PAGER='cat'; $env:GIT_PAGER='cat'; $env:MANPAGER='cat'; $env:GIT_CONFIG_COUNT='1'; $env:GIT_CONFIG_KEY_0='core.pager'; $env:GIT_CONFIG_VALUE_0='cat'; {command} }}"
            ),
            // Unknown shell cannot be safely decorated, bypass directly - pager suppression is completely invalid in this path,
            // and we rely solely on MAX_UNTIL_COMPLETION_DURATION fallback timeout to avoid hanging permanently.
            None => command.clone(),
        }
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> {
        let model = self.terminal_model.lock();

        // Determine the action we want to take based on the input.
        let action_id = input.action.id.clone();

        let handle = ctx.handle();
        match &input.action.action {
            AIAgentActionType::RequestCommandOutput {
                command,
                uses_pager,
                wait_until_completion,
                ..
            } => {
                if model
                    .block_list()
                    .active_block()
                    .is_active_and_long_running()
                {
                    // If there is an active block, we can't execute another command.
                    return ActionExecution::Sync(AIAgentActionResultType::RequestCommandOutput(
                        RequestCommandOutputResult::CancelledBeforeExecution,
                    ));
                }
                // Waz: Synchronous wait commands (wait_until_completion=true) unconditionally disable the pager.
                //
                // The model-reported `uses_pager` is unreliable - smaller models like deepseek-v4-flash rarely set it actively.
                // Once it hits an implicit pager like `git diff`/`git log`/`man`, it gets stuck at the less prompt.
                // Warp degrades the command and returns a LongRunningCommandSnapshot, but the agent does not understand this contract transition
                // and continues to emit new tool calls in parallel, causing both PTY and UI to lock up (input box disappears).
                //
                // Core logic: since the agent explicitly requested "wait until completion", a pager prompt violates this contract.
                // Warp must ensure the pager is never triggered, rather than relying on the model to predict the paging behavior of each CLI.
                //
                // This does not affect the explicit asynchronous path (wait_until_completion=false); command like `tail -f` or dev servers
                // that are genuinely long-running will still go through the original LongRunningCommandSnapshot path.
                let _ = uses_pager; // Field retained for API compatibility, but semantics are no longer dependent on it
                let decorated_command = if *wait_until_completion {
                    self.turn_off_pager_for_command(command, ctx)
                } else {
                    command.clone()
                };
                ctx.emit(ShellCommandExecutorEvent::ExecuteCommand {
                    action_id: action_id.clone(),
                    command: decorated_command,
                });

                let block_selector = BlockSelector::RequestedCommandId(action_id.clone());
                let command = command.clone();
                drop(model);

                ActionExecution::new_async(
                    self.action_result_future(
                        block_selector.clone(),
                        action_result_delay_for_requested_command(*wait_until_completion),
                    ),
                    move |result, ctx| {
                        // Remove the senders from the maps.
                        if let Some(handle) = handle.upgrade(ctx) {
                            handle.update(ctx, |me, _| {
                                me.block_finished_senders.remove(&block_selector);
                                me.force_refresh_senders.remove(&block_selector);
                            });
                        }

                        action_result_for_requested_command(command, result)
                    },
                )
            }
            AIAgentActionType::WriteToLongRunningShellCommand {
                block_id,
                input,
                mode,
            } => {
                let Some(block) = model.block_list().block_with_id(block_id) else {
                    return ActionExecution::Sync(
                        AIAgentActionResultType::WriteToLongRunningShellCommand(
                            WriteToLongRunningShellCommandResult::Error(
                                ShellCommandError::BlockNotFound,
                            ),
                        ),
                    );
                };
                if block.finished() {
                    let output: String = agent_shell_command_block_output(block);
                    let exit_code = block.exit_code();
                    return ActionExecution::Sync(
                        AIAgentActionResultType::WriteToLongRunningShellCommand(
                            WriteToLongRunningShellCommandResult::CommandFinished {
                                block_id: block.id().clone(),
                                output,
                                exit_code,
                            },
                        ),
                    );
                }
                // Drop immutable borrow.
                drop(model);

                let mut model = self.terminal_model.lock();
                if let Some(block) = model.block_list_mut().mut_block_from_id(block_id) {
                    block.mark_agent_written_to_block();
                }
                drop(model);

                ctx.emit(ShellCommandExecutorEvent::WriteToPty {
                    input: input.clone(),
                    mode: *mode,
                });

                let block_selector = BlockSelector::Id(block_id.clone());
                ActionExecution::new_async(
                    self.action_result_future(
                        block_selector.clone(),
                        ActionResultDelay::Duration(Duration::from_millis(200)),
                    ),
                    move |result, ctx| {
                        // Remove the senders from the maps.
                        if let Some(handle) = handle.upgrade(ctx) {
                            handle.update(ctx, |me, _| {
                                me.block_finished_senders.remove(&block_selector);
                                me.force_refresh_senders.remove(&block_selector);
                            });
                        }

                        action_result_for_write_to_long_running_shell_command(result)
                    },
                )
            }
            AIAgentActionType::ReadShellCommandOutput { block_id, delay } => {
                let Some(block) = model.block_list().block_with_id(block_id) else {
                    return ActionExecution::Sync(AIAgentActionResultType::ReadShellCommandOutput(
                        ReadShellCommandOutputResult::Error(ShellCommandError::BlockNotFound),
                    ));
                };
                if block.finished() {
                    let command = block.command_with_secrets_unobfuscated(false);
                    let output: String = block.output_with_secrets_unobfuscated();
                    let exit_code = block.exit_code();
                    return ActionExecution::Sync(AIAgentActionResultType::ReadShellCommandOutput(
                        ReadShellCommandOutputResult::CommandFinished {
                            command,
                            block_id: block_id.clone(),
                            output,
                            exit_code,
                        },
                    ));
                }
                let command = block.command_with_secrets_unobfuscated(false);
                // Adjust the wait duration down based on the command content only on the `ReadShellCommandOutput` path:
                // this is a secondary polling from the agent on a block that is **still running**, which by default would wait
                // up to `MAX_AGENT_DELAY_DURATION` (120s) under `OnCompletion`. For interactive sessions that never actively exit,
                // such as ssh / mosh / sftp / telnet, this wait makes no sense.
                // `RequestCommandOutput` (the initial launch) uses the default timeout of `MAX_WAIT_DURATION = 2s`,
                // which naturally does not block for 120s, and thus does not need the same treatment.
                let delay = effective_read_shell_command_delay(&command, delay.clone());
                drop(model);

                let block_selector = BlockSelector::Id(block_id.clone());
                ActionExecution::new_async(
                    self.action_result_future(block_selector.clone(), delay),
                    move |result, ctx| {
                        // Remove the senders from the maps.
                        if let Some(handle) = handle.upgrade(ctx) {
                            handle.update(ctx, |me, _| {
                                me.block_finished_senders.remove(&block_selector);
                                me.force_refresh_senders.remove(&block_selector);
                            });
                        }

                        action_result_for_read_shell_command_output(command.clone(), result)
                    },
                )
            }
            AIAgentActionType::TransferShellCommandControlToUser { reason } => {
                let active_block = model.block_list().active_block();
                if !active_block.is_active_and_long_running() {
                    return ActionExecution::Sync(
                        AIAgentActionResultType::TransferShellCommandControlToUser(
                            TransferShellCommandControlToUserResult::Error(
                                ShellCommandError::BlockNotFound,
                            ),
                        ),
                    );
                }

                let block_id = active_block.id().clone();
                drop(model);

                // Emit event to transfer control to user.
                ctx.emit(ShellCommandExecutorEvent::TransferControlToUser {
                    action_id: action_id.clone(),
                    reason: reason.clone(),
                });

                // Create a channel to wait for control handback.
                let (handback_tx, handback_rx) = oneshot::channel();
                self.control_handback_sender = Some(handback_tx);

                let block_selector = BlockSelector::Id(block_id.clone());

                // Set up a future to also wait for block completion.
                let (block_finished_tx, block_finished_rx) = oneshot::channel();
                self.block_finished_senders
                    .insert(block_selector.clone(), block_finished_tx);

                // Build the future that captures terminal model and block data.
                let transfer_future = {
                    let terminal_model = self.terminal_model.clone();
                    let block_id = block_id.clone();
                    async move {
                        pin!(handback_rx);
                        pin!(block_finished_rx);

                        // Wait for either control handback or block completion.
                        let transfer_result = select! {
                            val = handback_rx => match val {
                                Ok(_) => TransferControlResult::ControlHandedBack,
                                Err(_) => TransferControlResult::Cancelled,
                            },
                            val = block_finished_rx => match val {
                                Ok(_) => TransferControlResult::BlockFinished,
                                Err(_) => TransferControlResult::Cancelled,
                            },
                        };

                        // Convert to ActionResult
                        let model = terminal_model.lock();
                        match transfer_result {
                            TransferControlResult::ControlHandedBack
                            | TransferControlResult::BlockFinished => {
                                match model.block_list().block_with_id(&block_id) {
                                    Some(block) => {
                                        if block.finished() {
                                            ActionResult::CommandFinished {
                                                block_id: block.id().clone(),
                                                output: agent_shell_command_block_output(block),
                                                exit_code: block.exit_code(),
                                            }
                                        } else {
                                            let grid_contents = if model.is_alt_screen_active() {
                                                formatted_terminal_contents_for_input(
                                                    model.alt_screen().grid_handler(),
                                                    None,
                                                    CURSOR_MARKER,
                                                )
                                            } else {
                                                formatted_terminal_contents_for_input(
                                                    block.output_grid().grid_handler(),
                                                    Some(1000),
                                                    CURSOR_MARKER,
                                                )
                                            };
                                            ActionResult::LongRunningCommandSnapshot {
                                                block_id: block.id().clone(),
                                                grid_contents,
                                                cursor: CURSOR_MARKER,
                                                is_alt_screen_active: model.is_alt_screen_active(),
                                                is_preempted: false,
                                            }
                                        }
                                    }
                                    None => ActionResult::BlockNotFound,
                                }
                            }
                            TransferControlResult::Cancelled => ActionResult::Cancelled,
                        }
                    }
                };

                ActionExecution::new_async(transfer_future, move |result, ctx| {
                    // Clean up.
                    if let Some(handle) = handle.upgrade(ctx) {
                        handle.update(ctx, |me, _| {
                            me.block_finished_senders.remove(&block_selector);
                            me.control_handback_sender = None;
                        });
                    }

                    action_result_for_transfer_shell_command_control_to_user(result)
                })
            }
            _ => ActionExecution::InvalidAction,
        }
    }

    /// Called when user hands control back to agent after TransferShellCommandControlToUser.
    pub fn notify_control_handed_back(&mut self) {
        if let Some(sender) = self.control_handback_sender.take() {
            let _ = sender.send(());
        }
    }

    /// Produces a future which resolves when the action is complete and
    /// we have a result to send to the agent.
    fn action_result_future(
        &mut self,
        block_selector: BlockSelector,
        delay: ActionResultDelay,
    ) -> impl Spawnable<Output = ActionResult> {
        // Create a channel to notify us when we receive block metadata.
        let (block_metadata_received_tx, block_metadata_received_rx) = oneshot::channel();
        self.block_finished_senders
            .insert(block_selector.clone(), block_metadata_received_tx);

        // Create a channel so the `Check now` affordance can short-circuit the timeout
        // and deliver the agent a fresh snapshot immediately.
        let (force_refresh_tx, force_refresh_rx) = oneshot::channel();
        self.force_refresh_senders
            .insert(block_selector.clone(), force_refresh_tx);

        // Create a future that resolves when we should send a result to the agent.
        let terminal_model = self.terminal_model.clone();

        async move {
            pin!(block_metadata_received_rx);
            pin!(force_refresh_rx);

            let timeout_duration = match delay {
                ActionResultDelay::UntilCompletion => None,
                ActionResultDelay::Duration(duration) => {
                    // Enforce a maximum allowed delay that the agent may request, never waiting longer than MAX_AGENT_DELAY_DURATION.
                    // If the requested duration exceeds this cap, we'll still behave as if the agent may expect a running command,
                    // so there's no need to signal preemption (the agent already anticipates an incomplete command state).
                    Some(duration.min(Self::MAX_AGENT_DELAY_DURATION))
                }
                ActionResultDelay::OnCompletion { timeout } => {
                    Some(timeout.min(Self::MAX_AGENT_DELAY_DURATION))
                }
                ActionResultDelay::Default => Some(Self::MAX_WAIT_DURATION),
            };

            let wake_reason = if let Some(timeout_duration) = timeout_duration {
                let timeout = Timer::after(timeout_duration).fuse();
                pin!(timeout);
                select! {
                    val = block_metadata_received_rx => match val {
                        Ok(_) => WakeReason::BlockFinished,
                        Err(_) => return ActionResult::Cancelled,
                    },
                    val = force_refresh_rx => match val {
                        // User asked the agent to check now; fall through to the snapshot
                        // code path below. Treated as a preemption (snapshot arrives before
                        // the agent's own timer would have fired).
                        Ok(_) => WakeReason::ForceRefresh,
                        // Sender was dropped (e.g. because the executor is being torn down).
                        Err(_) => return ActionResult::Cancelled,
                    },
                    _ = timeout => WakeReason::Timeout,
                }
            } else {
                // The ActionResultDelay::UntilCompletion path originally had no timeout. Adding `MAX_UNTIL_COMPLETION_DURATION`
                // as a hard fallback prevents the agent from hanging permanently if `turn_off_pager_for_command` is bypassed
                // by the user's shell configuration (see #138). If a timeout triggers, it falls under the `(Timeout, UntilCompletion)`
                // branch of `compute_is_preempted` below, being marked as preempted.
                let hard_timeout = Timer::after(Self::MAX_UNTIL_COMPLETION_DURATION).fuse();
                pin!(hard_timeout);
                select! {
                    val = block_metadata_received_rx => match val {
                        Ok(_) => WakeReason::BlockFinished,
                        Err(_) => return ActionResult::Cancelled,
                    },
                    val = force_refresh_rx => match val {
                        Ok(_) => WakeReason::ForceRefresh,
                        Err(_) => return ActionResult::Cancelled,
                    },
                    _ = hard_timeout => WakeReason::Timeout,
                }
            };

            // Mark the snapshot as preempted if woken early, allowing the server to distinguish
            // true completion from a forced client poll (`ForceRefresh`), a timeout during
            // `on_completion`, or the `UntilCompletion` pager-hang safety-net timeout.
            //
            // Note: `RequestCommandOutputResult::LongRunningCommandSnapshot` currently does not have an `is_preempted`
            // field (unlike `ReadShellCommandOutputResult` and `TransferShellCommandControlToUserResult`).
            // This flag will be discarded on the `RequestCommandOutput` path by the `..` of `action_result_for_requested_command`;
            // we still assign it correctly here so it automatically takes effect once the field is added in the future.
            let is_preempted = compute_is_preempted(wake_reason, delay);

            // At this point, we've either received block metadata or we've timed out.
            // Check the current state of the block and produce a result accordingly.
            let model = terminal_model.lock();
            let result = match block_selector.get_block(&model) {
                Some(block) => {
                    if block.finished() {
                        ActionResult::CommandFinished {
                            block_id: block.id().clone(),
                            output: agent_shell_command_block_output(block),
                            exit_code: block.exit_code(),
                        }
                    } else {
                        let grid_contents = if model.is_alt_screen_active() {
                            formatted_terminal_contents_for_input(
                                model.alt_screen().grid_handler(),
                                None,
                                CURSOR_MARKER,
                            )
                        } else {
                            formatted_terminal_contents_for_input(
                                block.output_grid().grid_handler(),
                                // TODO(vorporeal): This is probably too large.
                                Some(1000),
                                CURSOR_MARKER,
                            )
                        };
                        ActionResult::LongRunningCommandSnapshot {
                            block_id: block.id().clone(),
                            grid_contents,
                            cursor: CURSOR_MARKER,
                            is_alt_screen_active: model.is_alt_screen_active(),
                            is_preempted,
                        }
                    }
                }
                None => ActionResult::BlockNotFound,
            };

            result
        }
    }

    pub(super) fn cancel_execution(&mut self, id: &AIAgentActionId, _ctx: &mut ModelContext<Self>) {
        // RequestedCommand path uses action id as selector, unconditionally clean up.
        // Cannot rely on the `is_active_and_long_running()` guard: ~50ms after the command is spawned
        // (LONG_RUNNING_COMMAND_DURATION_MS), the guard is false within this window, which leads to leftover senders,
        // causing the detached future to hang until the command actually finishes before exiting (especially affecting
        // wait_until_completion=true, i.e., ActionResultDelay::UntilCompletion).
        let requested_selector = BlockSelector::RequestedCommandId(id.clone());
        self.block_finished_senders.remove(&requested_selector);
        self.force_refresh_senders.remove(&requested_selector);

        // No longer use `BlockSelector::Id(active_block.id())` for fallback cleanup. Senders for `WriteToLRC` /
        // `ReadShellCommandOutput` / `TransferShellCommandControlToUser` have keys that originate from the
        // `block_id` in the action arguments or the `active_block` at creation time, which does not reliably
        // correspond to the `active_block` at the time of cancellation: if the user switches the active block
        // after the action is spawned, the old active-block fallback won't match; if they don't switch, the
        // cleanup is only 'correct by coincidence'. Their senders are cleaned up by their respective
        // `on_complete` callbacks when the futures naturally finish; immediate cleanup would require introducing
        // an `action_id -> BlockSelector` reverse index, which is an independent change outside this issue.
    }

    /// Force any in-flight poll for the given long-running command block to resolve
    /// immediately with a fresh snapshot, bypassing the agent-set timeout.
    ///
    /// Called by the `Check now` affordance in the warping indicator. No-ops if there
    /// is no matching in-flight poll (e.g. because the block already finished or the
    /// agent has transferred control to the user).
    pub fn force_refresh_block(&mut self, block_id: &BlockId) {
        let terminal_model = self.terminal_model.lock();
        // Find a sender whose selector resolves to this block. In practice there is at
        // most one: a given block can have at most one in-flight `action_result_future`
        // at a time.
        let matching_selector = self
            .force_refresh_senders
            .keys()
            .find(|selector| {
                selector
                    .get_block(&terminal_model)
                    .is_some_and(|block| block.id() == block_id)
            })
            .cloned();
        drop(terminal_model);

        if let Some(selector) = matching_selector {
            if let Some(sender) = self.force_refresh_senders.remove(&selector) {
                let _ = sender.send(());
            }
        }
    }

    pub(super) fn preprocess_action(
        &mut self,
        _action: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }
}

/// Waiting strategy used internally by `action_result_future`.
///
/// Compared to the external `Option<ShellCommandDelay>`, here the `OnCompletion` timeout is
/// promoted from an implicit constant to an explicit field, making it easier to dynamically
/// adjust based on the command scenario (see `effective_read_shell_command_delay`).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ActionResultDelay {
    UntilCompletion,
    Default,
    Duration(Duration),
    OnCompletion { timeout: Duration },
}

impl ActionResultDelay {
    fn from_shell_command_delay(delay: Option<ShellCommandDelay>) -> Self {
        match delay {
            Some(ShellCommandDelay::Duration(duration)) => Self::Duration(duration),
            Some(ShellCommandDelay::OnCompletion) => Self::OnCompletion {
                timeout: ShellCommandExecutor::MAX_AGENT_DELAY_DURATION,
            },
            None => Self::Default,
        }
    }
}

/// The reason for deciding the value of `is_preempted` in `action_result_future`.
/// Lifted to module scope so that `compute_is_preempted` can be invoked by unit tests in the same module.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum WakeReason {
    BlockFinished,
    Timeout,
    /// User clicked `Check now` in the warping indicator, short-circuiting  
    /// the agent-set poll timer. Treated as a preemption so the server does  
    /// not interpret the early snapshot as a completion.  
    ForceRefresh,
}

/// Determine if a snapshot should be marked as preempted (`is_preempted=true`). Extracted into a pure function
/// so that table-driven logic correctness can be verified by unit tests (avoiding the need to mock clocks in an async `select!`).
///
/// Preemption semantics: the server treats the snapshot as "a quick peek ahead" rather than "command completion". Satisfied by any of:
/// - `ForceRefresh` (manually triggered by user Check now)
/// - `Timeout` and delay is `OnCompletion` (exceeded the agent's specified on-completion timeout)
/// - `Timeout` and delay is `UntilCompletion` (hit the pager hang fallback timeout, see #138)
fn compute_is_preempted(wake: WakeReason, delay: ActionResultDelay) -> bool {
    matches!(wake, WakeReason::ForceRefresh)
        || matches!(
            (wake, delay),
            (WakeReason::Timeout, ActionResultDelay::OnCompletion { .. })
                | (WakeReason::Timeout, ActionResultDelay::UntilCompletion)
        )
}

fn action_result_delay_for_requested_command(wait_until_completion: bool) -> ActionResultDelay {
    if wait_until_completion {
        ActionResultDelay::UntilCompletion
    } else {
        ActionResultDelay::Default
    }
}

/// Maps the agent-requested `ShellCommandDelay` to the internal `ActionResultDelay`,
/// with special-casing for interactive sessions that **never actively exit** (ssh / mosh / sftp / telnet, etc.):
///
/// 1. `Some(OnCompletion)` - shortens the timeout from `MAX_AGENT_DELAY_DURATION` (120s)
///    to `MAX_WAIT_DURATION` (2s) to avoid the agent waiting indefinitely on a non-terminating command.
/// 2. `None` (default) - actively **upgrades** to `OnCompletion { 2s }` rather than retaining
///    `Default`. Note that this synchronously changes the value of `is_preempted` inside `action_result_future`:
///    `Default` + `Timeout` is not considered a preemption, while `OnCompletion` + `Timeout` will be
///    flagged as a preemption, making the server interpret this snapshot as "a quick peek" rather than "command completion".
///    This is the correct semantics for interactive sessions.
/// 3. `Some(Duration(d))` - preserves the agent's explicit request without rewriting it.
///
/// Non-interactive commands always use the original mapping of `from_shell_command_delay`.
fn effective_read_shell_command_delay(
    command: &str,
    delay: Option<ShellCommandDelay>,
) -> ActionResultDelay {
    if command_starts_non_terminating_session(command)
        && matches!(delay, None | Some(ShellCommandDelay::OnCompletion))
    {
        return ActionResultDelay::OnCompletion {
            timeout: ShellCommandExecutor::MAX_WAIT_DURATION,
        };
    }

    ActionResultDelay::from_shell_command_delay(delay)
}

/// Determine if `command` will start an interactive session that **never actively exits**. Matches if:
/// - A command wrapped by the Waz generator wrapper (recursively checks internal command).
/// - A bare `ssh ...` (processed via `parse_interactive_ssh_command`, which correctly filters out non-interactive options like `-T` / `-W`).
/// - An ssh with a path or `.exe` suffix (rewritten to a bare `ssh` before checking).
/// - `mosh` / `sftp` / `telnet` (including `.exe`) which lack non-interactive flags like ssh, matched directly by executable name.
///
/// A heuristic detection used only by `effective_read_shell_command_delay`. The consequences of a false positive are limited.
fn command_starts_non_terminating_session(command: &str) -> bool {
    let command = command.trim_start();
    in_band_generator_command(command)
        .as_deref()
        .is_some_and(command_starts_non_terminating_session)
        || parse_interactive_ssh_command(command).is_some()
        || normalized_ssh_command(command)
            .as_deref()
            .is_some_and(|command| parse_interactive_ssh_command(command).is_some())
        || first_executable_name(command).is_some_and(|name| {
            matches!(
                name.as_str(),
                "mosh" | "mosh.exe" | "sftp" | "sftp.exe" | "telnet" | "telnet.exe"
            )
        })
}

/// Unwraps Waz's own generator wrapper to extract the actual command to be run.
///
/// The wrapper protocol is of the form: `<wrapper> <generator_id> '<inner_command>' [extra flags...]`
/// Where:
/// - `<wrapper>` is `warp_run_generator_command` (POSIX shell) or
///   `Waz-Run-GeneratorCommand` (PowerShell, case-insensitive).
/// - `<generator_id>` is a numeric id, skipped here without parsing.
/// - `<inner_command>` is the actual command string enclosed in single quotes, which is what we want to return.
///
/// The protocol extracts `tokens[2]` strictly by index; if a subsequent wrapper change adds optional parameters
/// that break this index assumption, it will silently fail to match (returning None), which in the worst case
/// simply falls back to the old 120s wait without introducing buggy behavior.
fn in_band_generator_command(command: &str) -> Option<String> {
    let tokens = shell_words::split(command.trim_start()).ok()?;
    if tokens.len() >= 3
        && (tokens[0].eq_ignore_ascii_case("Waz-Run-GeneratorCommand")
            || tokens[0] == "warp_run_generator_command")
    {
        Some(tokens[2].clone())
    } else {
        None
    }
}

/// When the command's executable entry is ssh with a path or a `.exe` suffix, rewrite it to a bare `ssh`
/// to reuse `parse_interactive_ssh_command` which only accepts the `^ssh\s+...` pattern.
///
/// For example, `"C:\Windows\System32\OpenSSH\ssh.exe" host -p 22` will be rewritten as
/// `ssh host -p 22`. The rest of the arguments are preserved as-is (`rest` is the remaining string
/// after `first_executable_token` splits the first token).
///
/// Only "prefix rewriting" is performed on the name; it does not normalize backslashes/quotes in paths or expand escapes.
fn normalized_ssh_command(command: &str) -> Option<String> {
    let (token, rest) = first_executable_token(command)?;
    let name = command_basename(token);
    if name.eq_ignore_ascii_case("ssh") || name.eq_ignore_ascii_case("ssh.exe") {
        Some(format!("ssh{rest}"))
    } else {
        None
    }
}

fn first_executable_name(command: &str) -> Option<String> {
    let (token, _) = first_executable_token(command)?;
    Some(command_basename(token).to_ascii_lowercase())
}

/// Returns the actual "executable entry" token of the command, skipping common invocation prefixes:
/// - PowerShell invocation operator `&` (must be a standalone token, e.g., `& "C:\...\ssh.exe" host`).
/// - POSIX `command` builtin (`command ssh host`), used to bypass alias/function definitions.
///
/// Only strips one level of prefix, which is sufficient for practical scenarios; does not support `&&` chains, `call`, `exec`, or other forms.
fn first_executable_token(command: &str) -> Option<(&str, &str)> {
    let (token, rest) = first_command_token(command)?;
    if token == "&" || token.eq_ignore_ascii_case("command") {
        first_command_token(rest)
    } else {
        Some((token, rest))
    }
}

/// Heuristic tokenization: extracts the first token of a command and the remaining raw string after it.
///
/// Intentionally does **not** use `shell_words::split` for two reasons:
/// 1. `shell_words` fails directly on non-POSIX characters like the PowerShell call operator `&`,
///    which we need to recognize.
/// 2. We only need the "first token + rest" two parts instead of the full token stream, so a manual implementation is more straightforward.
///
/// Quote handling only recognizes `"` or `'` at the **start position** of the string, and does not handle escapes.
/// To prevent inputs like `"foo"bar` or `"ssh"hello-world` (where text is stuck to the closing quote) from being
/// split into `foo` or `ssh` and triggering a **false positive** (detecting a normal command as an interactive session),
/// we require that the closing quote must be followed immediately by whitespace or the end of the string.
/// If this is not satisfied, it returns `None`, letting the caller fall back to the safe "default wait behavior"
/// instead of risking a false positive.
fn first_command_token(command: &str) -> Option<(&str, &str)> {
    let command = command.trim_start();
    if command.is_empty() {
        return None;
    }

    let mut chars = command.char_indices();
    let (_, first) = chars.next()?;
    if first == '"' || first == '\'' {
        for (idx, ch) in chars {
            if ch == first {
                let token = &command[first.len_utf8()..idx];
                let rest = &command[idx + ch.len_utf8()..];
                // The closing quote must be followed by whitespace or the end of the string;
                // otherwise, it is considered untokenizable, letting the caller fall back to the non-preempting safe path.
                if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
                    return None;
                }
                return Some((token, rest));
            }
        }

        // No matching closing quote found: also considered untokenizable.
        return None;
    }

    let end = command.find(char::is_whitespace).unwrap_or(command.len());
    Some((&command[..end], &command[end..]))
}

fn command_basename(command_token: &str) -> &str {
    command_token
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command_token)
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum BlockSelector {
    Id(BlockId),
    RequestedCommandId(AIAgentActionId),
}

impl BlockSelector {
    fn get_block<'a>(&self, model: &'a TerminalModel) -> Option<&'a Block> {
        match self {
            BlockSelector::Id(block_id) => model.block_list().block_with_id(block_id),
            BlockSelector::RequestedCommandId(requested_command_id) => model
                .block_list()
                .block_for_ai_action_id(requested_command_id),
        }
    }
}

/// Returns the result from executing a requested command.
fn action_result_for_requested_command(
    command: String,
    result: ActionResult,
) -> AIAgentActionResultType {
    match result {
        ActionResult::CommandFinished {
            block_id,
            output,
            exit_code,
        } => AIAgentActionResultType::RequestCommandOutput(RequestCommandOutputResult::Completed {
            command,
            block_id,
            output,
            exit_code,
        }),
        ActionResult::LongRunningCommandSnapshot {
            block_id,
            grid_contents,
            cursor,
            is_alt_screen_active,
            ..
        } => AIAgentActionResultType::RequestCommandOutput(
            RequestCommandOutputResult::LongRunningCommandSnapshot {
                command,
                block_id,
                grid_contents,
                cursor: cursor.to_owned(),
                is_alt_screen_active,
            },
        ),
        ActionResult::BlockNotFound | ActionResult::Cancelled => {
            AIAgentActionResultType::RequestCommandOutput(
                RequestCommandOutputResult::CancelledBeforeExecution,
            )
        }
    }
}

/// Returns the result from writing to a long-running shell command.
fn action_result_for_write_to_long_running_shell_command(
    result: ActionResult,
) -> AIAgentActionResultType {
    match result {
        ActionResult::CommandFinished {
            block_id,
            output,
            exit_code,
        } => AIAgentActionResultType::WriteToLongRunningShellCommand(
            WriteToLongRunningShellCommandResult::CommandFinished {
                block_id,
                output,
                exit_code,
            },
        ),
        ActionResult::LongRunningCommandSnapshot {
            block_id,
            grid_contents,
            cursor,
            is_alt_screen_active,
            is_preempted,
        } => AIAgentActionResultType::WriteToLongRunningShellCommand(
            WriteToLongRunningShellCommandResult::Snapshot {
                block_id,
                grid_contents,
                cursor: cursor.to_owned(),
                is_alt_screen_active,
                is_preempted,
            },
        ),
        ActionResult::Cancelled => AIAgentActionResultType::WriteToLongRunningShellCommand(
            WriteToLongRunningShellCommandResult::Cancelled,
        ),
        ActionResult::BlockNotFound => AIAgentActionResultType::WriteToLongRunningShellCommand(
            WriteToLongRunningShellCommandResult::Error(ShellCommandError::BlockNotFound),
        ),
    }
}

/// Returns the result from reading shell command output.
fn action_result_for_read_shell_command_output(
    command: String,
    result: ActionResult,
) -> AIAgentActionResultType {
    match result {
        ActionResult::CommandFinished {
            output,
            exit_code,
            block_id,
        } => AIAgentActionResultType::ReadShellCommandOutput(
            ReadShellCommandOutputResult::CommandFinished {
                command,
                block_id,
                output,
                exit_code,
            },
        ),
        ActionResult::LongRunningCommandSnapshot {
            block_id,
            grid_contents,
            cursor,
            is_alt_screen_active,
            is_preempted,
        } => AIAgentActionResultType::ReadShellCommandOutput(
            ReadShellCommandOutputResult::LongRunningCommandSnapshot {
                command,
                block_id,
                grid_contents,
                cursor: cursor.to_owned(),
                is_alt_screen_active,
                is_preempted,
            },
        ),
        ActionResult::Cancelled => {
            AIAgentActionResultType::ReadShellCommandOutput(ReadShellCommandOutputResult::Cancelled)
        }
        ActionResult::BlockNotFound => AIAgentActionResultType::ReadShellCommandOutput(
            ReadShellCommandOutputResult::Error(ShellCommandError::BlockNotFound),
        ),
    }
}

/// Returns the result from transferring shell command control to user.
fn action_result_for_transfer_shell_command_control_to_user(
    result: ActionResult,
) -> AIAgentActionResultType {
    match result {
        ActionResult::CommandFinished {
            block_id,
            output,
            exit_code,
        } => AIAgentActionResultType::TransferShellCommandControlToUser(
            TransferShellCommandControlToUserResult::CommandFinished {
                block_id,
                output,
                exit_code,
            },
        ),
        ActionResult::LongRunningCommandSnapshot {
            block_id,
            grid_contents,
            cursor,
            is_alt_screen_active,
            is_preempted,
        } => AIAgentActionResultType::TransferShellCommandControlToUser(
            TransferShellCommandControlToUserResult::Snapshot {
                block_id,
                grid_contents,
                cursor: cursor.to_owned(),
                is_alt_screen_active,
                is_preempted,
            },
        ),
        ActionResult::Cancelled => AIAgentActionResultType::TransferShellCommandControlToUser(
            TransferShellCommandControlToUserResult::Cancelled,
        ),
        ActionResult::BlockNotFound => AIAgentActionResultType::TransferShellCommandControlToUser(
            TransferShellCommandControlToUserResult::Error(ShellCommandError::BlockNotFound),
        ),
    }
}

#[derive(Debug, Clone)]
pub enum ShellCommandExecutorEvent {
    ExecuteCommand {
        action_id: AIAgentActionId,
        command: String,
    },
    WriteToPty {
        input: Bytes,
        mode: AIAgentPtyWriteMode,
    },
    CancelExecution,
    /// Emitted when the agent requests to transfer control of a long-running command to the user.
    TransferControlToUser {
        action_id: AIAgentActionId,
        reason: String,
    },
}

impl Entity for ShellCommandExecutor {
    type Event = ShellCommandExecutorEvent;
}

/// Result from waiting for control transfer.
#[derive(Debug, Clone)]
enum TransferControlResult {
    ControlHandedBack,
    BlockFinished,
    Cancelled,
}

/// The possible results of taking an action.
#[derive(Debug, Clone)]
enum ActionResult {
    CommandFinished {
        block_id: BlockId,
        output: String,
        exit_code: ExitCode,
    },
    LongRunningCommandSnapshot {
        block_id: BlockId,
        grid_contents: String,
        cursor: &'static str,
        is_alt_screen_active: bool,
        is_preempted: bool,
    },
    Cancelled,
    BlockNotFound,
}

#[cfg(test)]
#[path = "shell_command_tests.rs"]
mod tests;
