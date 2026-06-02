//! Fluent-based localization layer for Waz Desktop.
//!
//! Load chain:
//!   1. `init()` is called once during startup (idempotent), using `RustEmbed` to load `app/i18n/{locale}/*.ftl`
//!   2. `LANGUAGE_LOADER` is a global `OnceLock<FluentLanguageLoader>`, selecting the current locale
//!      on the fallback chain (defaults to the system locale, can be overridden by settings)
//!   3. The business logic calls `t!("key")` / `t!("key", name = ..)` to retrieve strings; if the key is missing, it automatically falls back to English.
//!
//! When a key is missing:
//!   - If not present in the current locale -> fluent internally falls back to fallback_language (en)
//!   - If even not present in English -> returns the key itself as the string (and logs a warning via `log::warn`, making it easy for CI to catch untranslated entries)

#[cfg(not(target_os = "macos"))]
use i18n_embed::DesktopLanguageRequester;
use i18n_embed::{
    fluent::{fluent_language_loader, FluentLanguageLoader},
    LanguageLoader,
};
use rust_embed::RustEmbed;
use std::sync::OnceLock;
use unic_langid::LanguageIdentifier;

/// Embeds the `app/i18n` directory into the binary. It will be re-embedded on every build (the debug-embed feature is enabled in the workspace).
#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

static LANGUAGE_LOADER: OnceLock<FluentLanguageLoader> = OnceLock::new();

/// Called once in the early stages of app startup.
///
/// `override_locale`: The language explicitly selected by the user in Settings (e.g. "zh-CN"), or `None` to use the system locale.
/// Never panics — if loading fails, it falls back to the built-in English bundle.
pub fn init(override_locale: Option<&str>) {
    if LANGUAGE_LOADER.get().is_some() {
        return;
    }

    let loader = fluent_language_loader!();

    // Always load the fallback (en) bundle first - any missing keys in other locales will fall back to it.
    if let Err(e) = loader.load_fallback_language(&Localizations) {
        log::error!("[i18n] failed to load fallback (en) bundle: {e}");
    }

    // Determine the list of runtime locales (by priority).
    let requested: Vec<LanguageIdentifier> = match override_locale {
        Some(s) => match s.parse::<LanguageIdentifier>() {
            Ok(li) => vec![li],
            Err(e) => {
                log::warn!("[i18n] invalid override_locale {s:?}: {e} — falling back to system");
                system_requested_languages()
            }
        },
        None => system_requested_languages(),
    };

    if let Err(e) = i18n_embed::select(&loader, &Localizations, &requested) {
        log::warn!("[i18n] select() failed: {e} — running with fallback only");
    }

    log::info!(
        "[i18n] initialized; current_languages={:?}, fallback={}",
        loader.current_languages(),
        loader.fallback_language()
    );

    propagate_ui_locale(&loader);

    let _ = LANGUAGE_LOADER.set(loader);
}

/// Forward the resolved UI locale to `warpui::set_ui_locale` so DirectWrite / CoreText
/// glyph fallback biases CJK Han characters toward the user's UI language. Japanese,
/// Simplified Chinese, and Traditional Chinese share Han code points; without a locale
/// hint, DirectWrite tends to pick Microsoft YaHei (Simplified Chinese) on Windows even
/// when the UI is rendered in Japanese.
fn propagate_ui_locale(loader: &FluentLanguageLoader) {
    let langs = loader.current_languages();
    if let Some(li) = langs.first() {
        warpui::set_ui_locale(li.to_string());
    }
}

fn system_requested_languages() -> Vec<LanguageIdentifier> {
    #[cfg(target_os = "macos")]
    {
        macos_requested_languages()
    }

    #[cfg(not(target_os = "macos"))]
    {
        DesktopLanguageRequester::requested_languages()
    }
}

#[cfg(target_os = "macos")]
fn macos_requested_languages() -> Vec<LanguageIdentifier> {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    use warpui::platform::mac::utils::nsstring_as_str;

    unsafe {
        let locale_class = class!(NSLocale);
        let preferred_languages: *const Object = msg_send![locale_class, preferredLanguages];
        let count: usize = msg_send![preferred_languages, count];

        let mut requested = Vec::with_capacity(count);
        for index in 0..count {
            let language: *const Object = msg_send![preferred_languages, objectAtIndex: index];
            match nsstring_as_str(language) {
                Ok(language) => {
                    if let Some(language) = parse_language_identifier(language) {
                        requested.push(language);
                    }
                }
                Err(err) => {
                    log::warn!(
                        "[i18n] failed to read macOS preferred language at index {index}: {err}"
                    );
                }
            }
        }

        languages_or_fallback(requested)
    }
}

fn parse_language_identifier(language: &str) -> Option<LanguageIdentifier> {
    match language.parse::<LanguageIdentifier>() {
        Ok(language) => Some(language),
        Err(err) => {
            log::warn!("[i18n] invalid language identifier {language:?}: {err}");
            None
        }
    }
}

fn languages_or_fallback(languages: Vec<LanguageIdentifier>) -> Vec<LanguageIdentifier> {
    if languages.is_empty() {
        vec![fallback_language()]
    } else {
        languages
    }
}

fn fallback_language() -> LanguageIdentifier {
    "en".parse().expect("en is a valid language identifier")
}

