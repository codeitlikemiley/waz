//! Waz local identity facade.
//!
//! This module retains the type surfaces and public method signatures of `AuthState`,
//! `AuthStateProvider`, `AuthManager`, `User`, `UserUid`, `Credentials`, etc.,
//! while **all method bodies are localized**:
//! - `is_logged_in()` / each `is_*` predicate: returns a constant corresponding to the local user.
//! - `user_id()`: returns the constant [`UserUid`] based on `TEST_USER_UID`.
//! - `username_for_display` / `display_name`: based on the [`User::test`] placeholder metadata.
//! - External account callback triggers have been taken offline and no longer depend on remote account clients.
//!
//! The 167 calls to `crate::auth::AuthStateProvider::as_ref(ctx).get()` compile without modification,
//! and at runtime will always obtain the local placeholder state of "logged in, Free Tier with unlimited quota".
//!
//! For the physical deletion list, see README: 21 files including UI, RPC, token persistence, web handoff,
//! login_slide, paste_auth_token_modal, web_handoff, etc., have been offline along with the external account system.

use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use crate::server_time::ServerTimestamp;

pub const TEST_USER_EMAIL: &str = "test_user@warp.dev";
pub const TEST_USER_UID: &str = "test_user_uid";

pub mod user_uid;

pub use user_uid::UserUid;

#[derive(Clone, Copy, Debug)]
pub enum OwnerType {
    Team,
    User,
}

/// Waz local API key prefix.
///
/// Historically used to identify "strings starting with wk- as managed API keys". In the BYOP path,
/// there is no concept of a managed account API key. The constant is still consumed internally by
/// `AuthState::initialize` and matched by a few legacy call sites, so it is retained.
pub const API_KEY_PREFIX: &str = "wk-";

// ---------- Credentials / AuthToken / LoginToken ----------
//
// Originally used for runtime branching between several authentication methods like managed tokens, API keys, and session cookies.
// After Waz localization, only the `ApiKey` and `Test` variants, which are actually used, are kept.
// Managed tokens and cookie variants have been physically deleted, and all original external account branches under Waz will always walk `None` or return early.

/// Represents the user's authentication method with Waz.
///
/// Waz localized branches:
/// - `ApiKey`: Under the BYOP path, the user brings their own LLM provider API key, which is actually managed
///   by settings/keychain respectively. Here we only keep the enum facade for reading methods like `AuthState::credentials()`.
/// - `Test`: Used in test / `skip_login` builds.
#[derive(Clone, Debug)]
pub enum Credentials {
    /// BYOP / Waz Inc API key, retaining owner_type for old code to read (always `None`).
    ApiKey {
        key: String,
        owner_type: Option<OwnerType>,
    },
    /// Test / `skip_login` build placeholder.
    Test,
}

impl Credentials {
    /// Returns the API key string (only when the variant is [`Credentials::ApiKey`]).
    pub fn as_api_key(&self) -> Option<&str> {
        match self {
            Credentials::ApiKey { key, .. } => Some(key),
            Credentials::Test => None,
        }
    }

    /// Returns the API key owner type (always `None` under the Waz path).
    pub fn api_key_owner_type(&self) -> Option<OwnerType> {
        match self {
            Credentials::ApiKey { owner_type, .. } => *owner_type,
            Credentials::Test => None,
        }
    }

    /// Returns the bearer token to write to the Authorization header.
    ///
    /// After localization, only `ApiKey` yields a real value; `Test` returns [`AuthToken::NoAuth`].
    pub fn bearer_token(&self) -> AuthToken {
        match self {
            Credentials::ApiKey { key, .. } => AuthToken::ApiKey(key.clone()),
            Credentials::Test => AuthToken::NoAuth,
        }
    }
}

/// Short-term token used by HTTP request headers.
#[derive(Debug, Clone)]
pub enum AuthToken {
    /// BYOP / platform layer API key.
    ApiKey(String),
    /// No token (session cookie / test / Waz local mode).
    NoAuth,
}

impl AuthToken {
    /// Returns the bearer token string (if any).
    pub fn bearer_token(&self) -> Option<String> {
        match self {
            AuthToken::ApiKey(key) => Some(key.clone()),
            AuthToken::NoAuth => None,
        }
    }

    /// Returns the token reference used by the Authorization header.
    pub fn as_bearer_token(&self) -> Option<&str> {
        match self {
            AuthToken::ApiKey(key) => Some(key),
            AuthToken::NoAuth => None,
        }
    }
}

