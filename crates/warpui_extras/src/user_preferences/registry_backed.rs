use std::io;
use std::sync::Mutex;

/// Store user preferences in the Windows Registry.
/// Modeled after https://github.com/neovide/neovide/blob/main/src/windows_utils.rs .
use super::UserPreferences;
use windows_registry::{Key, CURRENT_USER};
use windows_result::HRESULT;

pub struct RegistryBackedPreferences {
    app_key_path: String,
    /// Cache the `HKCU\Software\Waz\<channel>` registry Key handle.
    ///
    /// When Waz starts, it calls `read_value` for ~100 settings sequentially.
    /// Opening/creating the Key via `CURRENT_USER.create(...)` each time is a synchronous system call taking ~3ms,
    /// accumulating to 300ms+ (taking up the bulk of the cold startup `READ_USER_DEFAULTS_AND_INITIALIZE_SETTINGS` phase).
    /// Here, we cache the successfully opened Key on the first attempt, and subsequent reads directly reuse it, saving N-1 system calls.
    ///
    /// We use `Mutex<Option<Key>>` instead of `OnceLock` because `windows_registry::Key`
    /// does not implement `Clone`, requiring a mutable lock to `replace`/`take`; meanwhile, the `read_value` interface is
    /// `&self`, making `RefCell` unusable (Sync is required).
    cached_key: Mutex<Option<Key>>,
}

static WARP_REGISTRY_BASE_PATH: &str = "Software\\Waz\\";
pub const KEY_NOT_FOUND_ERR: HRESULT = HRESULT::from_win32(0x80070002);

impl RegistryBackedPreferences {
    /// Construct a separate registry path for each channel (stable, dev, local, etc.)
    pub fn new(app_name: &str) -> Self {
        let app_key_path = WARP_REGISTRY_BASE_PATH.to_owned() + app_name;
        // Prewarm the Key during startup to avoid synchronous system calls for the first setting read as well.
        // Prewarming failure is not an error: `with_warp_registry` will retry when needed.
        let initial_key = CURRENT_USER
            .create(app_key_path.clone())
            .inspect_err(|e| {
                log::warn!("warp registry key prewarm failed (will retry on first access): {e:#}");
            })
            .ok();
        Self {
            app_key_path,
            cached_key: Mutex::new(initial_key),
        }
    }

    /// Operate on the cached Waz registry Key using a callback. The first invocation will perform `CURRENT_USER.create(...)`,
    /// and subsequent calls will reuse it directly. If the Key lock is poisoned (due to a previous panic), fallback to one-off creation
    /// without caching — behavior degrades gracefully without causing further panics.
    fn with_warp_registry<R>(
        &self,
        f: impl FnOnce(&Key) -> Result<R, super::Error>,
    ) -> Result<R, super::Error> {
        let mut guard = match self.cached_key.lock() {
            Ok(g) => g,
            // Mutex poisoned: fallback to the one-off create path without caching, equivalent to the original behavior.
            Err(_) => {
                let key = CURRENT_USER
                    .create(self.app_key_path.clone())
                    .map_err(|e| {
                        log::error!("unable to access Waz app key in Windows Registry: {e:#}");
                        super::Error::IoError(io::Error::from(e))
                    })?;
                return f(&key);
            }
        };

        if guard.is_none() {
            let key = CURRENT_USER
                .create(self.app_key_path.clone())
                .map_err(|e| {
                    log::error!("unable to access Waz app key in Windows Registry: {e:#}");
                    super::Error::IoError(io::Error::from(e))
                })?;
            *guard = Some(key);
        }

        // At this point, guard is guaranteed to be Some; unwrap is safe.
        f(guard.as_ref().expect("cached_key must be Some after init"))
    }
}

impl UserPreferences for RegistryBackedPreferences {
    fn read_value(&self, name: &str) -> Result<Option<String>, super::Error> {
        self.with_warp_registry(|key| Ok(key.get_string(name).ok()))
    }

    fn write_value(&self, key: &str, value: String) -> Result<(), super::Error> {
        self.with_warp_registry(|reg| {
            reg.set_string(key, value.as_str())
                .map_err(|e| super::Error::from(io::Error::from(e)))
        })
    }

    fn remove_value(&self, key: &str) -> Result<(), super::Error> {
        self.with_warp_registry(|reg| match reg.remove_value(key) {
            Ok(_) => Ok(()),
            // If the key doesn't exist, then treat removal of that nonexistent key as a success.
            Err(e) if e.code() == KEY_NOT_FOUND_ERR => Ok(()),
            Err(e) => Err(super::Error::from(io::Error::from(e))),
        })
    }
}