/// Retrieves the global loader. Returns `None` if `init()` has not been called (early stage or test code can use [`t_or`] as a fallback).
pub fn loader() -> Option<&'static FluentLanguageLoader> {
    LANGUAGE_LOADER.get()
}

/// Switches the runtime locale (can be called at any time after `init()`).
///
/// Implementation details: `FluentLanguageLoader::load_languages` internally uses RwLock to protect language data,
/// so it can be hot-replaced using `&loader` without rebuilding. However, **already rendered UI text will not automatically refresh** —
/// `t!()` returns a copy of `String` at that time, and to see the new language, the view must be rebuilt/repainted.
/// The caller can decide whether to trigger a global repaint or prompt the user to restart.
///
/// `locale` takes BCP-47 (e.g. `"en"`, `"zh-CN"`). On failure, the original locale is kept and a warning is logged, without panicking.
pub fn set_locale(locale: &str) {
    let Some(loader) = LANGUAGE_LOADER.get() else {
        log::warn!("[i18n] set_locale({locale:?}) called before init() — ignoring");
        return;
    };
    let lang_id: LanguageIdentifier = match locale.parse() {
        Ok(li) => li,
        Err(e) => {
            log::warn!("[i18n] set_locale({locale:?}): invalid BCP-47: {e}");
            return;
        }
    };
    if let Err(e) = loader.load_languages(&Localizations, &[lang_id]) {
        log::warn!("[i18n] set_locale({locale:?}) failed: {e}");
        return;
    }
    log::info!(
        "[i18n] locale switched to {locale:?}; current_languages={:?}",
        loader.current_languages()
    );
    propagate_ui_locale(loader);
}

/// Resets back to the system language (reverts explicit override).
pub fn reset_to_system_locale() {
    let Some(loader) = LANGUAGE_LOADER.get() else {
        return;
    };
    let requested = system_requested_languages();
    if let Err(e) = i18n_embed::select(loader, &Localizations, &requested) {
        log::warn!("[i18n] reset_to_system_locale failed: {e}");
    }
    propagate_ui_locale(loader);
}

/// Gets the list of active languages (primary choice + fallback). Only used for debugging / settings UI display.
pub fn current_languages() -> Vec<LanguageIdentifier> {
    LANGUAGE_LOADER
        .get()
        .map(|l| l.current_languages())
        .unwrap_or_default()
}

/// Primary entry point for business logic: `t!("key")` or `t!("key", name = value, count = 3)`.
///
/// - Wraps `i18n_embed_fl::fl!`, but adds fallback handling for "loader not initialized":
///   returns the key itself to avoid panics.
/// - Returns `String` (can be passed directly to GPUI Text/label_text without extra conversion).
#[macro_export]
macro_rules! t {
    ($message_id:literal $(,)?) => {{
        match $crate::i18n::loader() {
            Some(loader) => ::i18n_embed_fl::fl!(loader, $message_id),
            None => {
                ::log::warn!(
                    "[i18n] t!({:?}) called before init(); returning key as-is",
                    $message_id
                );
                String::from($message_id)
            }
        }
    }};
    ($message_id:literal, $($args:tt)*) => {{
        match $crate::i18n::loader() {
            Some(loader) => ::i18n_embed_fl::fl!(loader, $message_id, $($args)*),
            None => {
                ::log::warn!(
                    "[i18n] t!({:?}, ...) called before init(); returning key as-is",
                    $message_id
                );
                String::from($message_id)
            }
        }
    }};
}

/// Equivalent to `t!`, but returns `&'static str` (each call permanently allocates heap memory via `Box::leak`).
///
/// Usage constraints: **Only call inside `LazyLock`/one-time initialization** (e.g., scenarios where a struct field in `StaticCommand` is `&'static str` and must retrieve text from fluent). **Prohibited in hot paths or loops**, otherwise memory will be continuously leaked. Key validation of `fl!()` is still performed at compile time.
#[macro_export]
macro_rules! t_static {
    ($message_id:literal $(,)?) => {{
        let s: String = $crate::t!($message_id);
        &*::std::boxed::Box::leak(s.into_boxed_str())
    }};
}

/// Same as `t!` but with an explicit default value, suitable for extremely early stages or when the loader is not ready.
pub fn t_or(message_id: &str, fallback: &str) -> String {
    match LANGUAGE_LOADER.get() {
        Some(loader) if loader.has(message_id) => loader.get(message_id),
        _ => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        init(Some("en"));
        init(Some("en"));
        assert!(loader().is_some());
    }

    #[test]
    fn fallback_chain_works() {
        init(Some("zh-CN"));
        let loader = loader().unwrap();
        // common-ok translated to Chinese
        assert_eq!(loader.get("common-ok"), "确定");
        // Non-existent key — fluent returns the key itself or a string with markers
        let missing = loader.get("definitely-does-not-exist");
        assert!(missing.contains("definitely-does-not-exist"));
    }

    #[test]
    fn requested_languages_keep_preferred_order() {
        let languages = ["ja", "zh-CN"]
            .into_iter()
            .filter_map(parse_language_identifier)
            .collect();

        let languages = languages_or_fallback(languages);

        assert_eq!(languages[0].to_string(), "ja");
        assert_eq!(languages[1].to_string(), "zh-CN");
    }

    #[test]
    fn requested_languages_fall_back_to_english_when_empty() {
        let languages = languages_or_fallback(Vec::new());

        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].to_string(), "en");
    }
}
