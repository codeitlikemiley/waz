//! SSH-specific implementation of [`RemoteTransport`].
//!
//! [`SshTransport`] uses an existing SSH ControlMaster socket to check/install
//! the remote server binary and to launch the `remote-server-proxy` process
//! whose stdin/stdout become the protocol channel.
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use warpui::r#async::{executor, FutureExt as _};

use remote_server::auth::RemoteServerAuthContext;
use remote_server::client::RemoteServerClient;
use remote_server::setup::{
    parse_uname_output, remote_server_daemon_dir, PreinstallCheckResult, RemotePlatform,
};
use remote_server::ssh::ssh_args;
use remote_server::transport::{Connection, RemoteTransport};

/// SSH transport: connects via a ControlMaster socket.
///
/// `socket_path` is the local Unix socket created by the ControlMaster
/// process (`ssh -N -o ControlMaster=yes -o ControlPath=<path>`). All SSH
/// commands (binary check, install, proxy launch) are multiplexed through
/// this socket without re-authenticating.
#[derive(Clone)]
pub struct SshTransport {
    socket_path: PathBuf,
    auth_context: Arc<RemoteServerAuthContext>,
}

impl fmt::Debug for SshTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshTransport")
            .field("socket_path", &self.socket_path)
            .finish_non_exhaustive()
    }
}

impl SshTransport {
    pub fn new(socket_path: PathBuf, auth_context: Arc<RemoteServerAuthContext>) -> Self {
        Self {
            socket_path,
            auth_context,
        }
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    pub fn remote_daemon_socket_path(&self) -> String {
        format!(
            "{}/server.sock",
            remote_server_daemon_dir(&self.auth_context.remote_server_identity_key())
        )
    }

    pub fn remote_daemon_pid_path(&self) -> String {
        format!(
            "{}/server.pid",
            remote_server_daemon_dir(&self.auth_context.remote_server_identity_key())
        )
    }

    fn remote_proxy_command(&self) -> String {
        let binary = remote_server::setup::remote_server_binary();
        let identity_key = self.auth_context.remote_server_identity_key();
        let quoted_identity_key = shell_words::quote(&identity_key);
        format!("{binary} remote-server-proxy --identity-key {quoted_identity_key}")
    }
}

#[derive(Debug)]
enum InstallError {
    ScriptFailed { exit_code: i32, stderr: String },
    Other(anyhow::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScriptFailed { exit_code, stderr } => {
                write!(f, "install script failed (exit {exit_code}): {stderr}")
            }
            Self::Other(error) => write!(f, "{error:#}"),
        }
    }
}

impl From<anyhow::Error> for InstallError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

async fn detect_remote_platform(socket_path: &Path) -> Result<RemotePlatform> {
    let output = remote_server::ssh::run_ssh_command(
        socket_path,
        "uname -sm",
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return parse_uname_output(&stdout);
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!("uname -sm exited with code {code}: {stderr}"))
}

async fn verify_installed_binary(socket_path: &Path) -> Result<()> {
    let output = remote_server::ssh::run_ssh_command(
        socket_path,
        &remote_server::setup::binary_check_command(),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;

    if output.status.success() {
        return Ok(());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "installed binary check failed with code {code}: {stderr}"
    ))
}

async fn run_install_script(
    socket_path: &Path,
    staging_tarball_path: Option<&str>,
    timeout: std::time::Duration,
) -> core::result::Result<(), InstallError> {
    let script = remote_server::setup::install_script(staging_tarball_path);
    match remote_server::ssh::run_ssh_script(socket_path, &script, timeout).await {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            Err(InstallError::ScriptFailed { exit_code, stderr })
        }
        Err(error) => Err(InstallError::Other(error)),
    }
}

fn should_skip_scp_fallback(error: &InstallError) -> bool {
    matches!(error, InstallError::ScriptFailed { exit_code: 2, .. })
}

