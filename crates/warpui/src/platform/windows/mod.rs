use itertools::Itertools as _;
use std::os::windows::ffi::OsStrExt as _;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

// Re-export a couple winit types and modules as the concrete implementations
// for Windows.
pub use crate::windowing::winit::app::App;

pub(crate) static DXC_PATH: std::sync::OnceLock<Option<DXCPath>> = std::sync::OnceLock::new();

/// Path to the DXC DLLs to be used to compile DirectX shaders using DXC.
/// See https://github.com/microsoft/DirectXShaderCompiler.
#[derive(Debug)]
pub struct DXCPath {
    pub dxc_path: String,
    pub dxil_path: String,
}

pub trait AppBuilderExt {
    /// Set the AppUserModel ID, which Windows uses to attribute notifications to
    /// our correct application.
    fn set_app_user_model_id(&mut self, app_id: String);

    /// Use DXC (the newer DirectX Shader Compiler) to compile DirectX shaders.
    /// Using DXC requires the dlls within [`DXCPath`] to be available and shipped
    /// alongside the application.=
    fn use_dxc_for_directx_shader_compilation(&mut self, dxc_path: DXCPath);
}

impl AppBuilderExt for super::AppBuilder {
    fn set_app_user_model_id(&mut self, app_id: String) {
        // First register AUMID to HKCU\Software\Classes\AppUserModelId\<aumid>,
        // so that even without a Start Menu shortcut (`cargo run` dev mode / unpackaged version),
        // the Windows ToastNotificationManager can still find the AUMID, allowing the Toast to pop up.
        // Otherwise, `Toast::show()` will be silently swallowed by the system layer without the API reporting an error.
        // Ref: https://learn.microsoft.com/en-us/windows/apps/design/shell/tiles-and-notifications/send-local-toast-other-apps
        if let Err(err) = register_aumid_in_registry(&app_id) {
            log::warn!("Unable to register Windows AppUserModel ID in registry: {err:?}");
        }

        let set_id = unsafe { set_app_user_model_id(app_id) };
        if let Err(err) = set_id {
            log::error!("Unable to set Windows AppUserModel ID: {err:?}");
        }
    }

    fn use_dxc_for_directx_shader_compilation(&mut self, dxc_path: DXCPath) {
        if let Err(e) = DXC_PATH.set(Some(dxc_path)) {
            log::warn!("Failed to set DXC path {e:?}");
        }
    }
}

unsafe fn set_app_user_model_id(app_id: String) -> Result<(), windows::core::Error> {
    let wide_string = std::ffi::OsStr::new(&app_id)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect_vec();
    windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(windows::core::PCWSTR(
        wide_string.as_ptr(),
    ))
}

/// Registers AUMID to `HKCU\Software\Classes\AppUserModelId\<aumid>`,
/// which is the official registry path for Windows 10/11 "unpackaged apps" to send local toasts.
///
/// `DisplayName` determines the source name displayed at the top of the toast; `IconBackgroundColor` makes
/// Windows use a cleaner solid background instead of the default grey background. Icon is temporarily not written (requires an absolute path,
/// and the paths differ between `cargo run` and formal installation, which is left for the installer to handle).
fn register_aumid_in_registry(app_id: &str) -> std::io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let subkey = format!("Software\\Classes\\AppUserModelId\\{app_id}");
    let (key, _) = hkcu.create_subkey(&subkey)?;

    // Derives a decent display name from the last segment of AUMID (e.g. dev.waz.Waz -> Waz).
    let display_name = app_id.rsplit('.').next().unwrap_or(app_id);
    key.set_value("DisplayName", &display_name.to_string())?;
    Ok(())
}
