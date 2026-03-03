use std::env;

/// Get or create a session ID for the current shell session.
///
/// The session ID is stored in the `WAZ_SESSION_ID` environment variable.
/// If not set, a new UUID is generated. The shell integration scripts are
/// responsible for setting this env var on shell startup.
pub fn get_session_id() -> String {
    env::var("WAZ_SESSION_ID").unwrap_or_else(|_| {
        // Generate a new session ID if none exists
        uuid::Uuid::new_v4().to_string()
    })
}

/// Generate a new session ID (used by shell integration on startup).
pub fn new_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
