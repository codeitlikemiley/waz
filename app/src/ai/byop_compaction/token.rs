//! Token estimation — align opencode `packages/opencode/src/util/token.ts`.
//!
//! ```ts
//! const CHARS_PER_TOKEN = 4
//! export function estimate(input: string) {
//!   return Math.max(0, Math.round((input || "").length / CHARS_PER_TOKEN))
//! }
//! ```
//!
//! Use `chars().count()` instead of `len()` to avoid UTF-8 multibyte characters skewing estimates to the sky.
//! opencode In JS, `.length` is 1 for characters in BMP, which is consistent with chars().count() in most cases;
//! For emoji beyond BMP, JS is 2 (UTF-16 surrogate pair), Rust chars().count() is 1 —
//! This small deviation has no actual impact on head/tail segmentation.
use super::consts::CHARS_PER_TOKEN;

/// `Math.round(len / 4)` is equivalent. An empty string returns 0.
pub fn estimate(input: &str) -> usize {
    let n = input.chars().count();
    // Math.round is banker's rounding. The previous "rounding to even numbers" behaves as standard rounding in JS.
    // Here using (n + 2) / 4 is equivalent to round(n / 4) for positive integers.
    (n + CHARS_PER_TOKEN / 2) / CHARS_PER_TOKEN
}

/// JSON post-serialization imputation — alignment opencode `compaction.ts:241`:
/// `Token.estimate(JSON.stringify(msgs))`
pub fn estimate_json<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_string(value)
        .map(|s| estimate(&s))
        .unwrap_or(0)
}
