pub mod external_editor;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(windows)]
use warp_util::path::is_network_resource;
use warp_util::path::{CleanPathResult, LineAndColumnArg};

use crate::terminal::model::grid::grid_handler::{ContainsPoint, Link};
use crate::terminal::model::index::Point;
use crate::terminal::ShellLaunchData;

pub use self::external_editor::{open_file_path_in_external_editor, open_file_path_with_editor};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePathType {
    Absolute,
    /// Contains the working directory PathBuf.
    Relative(PathBuf),
}

#[derive(Debug)]
pub enum ShellPathType {
    /// The path comes from the shell and may need to be converted in a shell-aware way.
    ShellNative(String),
    /// The path has already been converted to a OS-native path.
    PlatformNative(PathBuf),
}

/// Waz: A snapshot of the real subkeys in a remote directory (cwd).
///
/// Populated by the results returned by the daemon's `ListDirectory` RPC. terminal link detector
/// Use it for accuracy verification in the remote session: `ls -l` the real file name in the entire line of candidate substrings
/// Cut it out - this is exactly what the `fs::metadata` existence check does in the local session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteDirListing {
    /// The absolute path to the directory (remote cwd).
    pub dir: PathBuf,
    /// Direct subkeys under the directory: file name -> whether it is a directory.
    pub entries: HashMap<String, bool>,
}

impl RemoteDirListing {
    pub fn new(dir: PathBuf, entries: HashMap<String, bool>) -> Self {
        Self { dir, entries }
    }
}

/// Waz: Verification source for terminal file links.
///
/// The local session uses the local file system `fs::metadata` to determine whether the path exists; remote SSH
/// (remote-server) The file of the session is not on the local disk, and the local verification must fail. Therefore, the remote
/// The session instead uses the real directory list cached by daemon `ListDirectory` RPC for accurate verification.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LinkValidationContext {
    /// Local session: Use the local file system to verify that the path actually exists.
    #[default]
    Local,
    /// Remote SSH session: Use cached remote cwd directory listing for accurate verification.
    ///
    /// `None` means that the directory list of the cwd has not been cached (asynchronous pull is in progress or the pull failed),
    /// At this time, this round of verification will be regarded as "invalid" (not highlighted), and will be lit again after re-rendering when the list arrives.
    Remote(Option<Arc<RemoteDirListing>>),
}

/// Checks if a file path exists and is valid for a file link.
pub fn absolute_path_if_valid(
    clean_path_result: &CleanPathResult,
    working_directory: ShellPathType,
    shell_launch_data: Option<&ShellLaunchData>,
    validation_ctx: &LinkValidationContext,
) -> Option<PathBuf> {
    let (maybe_absolute_path, relative_path) = match shell_launch_data {
        Some(shell_launch_data) => {
            // Attempt to parse the clean path result as an absolute path.
            let maybe_absolute_path =
                shell_launch_data.maybe_convert_absolute_path(&clean_path_result.path);
            let relative_path = match working_directory {
                ShellPathType::ShellNative(base_path_str) => shell_launch_data
                    .maybe_convert_relative_path(&base_path_str, &clean_path_result.path),
                ShellPathType::PlatformNative(base_path) => {
                    shell_launch_data.join_to_native_path(&base_path, &clean_path_result.path)
                }
            };
            (maybe_absolute_path, relative_path)
        }
        None => {
            // We naively attempt to treat the given paths as platform-native.
            let maybe_absolute_path = PathBuf::from(&clean_path_result.path);
            let relative_path = match working_directory {
                ShellPathType::ShellNative(path_str) => {
                    let mut path_buf = PathBuf::from(path_str);
                    path_buf.push(&clean_path_result.path);
                    path_buf
                }
                ShellPathType::PlatformNative(path_buf) => path_buf.join(&clean_path_result.path),
            };
            (Some(maybe_absolute_path), Some(relative_path))
        }
    };

    if relative_path
        .as_ref()
        .is_some_and(|path| is_path_valid(path, clean_path_result, validation_ctx))
    {
        return relative_path;
    } else if maybe_absolute_path
        .as_ref()
        .is_some_and(|path| is_path_valid(path, clean_path_result, validation_ctx))
    {
        return maybe_absolute_path;
    }

    None
}

