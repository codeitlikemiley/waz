pub mod fonts;
pub mod platform;
pub mod rendering;
pub mod windowing;

// Re-export everything from the core crate.
pub use warpui_core::*;

/// UI locale used to bias DirectWrite / CoreText / fontconfig glyph fallback for CJK Han characters.
/// Set by `app::i18n::init` / `set_locale` so that font fallback for Japanese UI prefers Japanese
/// glyphs (e.g. Yu Gothic / Meiryo) over Simplified Chinese (Microsoft YaHei) on Windows.
mod ui_locale {
    use std::sync::{Arc, Mutex, OnceLock, RwLock};

    static UI_LOCALE: OnceLock<RwLock<String>> = OnceLock::new();

    type LocaleListener = Arc<dyn Fn(&str) + Send + Sync>;
    static LISTENERS: OnceLock<Mutex<Vec<LocaleListener>>> = OnceLock::new();

    fn cell() -> &'static RwLock<String> {
        UI_LOCALE.get_or_init(|| RwLock::new("en-US".to_string()))
    }

    fn listeners() -> &'static Mutex<Vec<LocaleListener>> {
        LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
    }

    pub fn set_ui_locale(locale: impl Into<String>) {
        let s = locale.into();
        if s.is_empty() {
            return;
        }
        {
            let mut guard = cell().write().unwrap();
            if *guard == s {
                return;
            }
            *guard = s.clone();
        }
        let snapshot: Vec<LocaleListener> = listeners().lock().unwrap().iter().cloned().collect();
        for cb in snapshot {
            cb(&s);
        }
    }

    pub fn current_ui_locale() -> String {
        cell().read().unwrap().clone()
    }

    /// Register a callback fired whenever `set_ui_locale` actually changes the stored value.
    /// Used by `TextLayoutSystem` to rebuild cosmic-text's `FontSystem` with the new locale
    /// (it has no public `set_locale`). Subscribers are kept alive by this registry; capture
    /// `Weak` references inside the closure if you want the underlying object to be droppable.
    pub fn on_ui_locale_changed(cb: LocaleListener) {
        listeners().lock().unwrap().push(cb);
    }
}

pub use ui_locale::{current_ui_locale, on_ui_locale_changed, set_ui_locale};

/// Shared CJK Han codepoint check: these characters have different glyphs between ja / zh-Hans / zh-Hant / ko.
/// Callers use this to bias the Han fallback of DirectWrite / cosmic-text towards the local font of the current UI locale
/// (e.g. ja-* -> Yu Gothic UI, ko-* -> Malgun Gothic, zh-Hant -> Microsoft JhengHei UI).
///
/// Covers all Unified Ideographs blocks allocated before Unicode 15.0 (up to Extension G).
/// If Unicode is expanded in the future, append here uniformly as needed — this is the **single source of truth** for CJK Han ranges in this repository.
/// Callers (`crates/warpui/src/windowing/winit/fonts/windows.rs` and
/// `app/src/font_fallback.rs`) should not fork these ranges themselves.
pub fn is_shared_cjk_han(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF       // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
            | 0xFF01..=0xFF0F // Fullwidth ASCII Punctuation (! " # $ % & ' ( ) * + , - . /)
            | 0xFF1A..=0xFF20 // Fullwidth : ; < = > ? @
            | 0xFF3B..=0xFF40 // Fullwidth [ \ ] ^ _ `
            | 0xFF5B..=0xFF65 // Fullwidth { | } ~ and CJK punctuation 。 「 」 、 ・
            | 0x20000..=0x2A6DF // Extension B
            | 0x2A700..=0x2B73F // Extension C
            | 0x2B740..=0x2B81F // Extension D
            | 0x2B820..=0x2CEAF // Extension E
            | 0x2CEB0..=0x2EBEF // Extension F
            | 0x30000..=0x3134F // Extension G
    )
}
