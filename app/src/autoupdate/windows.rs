use crate::server::telemetry::TelemetryEvent;
use anyhow::anyhow;
use anyhow::{bail, Result};
use channel_versions::VersionInfo;
use command::blocking::Command;
use lazy_static::lazy_static;
use parking_lot::Mutex;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::{fs, io};
use std::{io::Write as _, time::Duration};
use tempfile::TempPath;
use warp_core::channel::{Channel, ChannelState};
use warpui::AppContext;

use super::{
    github, release_assets_directory_url, DownloadProgress, DownloadReady, ProgressCallback,
};
use crate::util::windows::install_dir;

lazy_static! {
    /// The path to the temporary file that stores the installer for the new update.
    static ref INSTALLER_PATH: Arc<Mutex<Option<TempPath>>> = Default::default();
}

/// Download the Inno Setup install wizard, the same one users run on the first Waz install, and
/// place it into the "data dir".
pub(super) async fn download_update_and_cleanup(
    version_info: &VersionInfo,
    _update_id: &str,
    client: &http_client::Client,
    on_progress: ProgressCallback,
) -> Result<DownloadReady> {
    use futures::StreamExt as _;
    use instant::Instant;
    const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

    let channel = ChannelState::channel();
    let installer_file_name = installer_file_name()?;
    // openWarp: Retrieve the actual download URL from GitHub Release cache (asset name is WazSetup.exe /
    // WazSetup-arm64.exe, see installer_file_name()). Other channels use the official base URL.
    let url = if matches!(channel, Channel::Oss) {
        if let Some(release) = github::cached_release() {
            if let Some(found) = release.find_asset(&installer_file_name) {
                found.browser_download_url.clone()
            } else {
                log::warn!(
                    "openWarp: cached release tag {} has no asset named {installer_file_name}, falling back to tag URL",
                    release.tag_name
                );
                format!(
                    "https://github.com/codeitlikemiley/waz/releases/download/v{}/{installer_file_name}",
                    version_info.version
                )
            }
        } else {
            format!(
                "https://github.com/codeitlikemiley/waz/releases/download/v{}/{installer_file_name}",
                version_info.version
            )
        }
    } else {
        format!(
            "{}/{}",
            release_assets_directory_url(channel, &version_info.version),
            installer_file_name
        )
    };

    // Create a temporary file that we'll write the download into.
    let mut already_exists = false;
    let mut new_installer = tempfile::Builder::new()
        .rand_bytes(0)
        .suffix(&format!("{}-{}", version_info.version, installer_file_name))
        .make(|path| {
            already_exists = path.is_file();
            if already_exists {
                File::open(path)
            } else {
                File::create(path)
            }
        })?;

    if !already_exists {
        log::info!("Downloading {url} to {}...", new_installer.path().display());

        let response = client
            .get(&url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await?
            .error_for_status()?;

        let total = response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        on_progress(DownloadProgress {
            downloaded: 0,
            total,
        });

        let mut downloaded: u64 = 0;
        let mut last_reported = 0u64;
        let mut last_reported_at = Instant::now();
        const REPORT_BYTES_THRESHOLD: u64 = 64 * 1024;
        const REPORT_TIME_THRESHOLD: Duration = Duration::from_millis(250);

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            new_installer.as_file_mut().write_all(&chunk)?;
            downloaded += chunk.len() as u64;
            if downloaded - last_reported >= REPORT_BYTES_THRESHOLD
                || last_reported_at.elapsed() >= REPORT_TIME_THRESHOLD
            {
                on_progress(DownloadProgress {
                    downloaded,
                    total,
                });
                last_reported = downloaded;
                last_reported_at = Instant::now();
            }
        }
        on_progress(DownloadProgress {
            downloaded,
            total,
        });
    } else {
        // Reuse the previously downloaded installer with the same name: do not initiate a new request, only send one progress report
        // so that the UI directly displays 100%.
        let downloaded = new_installer
            .as_file_mut()
            .metadata()
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);
        on_progress(DownloadProgress {
            downloaded,
            total: Some(downloaded),
        });
    }

    // openWarp: Verify SHA-256 in the GitHub Release metadata to protect against CDN MITM/corruption.
    // If verification fails, return Err directly; the installer temporary file will subsequently be cleaned up by TempPath drop;
    // here we deliberately do not put it into INSTALLER_PATH (otherwise subsequent relaunch() might misuse it).
    if matches!(channel, Channel::Oss) {
        let temp_path = new_installer.path().to_path_buf();
        if let Err(e) = super::verify_oss_asset_sha256(&temp_path, &installer_file_name) {
            return Err(e);
        }
    }

    *INSTALLER_PATH.lock() = Some(new_installer.into_temp_path());

    Ok(DownloadReady::Yes)
}