fn is_path_valid(
    path: &Path,
    clean_path_result: &CleanPathResult,
    validation_ctx: &LinkValidationContext,
) -> bool {
    // Checking for the existence of a network resource takes a long time (~15s),
    // and hangs the UI, so we skip validating it.
    #[cfg(windows)]
    if is_network_resource(path) {
        return false;
    }

    // Waz: The files of the remote SSH session are not on the local disk, `fs::metadata` must fail.
    // Use daemon `ListDirectory` instead to cache the real directory list for accurate verification: candidate analysis
    // A path is valid ⇔ its parent directory is exactly equal to the cached cwd and its filename is a known child of that directory.
    // This provides a disambiguation basis for the link detector's substring search equivalent to the local `fs::metadata`,
    // Can accurately cut out the real file name from the entire line of `ls -l`.
    if let LinkValidationContext::Remote(listing) = validation_ctx {
        // The cwd list has not been cached (asynchronous pulling/failed): this round is considered invalid and will be lit again after the list arrives.
        let Some(listing) = listing else {
            return false;
        };
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        // The parent directory must be exactly the cwd being cached.
        if path.parent() != Some(listing.dir.as_path()) {
            return false;
        }
        let Some(&is_dir) = listing.entries.get(file_name) else {
            return false;
        };
        // Same as local: it cannot be a directory with row and column numbers.
        return !is_dir || clean_path_result.line_and_column_num.is_none();
    }

    // It should only be a valid path if the path links to a file or a folder without
    // line and column number attached.
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() || (metadata.is_dir() && clean_path_result.line_and_column_num.is_none())
}

/// Waz: Determine whether a resolved remote path points to a directory.
///
/// Only called when the remote session clicks a link and needs to decide "open a file vs `cd` into a directory";
/// The basis is the cached remote cwd directory list. Returned if the list is not cached or the path is not in it
/// `false` (processed by file, consistent with conservative behavior when not caching).
pub fn remote_path_is_dir(path: &Path, listing: &RemoteDirListing) -> bool {
    let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if path.parent() != Some(listing.dir.as_path()) {
        return false;
    }
    listing.entries.get(file_name).copied().unwrap_or(false)
}

impl FilePathType {
    /// Given a path that we've identified the FilePathType of,
    /// returns the absolute path.
    pub fn absolute_path(&self, path: PathBuf) -> PathBuf {
        match self {
            FilePathType::Absolute => path,
            FilePathType::Relative(directory) => directory.join(&path),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLink {
    pub link: Link,
    /// This path has been converted (if needed) into a native path from the shell.
    pub absolute_path: PathBuf,
    pub line_and_column_num: Option<LineAndColumnArg>,
}

impl FileLink {
    pub fn absolute_path(&self) -> Option<PathBuf> {
        Some(self.absolute_path.clone())
    }
}

impl ContainsPoint for FileLink {
    fn contains(&self, point: Point) -> bool {
        self.link.contains(point)
    }
}

/// Creates the file at the given path if it doesn't already exist, opening it
/// in write mode. If any directories in the path are missing, those are created
/// as well.
///
/// This always returns an error for unit tests, as they should not directly
/// interact with the filesystem.
pub fn create_file<P: AsRef<Path>>(_path: P) -> io::Result<fs::File> {
    cfg_if::cfg_if! {
        if #[cfg(test)] {
            Err(io::Error::from_raw_os_error(1))
        } else {
            let path = _path.as_ref();
            fs::create_dir_all(path.parent().ok_or_else(|| {
                io::Error::other(
                    "full_path should never be root directory.",
                )
            })?)?;
            fs::File::create(path)
        }
    }
}