// ---------- User metadata ----------

/// Anonymous user type facade. After Waz localization, there is no concept of anonymous users.
/// Retaining the enum allows match arms scattered in telemetry / settings to compile.
/// No Waz code paths will construct `Some(AnonymousUserType::...)`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AnonymousUserType {
    NativeClientAnonymousUser,
    NativeClientAnonymousUserFeatureGated,
    WebClientAnonymousUser,
}

/// Authentication principal type facade. Waz always equates to `User`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrincipalType {
    #[default]
    User,
    ServiceAccount,
}

/// Personal object limit facade (formerly free tier limits for anonymous users). Waz never constructs this value,
/// but retains the struct so consumers can continue compiling.
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct PersonalObjectLimits {
    pub env_var_limit: usize,
    pub notebook_limit: usize,
    pub workflow_limit: usize,
}

/// User metadata facade, keeping only a few fields for telemetry / display use.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UserMetadata {
    pub email: String,
    pub display_name: Option<String>,
    pub photo_url: Option<String>,
}

/// Currently logged-in user (local placeholder).
#[derive(Debug, Clone)]
pub struct User {
    pub local_id: UserUid,
    pub metadata: UserMetadata,
    pub is_onboarded: bool,
    pub needs_sso_link: bool,
    pub anonymous_user_type: Option<AnonymousUserType>,
    pub is_on_work_domain: bool,
    pub linked_at: Option<ServerTimestamp>,
    pub personal_object_limits: Option<PersonalObjectLimits>,
    pub principal_type: PrincipalType,
}

impl User {
    /// Username for display - display_name has priority, otherwise email.
    pub fn username_for_display(&self) -> &str {
        self.metadata
            .display_name
            .as_deref()
            .unwrap_or(self.metadata.email.as_str())
    }

    /// User display name, does not fall back to email.
    pub fn display_name(&self) -> Option<String> {
        self.metadata.display_name.clone()
    }

    /// Test/default user placeholder. Waz uses this user in all paths.
    pub fn test() -> Self {
        Self {
            local_id: UserUid::new(TEST_USER_UID),
            metadata: UserMetadata {
                email: TEST_USER_EMAIL.to_string(),
                display_name: None,
                photo_url: None,
            },
            is_onboarded: true,
            needs_sso_link: false,
            anonymous_user_type: None,
            is_on_work_domain: false,
            linked_at: None,
            personal_object_limits: None,
            principal_type: PrincipalType::User,
        }
    }

    /// Whether the user is anonymous. Waz always returns `false`.
    pub fn is_user_anonymous(&self) -> bool {
        false
    }

    pub fn anonymous_user_type(&self) -> Option<AnonymousUserType> {
        self.anonymous_user_type
    }

    pub fn personal_object_limits(&self) -> Option<PersonalObjectLimits> {
        self.personal_object_limits
    }

    pub fn linked_at(&self) -> Option<ServerTimestamp> {
        self.linked_at
    }
}

// ---------- AuthState ----------

/// Current authentication state (localized stub).
///
/// All queries for "whether logged in, whether anonymous, whether re-authentication is needed" return fixed values;
/// `user_id()` always returns `Some(UserUid::new(TEST_USER_UID))`.
/// 167+ consumption points compile with zero modifications.
pub struct AuthState {
    user: RwLock<Option<User>>,
    credentials: RwLock<Option<Credentials>>,
}

impl Default for AuthState {
    fn default() -> Self {
        Self::new_for_test()
    }
}

impl AuthState {
    /// Creates a local default AuthState (always regarded as a logged-in test user).
    pub fn new() -> Self {
        Self {
            user: RwLock::new(Some(User::test())),
            credentials: RwLock::new(Some(Credentials::Test)),
        }
    }

    /// Constructs AuthState in test scenarios (equivalent to [`AuthState::new`]).
    pub fn new_for_test() -> Self {
        Self::new()
    }

    /// Initializes AuthState. The `api_key` parameter is faithfully retained (BYOP entry point might still pass it in),
    /// but all other external account check paths are no-ops.
    #[cfg_attr(target_family = "wasm", allow(unused_variables))]
    pub fn initialize(_ctx: &AppContext, api_key: Option<String>) -> Self {
        let state = Self::new();
        if let Some(api_key_value) = api_key {
            let formatted = if api_key_value.starts_with(API_KEY_PREFIX) {
                api_key_value
            } else {
                format!("{API_KEY_PREFIX}{api_key_value}")
            };
            *state.credentials.write() = Some(Credentials::ApiKey {
                key: formatted,
                owner_type: None,
            });
        }
        state
    }

