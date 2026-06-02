//! su password confirmation prompt. Continuously monitor PTY output, when user input `su root` / `su - root` is detected
//! When a password prompt appears after switching to the root command, a confirmation menu pops up, and the user confirms and injects the root password.
//!
//! Only injected into the root target, `su lg` will not trigger when switching to other users.
//! Wait for the shell prompt to appear (indicating that the SSH login has been completed) before starting the detection to avoid conflicts with the login password.
//! Use `spawn_stream_local` + `stream!` to achieve continuous monitoring, which will be triggered every time `su root` is called.

use std::sync::Arc;
use std::time::Duration;

use async_broadcast::InactiveReceiver;
use async_stream::stream;
use lazy_static::lazy_static;
use regex::bytes::Regex;
use warpui::r#async::FutureExt;
use warpui::{ViewContext, WeakViewHandle};
use zeroize::Zeroizing;

use crate::ssh_manager::shell_prompt::bytes_look_like_shell_prompt;
use crate::terminal::TerminalView;

const SLIDING_WINDOW_BYTES: usize = 8 * 1024;
const BUFFER_HARD_LIMIT: usize = 16 * 1024;
/// Phase 1 The maximum amount of time to wait for the shell prompt. If the timeout occurs, the entire stream is discarded (and the
/// Reset in_flight in `on_done`).
const SHELL_READY_TIMEOUT: Duration = Duration::from_secs(30);

lazy_static! {
    /// Password prompt regex — strictly matches two categories:
    /// 1. `password` / `passphrase` / `password` with half-width colon `:` or full-width colon `:` at the end of the line
    /// 2. Colon-less `Enter Password` for Galaxy Kirin V10
    ///
    /// The old implementation made the colon optional at the end of any line containing "password" (e.g.
    /// `Your password has expired`) will all be false positives.
    static ref PASSWORD_PROMPT_REGEX: Regex = Regex::new(
        r"(?im)(?:(?:password|passphrase|密码)[^\n]*(?::|：)\s*$|输入密码\s*$)"
    )
    .expect("su password prompt regex must compile");

    /// su command regex — matches su commands targeting root (end of line):
    /// `su` / `su -` / `su -l` / `su --login` / `su root` / `su - root` /
    /// `su -l root` / `su --login root`. Does not match `su lg` / `su - lg` etc. cut to
    /// Other user forms; `sudo su` still hits the trailing `su` because of the `\bsu` word boundary.
    static ref SU_ROOT_CMD_REGEX: Regex =
        Regex::new(r"(?m)\bsu(?:\s+(?:-l?|--login|-))*(?:\s+root)?\s*$")
            .expect("su root cmd regex must compile");
}

/// In the owner context spawn su password continues to listen to the stream.
pub fn spawn_su_password_injector<O>(
    pty_reads_rx: Option<InactiveReceiver<Arc<Vec<u8>>>>,
    terminal_view: WeakViewHandle<TerminalView>,
    root_password: Zeroizing<String>,
    ctx: &mut ViewContext<O>,
) where
    O: warpui::View + 'static,
{
    let Some(rx) = pty_reads_rx else {
        log::debug!("ssh su password injector: no pty_reads_rx — skip");
        return;
    };
    if root_password.is_empty() {
        log::debug!("ssh su password injector: empty root password — skip");
        return;
    }

    // Set the in-flight flag to prevent the OneKey credential selection box from popping up while waiting for the shell prompt.
    if let Some(view) = terminal_view.upgrade(ctx) {
        view.update(ctx, |view, _| {
            view.set_ssh_secret_auto_injection_in_flight(true);
        });
    }

    let prompt_stream = stream! {
        let mut active = rx.activate_cloned();
        let mut buf: Vec<u8> = Vec::with_capacity(SLIDING_WINDOW_BYTES);

        // Phase 1: Wait for shell prompt (SHELL_READY_TIMEOUT timeout), indicating that the login is completed
        loop {
            match active.recv().with_timeout(SHELL_READY_TIMEOUT).await {
                Ok(Ok(chunk)) => {
                    buf.extend_from_slice(&chunk);
                    if buf.len() > BUFFER_HARD_LIMIT {
                        let drop_n = buf.len() - SLIDING_WINDOW_BYTES;
                        buf.drain(..drop_n);
                    }
                    if bytes_look_like_shell_prompt(&buf) {
                        break;
                    }
                }
                _ => return,
            }
        }

        // Stage 2: Continuously detect su root + password prompt, and continue to listen after each yield
        buf.clear();
        while let Ok(chunk) = active.recv().await {
            buf.extend_from_slice(&chunk);
            if buf.len() > BUFFER_HARD_LIMIT {
                let drop_n = buf.len() - SLIDING_WINDOW_BYTES;
                buf.drain(..drop_n);
            }
            if PASSWORD_PROMPT_REGEX.is_match(&buf) && is_su_to_root(&buf) {
                buf.clear();
                yield ();
            }
        }
    };

    // on_done must reset in_flight: Phase 1 (waiting for shell prompt) if timeout/EOF directly
    // `return` exits the stream. The on_item has not been passed at this time. If it is not reset in on_done,
    // OneKey will be permanently blocked on this terminal.
    let terminal_view_done = terminal_view.clone();
    let _ = ctx.spawn_stream_local(
        prompt_stream,
        move |_owner, (), ctx| {
            let Some(view) = terminal_view.upgrade(ctx) else {
                return;
            };
            view.update(ctx, |view, ctx| {
                view.su_root_password = Some(root_password.clone());
                view.show_su_root_confirm_menu(ctx);
                view.set_ssh_secret_auto_injection_in_flight(false);
            });
        },
        move |_owner, ctx| {
            if let Some(view) = terminal_view_done.upgrade(ctx) {
                view.update(ctx, |view, _| {
                    view.set_ssh_secret_auto_injection_in_flight(false);
                });
            }
        },
    );
}

/// Check if the buffer contains a su command targeting root.
fn is_su_to_root(buf: &[u8]) -> bool {
    SU_ROOT_CMD_REGEX.is_match(buf)
}

#[cfg(test)]
#[path = "su_password_injector_tests.rs"]
mod tests;
