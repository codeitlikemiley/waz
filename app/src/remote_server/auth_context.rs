use std::sync::Arc;

use remote_server::auth::RemoteServerAuthContext;
use warpui::r#async::BoxFuture;

use crate::auth::AuthState;

/// Construct an auth context for use by the remote-server module.
///
/// Waz Wave 3-1: The `AuthClient` trait has been physically removed. Bearer token source changed to read directly
/// `AuthState::get_access_token_ignoring_validity()` (under the Waz path, only when the user hangs
/// `Some` is returned when BYOP API key is used, and `None` is always returned for the rest).
pub fn server_api_auth_context(auth_state: Arc<AuthState>) -> RemoteServerAuthContext {
    let token_auth_state = auth_state.clone();
    let identity_auth_state = auth_state;

    RemoteServerAuthContext::new(
        move || -> BoxFuture<'static, Option<String>> {
            let token = token_auth_state.get_access_token_ignoring_validity();
            Box::pin(async move { token })
        },
        move || remote_server_identity_key(&identity_auth_state),
    )
}

fn remote_server_identity_key(auth_state: &AuthState) -> String {
    // Waz no longer distinguishes between anonymous/logged-in identities, and uses `user_id()` (local test UID) uniformly.
    auth_state
        .user_id()
        .map(|uid| uid.as_string())
        .unwrap_or_else(|| auth_state.anonymous_id())
}