// ===========================================================================
// Waz fork: development mode remote-server installation path
//
// The upstream/release build will cause the remote install script to download the precompiled version from GitHub releases
// remote-server binary. But when building from local source code (`cargo run`), this will be downloaded to
// The "latest released" stale binary, rather than the code just modified by the developer, makes it impossible to
// Debugging changes to remote-server.
//
// Therefore, under the source code build with DEBUG and no release tag (see
// `remote_server::setup::is_dev_source_build()`), `install_binary()` changed to:
//   1. Cross-compile the `warp` binary locally to x86_64 musl(profile/features and
//      `script/deploy_remote_server` is exactly the same);
//   2. Use `scp_upload` to upload the product to the existing SSH ControlMaster socket.
//      `remote_server::setup::remote_server_binary()` resolves the remote path;
//   3. Skip the GitHub download and installation script entirely.
//
// If the cross-compilation preconditions are missing (musl target is not installed or musl linker is not installed), it will not
// Hard failure, but print a clear warning and fall back to the original download and installation process to ensure that dev is still available.
// ===========================================================================

/// Possible musl linker candidates for development mode cross-compiling (in order of priority).
/// On macOS, it is usually `x86_64-linux-musl-gcc`(filosottile/musl-cross),
/// Commonly used on Linux is `musl-gcc`.
const DEV_MUSL_LINKER_CANDIDATES: &[&str] = &["x86_64-linux-musl-gcc", "musl-gcc"];