    /// Whether the user is logged in. Waz is always `true`.
    pub fn is_logged_in(&self) -> bool {
        true
    }

    /// Whether anonymous or logged out. Waz is always `false`.
    pub fn is_anonymous_or_logged_out(&self) -> bool {
        false
    }

    /// Returns the cached access token (ignoring validity). Under the Waz path, it only has a value
    /// when the user has `Credentials::ApiKey`.
    pub fn get_access_token_ignoring_validity(&self) -> Option<String> {
        self.credentials
            .read()
            .as_ref()?
            .bearer_token()
            .bearer_token()
    }

    pub fn username_for_display(&self) -> Option<String> {
        Some(self.user.read().as_ref()?.username_for_display().to_owned())
    }

    pub fn display_name(&self) -> Option<String> {
        self.user
            .read()
            .as_ref()
            .and_then(|user| user.display_name())
    }

    pub fn user_email(&self) -> Option<String> {
        self.user
            .read()
            .as_ref()
            .map(|user| user.metadata.email.clone())
    }

    pub fn is_onboarded(&self) -> Option<bool> {
        self.user.read().as_ref().map(|user| user.is_onboarded)
    }

    pub fn user_email_domain(&self) -> Option<String> {
        self.user.read().as_ref().map(|user| {
            user.metadata
                .email
                .split('@')
                .nth(1)
                .unwrap_or("")
                .to_string()
        })
    }

    pub fn is_user_anonymous(&self) -> Option<bool> {
        Some(false)
    }

    pub fn is_user_web_anonymous_user(&self) -> Option<bool> {
        Some(false)
    }

    pub fn is_anonymous_user_feature_gated(&self) -> Option<bool> {
        Some(false)
    }

    /// Waz local users will never hit the Free Tier limit.
    pub fn is_anonymous_user_past_object_limit(
        &self,
        _object_type: crate::cloud_object::ObjectType,
        _num_objects: usize,
    ) -> Option<bool> {
        Some(false)
    }

    pub fn user_photo_url(&self) -> Option<String> {
        self.user
            .read()
            .as_ref()
            .and_then(|user| user.metadata.photo_url.clone())
    }

    pub fn needs_sso_link(&self) -> Option<bool> {
        Some(false)
    }

    pub fn anonymous_user_type(&self) -> Option<AnonymousUserType> {
        None
    }

    pub fn personal_object_limits(&self) -> Option<PersonalObjectLimits> {
        None
    }

    /// Marks the user as onboarded.
    pub fn set_is_onboarded(&self, is_onboarded: bool) {
        if let Some(user) = self.user.write().as_mut() {
            user.is_onboarded = is_onboarded;
        }
    }

    pub fn user_id(&self) -> Option<UserUid> {
        self.user.read().as_ref().map(|user| user.local_id)
    }

    /// Returns a nil UUID string. After Waz localization, this ID no longer appears in
    /// any outgoing HTTP headers, and only serves as a formal placeholder for telemetry context / session headers.
    pub fn anonymous_id(&self) -> String {
        Uuid::nil().to_string()
    }

    /// Returns whether re-authentication is required. Waz is always `false`.
    pub fn needs_reauth(&self) -> bool {
        false
    }

    /// Returns whether the current user's anonymous renotification block has expired. Waz users
    /// are not considered anonymous users, so this function returns `false` (never pops up registration prompts).
    pub fn anonymous_user_renotification_block_expired(
        &self,
        _last_time_opt: Option<String>,
    ) -> bool {
        false
    }

    pub fn is_on_work_domain(&self) -> Option<bool> {
        Some(false)
    }

    pub fn is_api_key_authenticated(&self) -> bool {
        matches!(
            self.credentials.read().as_ref(),
            Some(Credentials::ApiKey { .. })
        )
    }

    pub fn api_key(&self) -> Option<String> {
        self.credentials
            .read()
            .as_ref()
            .and_then(|c| c.as_api_key().map(|s| s.to_owned()))
    }

    pub fn principal_type(&self) -> Option<PrincipalType> {
        Some(PrincipalType::User)
    }

    pub fn is_service_account(&self) -> bool {
        false
    }

