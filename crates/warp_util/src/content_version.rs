/// A app-unique version number for content.
/// This is used for tracking and comparing versions of content across the application.
/// The Rich Text Buffer and the LocalFileModel use this for comparing versions of content.
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, PartialEq, Debug, Copy, Eq, PartialOrd, Ord, Hash)]
pub struct ContentVersion(usize);

impl ContentVersion {
    /// Constructs a new app-unique content version.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let raw = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        ContentVersion(raw)
    }

    pub fn as_i32(&self) -> i32 {
        self.0 as i32
    }

    /// Reconstructs a `ContentVersion` from a raw value received over the wire.
    ///
    /// This bypasses the global atomic counter and should only be used at
    /// protocol deserialization boundaries (e.g. converting a `u64` from a
    /// proto message back into a `ContentVersion`).
    pub fn from_raw(val: usize) -> Self {
        ContentVersion(val)
    }

    /// Protocol deserialization helper: constructs `ContentVersion` from a `u64` on the wire,
    /// saturating (`usize::MAX`) on 32-bit platforms instead of implicit truncation.
    /// The native builds of the current repository are all 64-bit; this is just to make the behavior
    /// explicit, avoiding `as usize` silently discarding high bits in improbable 32-bit builds.
    pub fn from_wire_u64(val: u64) -> Self {
        ContentVersion(usize::try_from(val).unwrap_or(usize::MAX))
    }

    /// Returns the underlying value as a `u64` for wire serialization.
    pub fn as_u64(&self) -> u64 {
        self.0 as u64
    }
}

#[cfg(test)]
#[path = "content_version_test.rs"]
mod tests;
