//! SSH password/passphrase automatically injected. Subscribe to the terminal pane's PTY output broadcast,
//! Match `password:` / `passphrase:` and write secret + `\n` **once** after the end-of-line prompt.
//!
//! ## Key Design Tradeoffs
//!
//! - **8KB sliding window + strict matching at the end of the line**: Regular `(?im)(password|passphrase)[^\n]*:\s*$`
//!   Only match the end of the line (to avoid accidentally hitting the word "password" in motd/banner) + sliding window to ensure the upper memory limit.
//!
//! - **15s timeout**: Typical SSH public key negotiation < 2s, password prompt < 5s. 15s is public key authentication
//!   A reasonable upper limit for failure + fallback passwords. **Login-free boundary with public key**(authorized_keys
//!   Configured + we also saved the password): Public key handshake is successful → no prompt will appear → injector is silent
//!   Timeout and exit, **will not be randomly injected into the shell after login**.
//!
//! - **One-time trigger**: break immediately after matching, injector future exit → InactiveReceiver
//!   drop → Subsequent PTY streams will no longer be seen by this injector, **preventing secondary injection**.
//!
//! - **bytes::Regex**:PTY output may contain incomplete UTF-8 bytes, use `regex::bytes` to be safe.

use std::sync::Arc;
use std::time::Duration;

use async_broadcast::InactiveReceiver;
use warpui::r#async::FutureExt;
use warpui::{ViewContext, WeakViewHandle};
use zeroize::Zeroizing;

use crate::ssh_manager::password_prompt::bytes_look_like_password_prompt;
use crate::terminal::TerminalView;

/// Injection timeout limit.
const INJECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The sliding window retains the most recent PTY output of this many bytes for regular matching.
const SLIDING_WINDOW_BYTES: usize = 8 * 1024;
/// When the buffer exceeds this value, drain to the sliding window size.
const BUFFER_HARD_LIMIT: usize = 16 * 1024;

/// Spawn a one-time injection task in the owner=Workspace context. Workspace drop
/// The task is automatically canceled; the owner does not need to abort.
///
/// Calling premise: `pty_reads_rx` by `terminal_view.inactive_pty_reads_rx(ctx)`
/// The future will actually start when **Some is obtained; wasm/remote session gets None and directly no-ops.
pub fn spawn_password_injector<O>(
    pty_reads_rx: Option<InactiveReceiver<Arc<Vec<u8>>>>,
    terminal_view: WeakViewHandle<TerminalView>,
    secret: Zeroizing<String>,
    ctx: &mut ViewContext<O>,
) where
    O: warpui::View + 'static,
{
    let Some(rx) = pty_reads_rx else {
        log::debug!("ssh secret injector: no pty_reads_rx (non-local session) — skip");
        return;
    };
    if secret.is_empty() {
        log::debug!("ssh secret injector: empty secret — skip");
        return;
    }

    // When taking off, set in-flight to true and notify the OneKey listener before this injection is completed.
    // Don't pop up the menu. In this way, no matter whether the injector is injected first, it is onekey's turn to see the same segment of bytes.
    // Or onekey sees it first, the semantics are unified: **injector takes precedence**.
    if let Some(view) = terminal_view.upgrade(ctx) {
        view.update(ctx, |view, _| {
            view.set_ssh_secret_auto_injection_in_flight(true);
        });
    }

    let owned_secret = secret.clone();
    let future = async move {
        match watch_for_prompt(rx).with_timeout(INJECT_TIMEOUT).await {
            Ok(true) => Some(owned_secret),
            Ok(false) | Err(_) => None, // EOF or timeout → no-op
        }
    };
    ctx.spawn(future, move |_owner, secret_opt, ctx| {
        let Some(view) = terminal_view.upgrade(ctx) else {
            log::debug!("ssh secret injector: terminal view dropped before injection");
            return;
        };
        let Some(secret) = secret_opt else {
            log::debug!("ssh secret injector: no prompt seen within timeout");
            view.update(ctx, |view, _| {
                view.set_ssh_secret_auto_injection_in_flight(false);
            });
            return;
        };
        view.update(ctx, |view, ctx| {
            // Write the password + newline as bytes into PTY, which is equivalent to simulating keyboard keys to respond to an interactive prompt.
            // At this time, ssh is already running (bootstrap has been completed earlier), and direct writing of write_to_pty is the correct solution.
            let mut bytes = secret.as_bytes().to_vec();
            bytes.push(b'\n');
            view.write_to_pty(bytes, ctx);
            view.note_ssh_secret_auto_injected(ctx);
            view.set_ssh_secret_auto_injection_in_flight(false);
        });
    });
}

/// Asynchronous loop: consume PTY broadcast, sliding window append, **Regular returns true once the end-of-line prompt is hit**;
/// EOF returns false. timeout is wrapped by the caller `with_timeout`.
async fn watch_for_prompt(rx: InactiveReceiver<Arc<Vec<u8>>>) -> bool {
    let mut active = rx.activate_cloned();
    let mut buf: Vec<u8> = Vec::with_capacity(SLIDING_WINDOW_BYTES);
    while let Ok(chunk) = active.recv().await {
        buf.extend_from_slice(&chunk);
        if buf.len() > BUFFER_HARD_LIMIT {
            let drop_n = buf.len() - SLIDING_WINDOW_BYTES;
            buf.drain(..drop_n);
        }
        if bytes_look_like_password_prompt(&buf) {
            return true;
        }
    }
    false
}
