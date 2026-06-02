use std::sync::OnceLock;

type Localizer = fn(&str) -> String;

static LOCALIZER: OnceLock<Localizer> = OnceLock::new();

pub fn set_localizer(localizer: Localizer) {
    let _ = LOCALIZER.set(localizer);
}

pub(crate) fn localized(key: &str, fallback: &str) -> String {
    LOCALIZER
        .get()
        .map(|f| f(key))
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn localized_static(key: &str, fallback: &'static str) -> String {
    localized(key, fallback)
}