    pub fn api_key_owner_type(&self) -> Option<OwnerType> {
        self.credentials.read().as_ref()?.api_key_owner_type()
    }

    /// Returns a clone of the current credentials.
    pub fn credentials(&self) -> Option<Credentials> {
        self.credentials.read().clone()
    }

    /// Restores the local auth state to the default snapshot of the local placeholder user, used for `log_out` and local reset paths.
    pub fn reset_local_defaults(&self) {
        *self.user.write() = Some(User::test());
        *self.credentials.write() = Some(Credentials::Test);
    }
}

impl warp_managed_secrets::ActorProvider for AuthState {
    fn actor_uid(&self) -> Option<String> {
        self.user_id().map(|uid| uid.as_string())
    }
}

/// Singleton wrapper of AuthState.
pub struct AuthStateProvider {
    auth_state: Arc<AuthState>,
}

impl AuthStateProvider {
    pub fn new(auth_state: Arc<AuthState>) -> Self {
        Self { auth_state }
    }

    pub fn new_for_test() -> Self {
        Self {
            auth_state: Arc::new(AuthState::new_for_test()),
        }
    }

    /// Constructs a "logged-out" AuthState provider.
    ///
    /// Waz no longer has a true logged-out state. This function returns the "logged-in test user"
    /// provider equivalent to `new_for_test` to ensure legacy test code continues to compile.
    pub fn new_logged_out_for_test() -> Self {
        Self::new_for_test()
    }

    pub fn get(&self) -> &Arc<AuthState> {
        &self.auth_state
    }
}

impl Entity for AuthStateProvider {
    type Event = ();
}

impl SingletonEntity for AuthStateProvider {}

// ---------- AuthManager facade ----------

/// Legacy "login gated feature" identifier from the old UI, as a string constant (originally `&'static str`).
pub type LoginGatedFeature = &'static str;

/// URL constructor callback for `AuthManager::open_url_maybe_with_anonymous_token`.
///
/// In the original UI, this callback would receive the anonymous user token and construct a URL to "open the browser with identity".
/// Under Waz, the anonymous identity no longer exists, and the callback is discarded.
pub type AnonymousTokenUrlBuilder = Box<dyn FnOnce(Option<&str>) -> String>;

/// AuthView variant facade. Waz has physically deleted AuthView UI. All dispatch points in the stub
/// only produce logs, but the enum surface is preserved for old `match` arms to compile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthViewVariant {
    Initial,
    RequireLoginCloseable,
    ShareRequirementCloseable,
}

// ---------- UI view facade (placeholder of the physically deleted UI) ----------
//
// `root_view.rs` / `workspace/view.rs` originally held 6 `ViewHandle<T>` fields,
// and events originating from these views. After physically deleting the view body in Wave 3-1, retaining these
// view + event enum facades allows `ViewHandle<AuthView>` type, event match arms,
// and `ctx.add_typed_action_view(AuthView::new)` calls to still compile.
//
// At runtime, these view code paths will still be created but not rendered (`View::render` returns `Empty`),
// and events will not be triggered (since the original UI interaction points no longer exist).

use warpui::elements::Empty;
use warpui::{Element, View, ViewContext};

/// AuthView facade. The original UI contained a "Login / Sign Up" form, which has been physically deleted after localization.
pub struct AuthView {
    variant: AuthViewVariant,
}

impl AuthView {
    pub fn new(variant: AuthViewVariant, _ctx: &mut ViewContext<Self>) -> Self {
        Self { variant }
    }

    pub fn set_variant(&mut self, _ctx: &mut ViewContext<Self>, variant: AuthViewVariant) {
        self.variant = variant;
    }

    /// Returns the current variant. Unused under the Waz path.
    pub fn variant(&self) -> AuthViewVariant {
        self.variant
    }

    /// The original native login UI skips the "enter password" step and proceeds to the subsequent "open in browser" step. Waz: no-op.
    pub fn skip_to_browser_open_step(&mut self, _ctx: &mut ViewContext<Self>) {}
}

impl Entity for AuthView {
    type Event = AuthViewEvent;
}

impl View for AuthView {
    fn ui_name() -> &'static str {
        "AuthView (stub)"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Box::new(Empty::new())
    }
}

impl warpui::TypedActionView for AuthView {
    type Action = ();
    fn handle_action(&mut self, _action: &(), _ctx: &mut ViewContext<Self>) {}
}