const UPDATE_LOG_FILENAME: &str = "warp_update.log";

fn autoupdate_log_file() -> Result<PathBuf> {
    warp_logging::log_directory().map(|dir| dir.join(UPDATE_LOG_FILENAME))
}

/// Checks the autoupdate log file from a previous update attempt.
/// Records known issues found during a previous update attempt.
/// The log file is renamed after processing to avoid duplicate reports on subsequent launches.
pub(super) fn check_and_report_update_errors(ctx: &mut AppContext) {
    let log_path = match autoupdate_log_file() {
        Ok(path) => path,
        Err(e) => {
            log::warn!("Failed to determine autoupdate log file path: {e:#}");
            return;
        }
    };

    // Inno Setup logs use the system's active codepage (often Windows-1252), not UTF-8.
    // We read as raw bytes to avoid silently skipping non-UTF-8 log files.
    let contents = match fs::read(&log_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            log::info!("No autoupdate logs found");
            return;
        }
        Err(e) => {
            log::warn!("Failed to read autoupdate log file: {e:#}");
            return;
        }
    };

    let contents_lowercase = contents.to_ascii_lowercase();

    let has_unable_to_close = memchr::memmem::find(
        &contents_lowercase,
        b"setup was unable to automatically close all applications",
    )
    .is_some();
    if has_unable_to_close {
        crate::send_telemetry_sync_from_app_ctx!(
            TelemetryEvent::AutoupdateUnableToCloseApplications,
            ctx
        );
    }

    let has_file_in_use = memchr::memmem::find(
        &contents_lowercase,
        b"the process cannot access the file because it is being used by another process",
    )
    .is_some();
    if has_file_in_use {
        crate::send_telemetry_sync_from_app_ctx!(TelemetryEvent::AutoupdateFileInUse, ctx);
    }

    // Fired when the mutex polling loop timed out and a force-kill was attempted.
    let has_mutex_timeout =
        memchr::memmem::find(&contents_lowercase, b"warp mutex still held after timeout").is_some();
    if has_mutex_timeout {
        crate::send_telemetry_sync_from_app_ctx!(TelemetryEvent::AutoupdateMutexTimeout, ctx);
    }

    // Fired when taskkill returned non-zero after the mutex timeout.
    let has_forcekill_failed =
        memchr::memmem::find(&contents_lowercase, b"force-kill failed for").is_some();
    if has_forcekill_failed {
        crate::send_telemetry_sync_from_app_ctx!(TelemetryEvent::AutoupdateForcekillFailed, ctx);
    }

    // openWarp does not upload autoupdate failure logs, it only records error counts locally; the full log file will be renamed
    // and preserved with `.log.reported` below, so users/debugging can view the local file directly if needed.
    #[cfg(feature = "crash_reporting")]
    {
        const IGNOREABLE_ERRORS: &[&[u8]] = &[
            b"there is not enough space on the disk",
            b"setprocessmitigationpolicy failed with error code 87",
            // Bundled skill files whose names contain "error" appear in "Dest filename:" log lines
            // and produce false positives.
            b"error-codes.md",
            b"error-recovery.md",
        ];

        let mut error_count = memchr::memmem::find_iter(&contents_lowercase, b"error").count();
        for pattern in IGNOREABLE_ERRORS {
            let ignoreable_count = memchr::memmem::find_iter(&contents_lowercase, pattern).count();
            error_count = error_count.saturating_sub(ignoreable_count);
        }

        if error_count > 0 {
            log::error!(
                "openWarp: Windows auto-update log contains {error_count} error(s) (log: {:?})",
                log_path
            );
        }
        let _ = &contents;
    }

    // Rename the log file to avoid duplicate reports on subsequent launches.
    // We keep the file around so the user can still view it or attach it to a GitHub issue.
    let reported_path = log_path.with_extension("log.reported");
    if let Err(e) = fs::rename(&log_path, &reported_path) {
        log::warn!("Failed to rename autoupdate log file after reporting: {e:#}");
    }
}