/// Return to the current workspace root directory.
///
/// `ssh_transport.rs` belongs to `app` crate, `CARGO_MANIFEST_DIR` points to
/// `<workspace>/app`, its parent directory is the workspace root.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        // Theoretically, `app` must have a parent directory; if not, it will fall back to the manifest directory itself.
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// Returns PATH with `~/.cargo/bin` (and `$CARGO_HOME/bin`) appended.
///
/// The warp process is often started by the desktop environment or system `cargo`, and its PATH may only contain `/usr/bin`
/// Without `~/.cargo/bin`. This results in:
///   - `cargo zigbuild` cannot find `cargo-zigbuild` subcommand → fallback to musl-gcc;
///   - cargo-zigbuild itself cannot find `cargo` / `rustc`.
/// Cross-compilation related sub-processes use the PATH returned here to ensure that both can be parsed.
/// If no adjustment is required (no HOME / cannot be spliced), `None` is returned, and the caller continues to use the inherited PATH.
fn dev_build_path_env() -> Option<std::ffi::OsString> {
    let mut extra: Vec<PathBuf> = Vec::new();
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        extra.push(PathBuf::from(cargo_home).join("bin"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        extra.push(PathBuf::from(home).join(".cargo").join("bin"));
    }
    if extra.is_empty() {
        return None;
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    extra.extend(std::env::split_paths(&current));
    std::env::join_paths(extra).ok()
}

/// Search for the first available musl linker in `PATH` and return `None` if not found.
fn find_musl_linker() -> Option<&'static str> {
    DEV_MUSL_LINKER_CANDIDATES.iter().copied().find(|linker| {
        command::blocking::Command::new(linker)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

/// dev The build backend used for cross-compilation.
enum DevBuildBackend {
    /// `cargo zigbuild`: zig acts as a complete C/C++ musl cross tool chain, no separate installation is required
    /// `*-musl-gcc` / `*-musl-g++`, can correctly compile `freetype-sys` and other C/C++ source codes
    /// dependence. This is the preferred backend.
    Zigbuild,
    /// Native `cargo build` + musl linker. Only available if the system has the full musl C/C++ cross
    /// Reliable only when using the toolchain - only `*-musl-gcc`, when `*-musl-g++` is missing, `freetype-sys`
    /// Such C++ dependencies will fail to compile.
    MuslGcc(&'static str),
}

/// Check whether `cargo-zigbuild` is available.
///
/// Detect `cargo-zigbuild --version` (the binary itself) directly instead of
/// `cargo zigbuild --version` - the latter will be parsed as an unknown parameter by the `zigbuild` subcommand
/// And failed. The PATH used for detection is the same as the actual build (injected `~/.cargo/bin`).
fn cargo_zigbuild_available() -> bool {
    let mut cmd = command::blocking::Command::new("cargo-zigbuild");
    cmd.arg("--version");
    if let Some(path) = dev_build_path_env() {
        cmd.env("PATH", path);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Select dev cross-compilation backend: give priority to `cargo zigbuild`, fall back to native `cargo build`
/// + musl linker. Returns `None` when neither is available, and the caller falls back to download and installation.
fn select_dev_build_backend() -> Option<DevBuildBackend> {
    if cargo_zigbuild_available() {
        return Some(DevBuildBackend::Zigbuild);
    }
    find_musl_linker().map(DevBuildBackend::MuslGcc)
}

/// Check if the `x86_64-unknown-linux-musl` target is installed via rustup.
async fn musl_target_installed() -> bool {
    let output = command::r#async::Command::new("rustup")
        .arg("target")
        .arg("list")
        .arg("--installed")
        .kill_on_drop(true)
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == remote_server::setup::DEV_MUSL_TARGET),
        // When the rustup output cannot be obtained, it is conservatively considered not installed, thus triggering a rollback.
        _ => false,
    }
}

/// Cross-compile the local `warp` binary to musl, returning the product path.
///
/// profile/features align with `script/deploy_remote_server`.
async fn cross_compile_remote_server(backend: &DevBuildBackend) -> Result<PathBuf> {
    let root = workspace_root();
    // The `[[bin]]` name corresponding to the current channel - the OSS fork is `warp-oss` (see app/Cargo.toml).
    // You cannot hardcode `warp`: The bin of `warp` should be `load_config!("local")`, which needs to be private.
    // `warp-channel-config` can generate `local_config.json`, OSS fork will fail to compile without it;
    // `warp-oss`(src/bin/oss.rs) inline `ChannelConfig`, no such dependency.
    let bin_name = remote_server::setup::binary_name();
    let backend_desc = match backend {
        DevBuildBackend::Zigbuild => "cargo-zigbuild".to_string(),
        DevBuildBackend::MuslGcc(linker) => format!("cargo-build/{linker}"),
    };
    log::info!(
        "dev remote-server: cross-compiling {bin_name} -> {} (profile={}, backend={backend_desc})",
        remote_server::setup::DEV_MUSL_TARGET,
        remote_server::setup::DEV_REMOTE_PROFILE,
    );
    // The entire warp is compiled for the first time, which usually takes several minutes. stdout/stderr directly inherit to run
    // Waz's terminal, so developers can see the real-time compilation progress of cargo (otherwise the whole process is silent,
    // It is easy to mistakenly think it is stuck).
    log::info!(
        "dev remote-server: cross-compiling, the first time usually takes a few minutes —— cargo progress will be printed to \
         the terminal running Waz"
    );

    let status = async {
        let mut cmd = command::r#async::Command::new("cargo");
        cmd.current_dir(&root);
        // Inject `~/.cargo/bin` to ensure that `cargo zigbuild` can parse `cargo-zigbuild`
        // subcommand, and cargo-zigbuild can find `cargo` / `rustc`.
        if let Some(path) = dev_build_path_env() {
            cmd.env("PATH", path);
        }
        match backend {
            // zigbuild is a cargo subcommand that comes with a zig linker and C/C++ cross-compiler.
            // No need to set up LINKER env anymore.
            DevBuildBackend::Zigbuild => {
                cmd.arg("zigbuild");
            }
            // Native cargo build: specify musl linker via env and override rustflags,
            // Avoid macOS-specific flags in .cargo/config.toml from contaminating cross-compilation.
            DevBuildBackend::MuslGcc(linker) => {
                cmd.arg("build")
                    .env("CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER", *linker)
                    .env(
                        "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS",
                        "-C symbol-mangling-version=v0",
                    );
            }
        }
        cmd.arg("-p")
            .arg("warp")
            .arg("--bin")
            .arg(bin_name)
            .arg("--target")
            .arg(remote_server::setup::DEV_MUSL_TARGET)
            .arg("--profile")
            .arg(remote_server::setup::DEV_REMOTE_PROFILE)
            .arg("--features")
            .arg(remote_server::setup::DEV_REMOTE_FEATURES)
            // inherit: Transmit the real-time progress of cargo to the terminal instead of silently buffering the entire process.
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .status()
            .await
    }
    .with_timeout(remote_server::setup::DEV_CROSS_COMPILE_TIMEOUT)
    .await
    .map_err(|_| {
        anyhow!(
            "dev remote-server cross-compilation timed out (>{:?})",
            remote_server::setup::DEV_CROSS_COMPILE_TIMEOUT
        )
    })?
    .map_err(|e| anyhow!("Failed to start cargo build: {e}"))?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(anyhow!(
            "cargo cross-compilation failed (exit {code}), see cargo output in the terminal running Waz"
        ));
    }

    // Product location: `<target_dir>/<triple>/<profile>/<bin_name>`.
    // Read `CARGO_TARGET_DIR` first, otherwise fall back to `<workspace>/target`. The warehouse is not there
    // `.cargo/config.toml` contains `[build] target-dir`, so only env needs to be considered.
    let target_root = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let binary = target_root
        .join(remote_server::setup::DEV_MUSL_TARGET)
        .join(remote_server::setup::DEV_REMOTE_PROFILE)
        .join(bin_name);
    if !binary.is_file() {
        return Err(anyhow!(
            "Cross-compilation finished but artifact not found at {} (if CARGO_TARGET_DIR is set, please confirm the path)",
            binary.display()
        ));
    }
    Ok(binary)
}

/// Development mode installation: cross-compile the local `warp` and upload it to the remote remote-server path.
///
/// The upload target is exactly the same as `remote_server_binary()`, ensuring that subsequent
/// `check_binary()` / Proxy startup can find it.
async fn dev_install_local_binary(socket_path: &Path) -> Result<()> {
    // Precondition check: If any item is missing, an error will be returned, and the caller will fall back to download and installation.
    if !musl_target_installed().await {
        return Err(anyhow!(
            "Rust target {} is not installed; you can run `rustup target add {}`",
            remote_server::setup::DEV_MUSL_TARGET,
            remote_server::setup::DEV_MUSL_TARGET,
        ));
    }
    // Select the cross-compilation backend: give priority to `cargo zigbuild` (zig comes with a complete C/C++ musl tool chain,
    // Can compile freetype-sys and other C++ dependencies), otherwise fall back to musl-gcc. If neither is found, an error will be reported.
    let backend = select_dev_build_backend().ok_or_else(|| {
        anyhow!(
            "No available musl cross-compilation backend found. It is recommended to install cargo-zigbuild + zig \
             (`cargo install cargo-zigbuild`, and install `zig` using package manager), \
             or install the full musl C/C++ cross toolchain ({})",
            DEV_MUSL_LINKER_CANDIDATES.join(" / ")
        )
    })?;

    let local_binary = cross_compile_remote_server(&backend).await?;

    // Upload to the precise path parsed by `remote_server_binary()`, first create the parent directory.
    let remote_binary = remote_server::setup::remote_server_binary();
    let remote_dir = remote_server::setup::remote_server_dir();
    let mkdir_output = remote_server::ssh::run_ssh_command(
        socket_path,
        &format!("mkdir -p {remote_dir}"),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if !mkdir_output.status.success() {
        let code = mkdir_output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
        return Err(anyhow!(
            "Remote remote-server directory creation failed (exit {code}): {stderr}"
        ));
    }

    log::info!("dev remote-server: uploading local cross-compiled artifact to {remote_binary} (scp -C compressed, hundreds of MB may take a few minutes)");
    // The dev product is hundreds of MB, use DEV_UPLOAD_TIMEOUT (much more than SCP_INSTALL_TIMEOUT),
    // Avoid large file uploads being interrupted by a 120s timeout and then falling back to downloading stale releases.
    remote_server::ssh::scp_upload(
        socket_path,
        &local_binary,
        &remote_binary,
        remote_server::setup::DEV_UPLOAD_TIMEOUT,
    )
    .await?;

    // Grant executable permissions.
    let chmod_output = remote_server::ssh::run_ssh_command(
        socket_path,
        &format!("chmod 755 {remote_binary}"),
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;
    if !chmod_output.status.success() {
        let code = chmod_output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&chmod_output.stderr);
        return Err(anyhow!("Remote chmod failed (exit {code}): {stderr}"));
    }

    // Reuse existing verification logic to confirm that the uploaded binary is executable.
    verify_installed_binary(socket_path).await
}

async fn download_remote_server_tarball(download_url: &str, tarball_path: &Path) -> Result<()> {
    let output = async {
        command::r#async::Command::new("curl")
            .arg("-fSL")
            .arg("--connect-timeout")
            .arg("15")
            .arg(download_url)
            .arg("-o")
            .arg(tarball_path.as_os_str())
            .kill_on_drop(true)
            .output()
            .await
    }
    .with_timeout(remote_server::setup::SCP_INSTALL_TIMEOUT)
    .await
    .map_err(|_| {
        anyhow!(
            "local tarball download timed out after {:?}",
            remote_server::setup::SCP_INSTALL_TIMEOUT
        )
    })?
    .map_err(|e| anyhow!("local curl failed to execute: {e}"))?;

    if output.status.success() {
        return Ok(());
    }

    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "local tarball download failed with code {code}: {stderr}"
    ))
}

async fn scp_install_fallback(socket_path: &Path) -> Result<()> {
    let platform = detect_remote_platform(socket_path).await?;
    let download_url = remote_server::setup::download_tarball_url(&platform);
    let remote_server_dir = remote_server::setup::remote_server_dir();
    let mkdir_cmd = format!("mkdir -p {remote_server_dir}");
    let mkdir_output = remote_server::ssh::run_ssh_command(
        socket_path,
        &mkdir_cmd,
        remote_server::setup::CHECK_TIMEOUT,
    )
    .await?;

    if !mkdir_output.status.success() {
        let code = mkdir_output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&mkdir_output.stderr);
        return Err(anyhow!(
            "remote-server dir creation failed with code {code}: {stderr}"
        ));
    }

    let tempdir = tempfile::tempdir()?;
    let tarball_path = tempdir.path().join("waz.tar.gz");
    download_remote_server_tarball(&download_url, &tarball_path).await?;

    let remote_tarball_path = format!("{remote_server_dir}/waz-upload.tar.gz");
    remote_server::ssh::scp_upload(
        socket_path,
        &tarball_path,
        &remote_tarball_path,
        remote_server::setup::SCP_INSTALL_TIMEOUT,
    )
    .await?;

    run_install_script(
        socket_path,
        Some(&remote_tarball_path),
        remote_server::setup::SCP_INSTALL_TIMEOUT,
    )
    .await
    .map_err(|error| anyhow!("staged install failed: {error}"))?;

    verify_installed_binary(socket_path).await
}

impl RemoteTransport for SshTransport {
    fn detect_platform(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<RemotePlatform, String>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            detect_remote_platform(&socket_path)
                .await
                .map_err(|e| format!("{e:#}"))
        })
    }

    fn run_preinstall_check(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PreinstallCheckResult, String>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            match remote_server::ssh::run_ssh_script(
                &socket_path,
                remote_server::setup::PREINSTALL_CHECK_SCRIPT,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await
            {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    Ok(PreinstallCheckResult::parse(&stdout))
                }
                Ok(output) => {
                    let code = output.status.code().unwrap_or(-1);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!(
                        "Preinstall check exited with code {code}: {stderr}"
                    ))
                }
                Err(e) => Err(format!("{e:#}")),
            }
        })
    }

    fn check_binary(&self) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            let bin_path = remote_server::setup::remote_server_binary();
            log::info!("Checking for remote server binary at {bin_path}");
            match remote_server::ssh::run_ssh_command(
                &socket_path,
                &remote_server::setup::binary_check_command(),
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await
            {
                // `{binary} --version` exits 0 if it exists and is runnable.
                // 126/127 means missing or unexecutable; other non-zero exits are treated as truth check failures.
                Ok(output) => match output.status.code() {
                    Some(0) => Ok(true),
                    Some(126) | Some(127) => Ok(false),
                    Some(code) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        Err(format!("binary check exited with code {code}: {stderr}"))
                    }
                    None => Err("binary check terminated by signal".into()),
                },
                Err(e) => Err(format!("{e:#}")),
            }
        })
    }

    fn check_has_old_binary(&self) -> Pin<Box<dyn Future<Output = anyhow::Result<bool>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            // Treat the existence of the remote-server install directory
            // itself as evidence of a prior install. If `~/.warp-XX/remote-server`
            // exists, something was installed there before, so any mismatch
            // with the client's expected binary path should be auto-updated
            // rather than surfaced as a first-time install prompt.
            let cmd = format!("test -d {}", remote_server::setup::remote_server_dir());
            let output = remote_server::ssh::run_ssh_command(
                &socket_path,
                &cmd,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await?;
            // `test -d` exits 0 when present, 1 when missing.
            // Anything else is treated as a check failure.
            match output.status.code() {
                Some(0) => Ok(true),
                Some(1) => Ok(false),
                Some(code) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(anyhow::anyhow!(
                        "remote-server dir check exited with code {code}: {stderr}"
                    ))
                }
                None => Err(anyhow::anyhow!(
                    "remote-server dir check terminated by signal"
                )),
            }
        })
    }

    fn install_binary(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            log::info!(
                "Installing remote server binary to {}",
                remote_server::setup::remote_server_binary()
            );

            // Waz fork:DEBUG source code build (without release tag) in development mode,
            // Cross-compile a local `warp` and upload it instead of downloading the stale GitHub release.
            // In case of failure (missing cross-compilation preconditions, etc.), a warning will be printed and fall back to download and installation.
            // Ensure that the dev experience is not broken. The release build skips the entire logic and the behavior remains unchanged.
            if remote_server::setup::is_dev_source_build() {
                log::info!("dev remote-server: DEBUG source build detected, switching to local cross-compiled installation");
                match dev_install_local_binary(&socket_path).await {
                    Ok(()) => return Ok(()),
                    Err(error) => {
                        log::warn!(
                            "dev remote-server: Local cross-compiled installation unavailable, falling back to download installation: {error:#}"
                        );
                        // If failed, continue with the regular download and installation process below.
                    }
                }
            }

            match run_install_script(&socket_path, None, remote_server::setup::INSTALL_TIMEOUT)
                .await
            {
                Ok(()) => verify_installed_binary(&socket_path)
                    .await
                    .map_err(|error| format!("{error:#}")),
                Err(error) if should_skip_scp_fallback(&error) => Err(error.to_string()),
                Err(error) => {
                    log::warn!("remote-server install failed, trying SCP fallback: {error}");
                    match scp_install_fallback(&socket_path).await {
                        Ok(()) => Ok(()),
                        Err(fallback_error) => {
                            Err(format!("{error}; SCP fallback failed: {fallback_error:#}"))
                        }
                    }
                }
            }
        })
    }

    fn connect(
        &self,
        executor: Arc<executor::Background>,
    ) -> Pin<Box<dyn Future<Output = Result<Connection>> + Send>> {
        let socket_path = self.socket_path.clone();
        let remote_proxy_command = self.remote_proxy_command();
        Box::pin(async move {
            let mut args = ssh_args(&socket_path);
            args.push(remote_proxy_command);

            // `kill_on_drop(true)` pairs with ownership of the `Child` being
            // returned in the [`Connection`] below: the
            // [`RemoteServerManager`] holds the `Child` on its per-session
            // state, and dropping that state (on explicit teardown or
            // spontaneous disconnect) sends SIGKILL to this ssh process.
            let mut child = command::r#async::Command::new("ssh")
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;

            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to capture child stdin"))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to capture child stdout"))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| anyhow::anyhow!("Failed to capture child stderr"))?;

            let (client, event_rx) =
                RemoteServerClient::from_child_streams(stdin, stdout, stderr, &executor);
            Ok(Connection {
                client,
                event_rx,
                child,
                control_path: Some(socket_path),
            })
        })
    }

    fn remove_remote_server_binary(
        &self,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>> {
        let socket_path = self.socket_path.clone();
        Box::pin(async move {
            let cmd = format!("rm -f {}", remote_server::setup::remote_server_binary());
            log::info!("Removing stale remote server binary: {cmd}");
            let output = remote_server::ssh::run_ssh_command(
                &socket_path,
                &cmd,
                remote_server::setup::CHECK_TIMEOUT,
            )
            .await?;
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(anyhow::anyhow!("Failed to remove binary: {stderr}"))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use warpui::r#async::BoxFuture;
    fn static_auth_context() -> Arc<RemoteServerAuthContext> {
        Arc::new(RemoteServerAuthContext::new(
            || -> BoxFuture<'static, Option<String>> { Box::pin(async { None }) },
            || "user id/with spaces".to_string(),
        ))
    }

    #[test]
    fn remote_proxy_command_quotes_identity_key() {
        let transport = SshTransport::new(
            PathBuf::from("/tmp/control-master.sock"),
            static_auth_context(),
        );

        let command = transport.remote_proxy_command();

        assert!(command.contains("remote-server-proxy --identity-key"));
        assert!(command.contains("'user id/with spaces'"));
    }
}