#[derive(Debug)]
pub enum AuthViewEvent {
    Close,
}

/// AuthOverrideWarningModal facade.
pub struct AuthOverrideWarningModal;

impl AuthOverrideWarningModal {
    pub fn new(_ctx: &mut ViewContext<Self>, _variant: AuthOverrideWarningModalVariant) -> Self {
        Self
    }
}

impl Entity for AuthOverrideWarningModal {
    type Event = AuthOverrideWarningModalEvent;
}

impl View for AuthOverrideWarningModal {
    fn ui_name() -> &'static str {
        "AuthOverrideWarningModal (stub)"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Box::new(Empty::new())
    }
}

impl warpui::TypedActionView for AuthOverrideWarningModal {
    type Action = ();
    fn handle_action(&mut self, _action: &(), _ctx: &mut ViewContext<Self>) {}
}

#[derive(Debug)]
pub enum AuthOverrideWarningModalEvent {
    Close,
    BulkExport,
}

#[derive(Clone, Copy, Debug)]
pub enum AuthOverrideWarningModalVariant {
    OnboardingView,
    WorkspaceModal,
}

/// NeedsSsoLinkView facade.
pub struct NeedsSsoLinkView;

impl NeedsSsoLinkView {
    pub fn new() -> Self {
        Self
    }

    pub fn set_email(&mut self, _email: String) {}
}

impl Default for NeedsSsoLinkView {
    fn default() -> Self {
        Self::new()
    }
}

impl Entity for NeedsSsoLinkView {
    type Event = ();
}

impl View for NeedsSsoLinkView {
    fn ui_name() -> &'static str {
        "NeedsSsoLinkView (stub)"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Box::new(Empty::new())
    }
}

impl warpui::TypedActionView for NeedsSsoLinkView {
    type Action = ();
    fn handle_action(&mut self, _action: &(), _ctx: &mut ViewContext<Self>) {}
}

/// WebHandoffView facade (wasm-only re-login entry point).
pub struct WebHandoffView;

impl WebHandoffView {
    pub fn new(_ctx: &mut ViewContext<Self>) -> Self {
        Self
    }
}

impl Entity for WebHandoffView {
    type Event = WebHandoffEvent;
}

impl View for WebHandoffView {
    fn ui_name() -> &'static str {
        "WebHandoffView (stub)"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Box::new(Empty::new())
    }
}

#[derive(Debug)]
pub enum WebHandoffEvent {
    Unsupported,
}

/// AuthManager event facade. `AuthManagerEvent::AuthComplete` can still be triggered internally by
/// `AuthManager::new` to remain compatible with some subscribers' reliance on the "authenticated" signal.
#[derive(Debug)]
pub enum AuthManagerEvent {
    AuthComplete,
    AuthFailed(UserAuthenticationError),
    SkippedLogin,
    NeedsReauth,
    AttemptedLoginGatedFeature {
        auth_view_variant: AuthViewVariant,
    },
    /// Low frequency failure: same as above.
    CreateAnonymousUserFailed,
}

/// User authentication error facade. A few subscribers still match each variant, so the enum is retained;
/// Waz no longer triggers construction of any variant.
#[derive(Debug, thiserror::Error)]
pub enum UserAuthenticationError {
    #[error("Access token denied")]
    DeniedAccessToken,
    #[error("User account disabled")]
    UserAccountDisabled,
    #[error("Invalid state parameter")]
    InvalidStateParameter,
    #[error("Missing state parameter")]
    MissingStateParameter,
    #[error("Unexpected error: {0}")]
    Unexpected(anyhow::Error),
}

/// Server-persisted user privacy settings facade, still consumed by `settings/privacy.rs`.
#[derive(Copy, Clone, Debug, Default)]
pub struct SyncedUserSettings {
    pub is_crash_reporting_enabled: bool,
    pub is_telemetry_enabled: bool,
}

/// Current user information persisted in the SQLite `current_user_information` table.
/// `persistence/sqlite.rs` and `persistence/mod.rs` still consume this struct, so it is retained.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedCurrentUserInformation {
    pub email: String,
}

/// AuthManager facade. After Waz localization, all external account/RPC entry points become no-ops.
/// `AuthManager` is still registered as a singleton model in the App to guarantee zero-change calls to
/// `subscribe_to_model` / `handle(ctx).update(...)`, while retaining local identity / onboarded flags /
/// logout reset semantics.
pub struct AuthManager {
    auth_state: Arc<AuthState>,
}