pub(super) fn relaunch() -> Result<()> {
    let channel = ChannelState::channel();

    let install_dir = install_dir()?;
    let Some(installer_path) = INSTALLER_PATH.lock().take() else {
        bail!("No installer path");
    };

    let log_arg = match autoupdate_log_file() {
        Ok(dir) => format!("/LOG={}", dir.display()),
        Err(e) => {
            log::warn!("Failed to determine location for autoupdate logs: {e:#}");
            "/LOG".to_string()
        }
    };

    // openWarp(Channel::Oss): Inno Setup runs in "non-silent" mode. Without /SILENT, the user sees the
    // standard installation interface, allowing them to visually confirm the version number to be installed, the target directory, and cancel via regular UI.
    // Still keep /SP- to skip the "Ready to Install" confirmation popup; /NORESTART to avoid requiring a Windows restart;
    // /update=1 is for the Inno script to detect upgrade mode.
    // /NOCLOSEAPPLICATIONS lets Inno wait for the current Waz process to exit naturally (mutex poll),
    // without forcing RestartManager to kill the process.
    let mut cmd = Command::new(&installer_path);
    if matches!(channel, Channel::Oss) {
        cmd.args([
            "/SP-",
            "/NORESTART",
            &log_arg,
            "/update=1",
            "/NOCLOSEAPPLICATIONS",
            &format!("/DIR={}", install_dir.display()),
        ]);
    } else {
        // Official channel: Maintain the original "silent + progress bar" behavior, automatically install and restart.
        // The Inno Setup install wizard will run without user input. It will re-launch Waz after
        // installing the update files.
        // https://jrsoftware.org/ishelp/index.php?topic=setupcmdline
        cmd.args([
            // Skip asking the user to confirm.
            "/SP-",
            // Do not prompt the user for anything. Note that we do not use "VERYSILENT" so that a
            // progress bar is still shown. This is useful since the update process may take a few
            // seconds.
            "/SILENT",
            // Do not provide a cancel button on the progress bar page.
            "/NOCANCEL",
            // Indicate that restarting Windows is not necessary.
            "/NORESTART",
            &log_arg,
            "/update=1",
            // Do not forcibly kill Waz via RestartManager. The installer will wait for
            // Waz to exit naturally by polling the single-instance mutex instead.
            "/NOCLOSEAPPLICATIONS",
            &format!("/DIR={}", install_dir.display()),
        ]);
    }
    cmd.spawn()?;

    // DEV ONLY: Sleep after spawning the installer so this process is still alive
    // when Inno Setup tries to overwrite files. This reliably reproduces the
    // auto-update race condition (APP-3702) for testing.
    if matches!(channel, Channel::Dev) {
        log::info!("DEV: Sleeping 10s after spawning installer to reproduce update race");
        std::thread::sleep(Duration::from_secs(10));
    }

    Ok(())
}

fn installer_file_name() -> Result<String> {
    let app_name_prefix = app_name_prefix(ChannelState::channel());

    // For example, on arm64 this is WazSetup-arm64.exe and on x64 this is
    // WazSetup.exe.
    if cfg!(target_arch = "aarch64") {
        Ok(format!("{app_name_prefix}Setup-arm64.exe"))
    } else if cfg!(target_arch = "x86_64") {
        Ok(format!("{app_name_prefix}Setup.exe"))
    } else {
        Err(anyhow!(
            "Could not construct setup file name for unsupported architecture"
        ))
    }
}

fn app_name_prefix(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "Waz",
        Channel::Preview => "WarpPreview",
        Channel::Local => "warp",
        Channel::Integration => "integration",
        Channel::Dev => "WarpDev",
        // Align with script/windows/bundle.ps1 OSS branch INSTALLER_NAME=Waz+Setup,
        // so that the GitHub Release asset name WazSetup.exe can be correctly generated by installer_file_name().
        Channel::Oss => "Waz",
    }
}