impl AuthManager {
    /// Creates AuthManager. After localization, it no longer accepts external account client parameters.
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let auth_state = AuthStateProvider::as_ref(ctx).get().clone();
        Self { auth_state }
    }

    /// Test scenario construction, equivalent to [`Self::new`].
    pub fn new_for_test(ctx: &mut ModelContext<Self>) -> Self {
        Self::new(ctx)
    }

    /// Refreshes the current user state.
    ///
    /// Historically this would refresh the cloud token; after Waz localization, the authentication state is completed
    /// during startup, and no external account requests are sent.
    pub fn refresh_user(&self, _ctx: &mut ModelContext<Self>) {}

    /// Active logout.
    ///
    /// Waz no longer enters a "cloud logged out" state. Here we only restore the local identity snapshot to the default placeholder user,
    /// reused by settings reset / session cleanup and other call sites.
    pub(crate) fn log_out(&mut self, _ctx: &mut ModelContext<Self>) {
        self.auth_state.reset_local_defaults();
        log::debug!("AuthManager::log_out reset locally: switched to local placeholder user state");
    }

    /// Marks re-authentication as required. Localization: no-op.
    pub fn set_needs_reauth(&mut self, _new_value: bool, _ctx: &mut ModelContext<Self>) {}

    /// Creates an anonymous user. Localization: no-op, directly emits `AuthComplete` to advance the onboarding flow.
    pub fn create_anonymous_user(
        &mut self,
        _referral_code: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.emit(AuthManagerEvent::AuthComplete);
    }

    /// Dispatches "anonymous user attempts to touch login-gated feature". Localization: no-op.
    pub fn attempt_login_gated_feature(
        &mut self,
        _feature: LoginGatedFeature,
        _auth_view_variant: AuthViewVariant,
        _ctx: &mut ModelContext<Self>,
    ) {
    }

    /// Anonymous user hits Drive quota limit warning. Localization: no-op.
    pub fn anonymous_user_hit_drive_object_limit(&mut self, _ctx: &mut ModelContext<Self>) {}

    /// Starts anonymous user -> full user browser login flow. Localization: no-op.
    pub fn initiate_anonymous_user_linking(
        &mut self,
        _entrypoint: crate::server::telemetry::AnonymousUserSignupEntrypoint,
        _ctx: &mut ModelContext<Self>,
    ) {
    }

    /// A local onboarded mark is placed after the user is guided through the process.
    pub fn set_user_onboarded(&mut self, ctx: &mut ModelContext<Self>) {
        self.auth_state.set_is_onboarded(true);
        ctx.emit(AuthManagerEvent::AuthComplete);
    }

    // ---------- URL construction facade ----------
    //
    // Old UI (login_slide / paste_auth_token_modal / auth_view_modal) before physical removal
    // These methods are called to populate the historical login prompt link; Waz no longer opens the Waz Cloud login page.
    // After physically deleting the UI, there is no caller, but the enum/trait may still be consumed reflectively and the stub is retained.

    pub fn sign_up_url(&self) -> String {
        String::new()
    }

    pub fn sign_in_url(&self) -> String {
        String::new()
    }

    pub fn upgrade_url(&self) -> String {
        String::new()
    }

    pub fn login_options_url(&self) -> String {
        String::new()
    }

    pub fn link_sso_url(&self) -> String {
        String::new()
    }

    /// Opens url in browser, optionally attached with anonymous token. Localization: no-op.
    pub fn open_url_maybe_with_anonymous_token(
        &mut self,
        _ctx: &mut ModelContext<Self>,
        _url_constructor: AnonymousTokenUrlBuilder,
    ) {
    }

    /// Copies anonymous user linking URL to clipboard. Localization: no-op.
    pub fn copy_anonymous_user_linking_url_to_clipboard(&mut self, _ctx: &mut ModelContext<Self>) {}
}

impl Entity for AuthManager {
    type Event = AuthManagerEvent;
}

impl SingletonEntity for AuthManager {}

// ---------- Full module init ----------

/// Init of Waz local identity facade (no-op).
///
/// The submodules `init`, `auth_view_body::init`, `auth_override_warning_body::init`,
/// `login_slide::init`, and `paste_auth_token_modal::init` originally mounted in `init` have all been physically deleted.
pub fn init(_app: &mut AppContext) {}
