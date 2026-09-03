//! Finding files the bundler ships beside the application.
//!
//! Both bundled tools - node and pnpm - are looked up the same way and share the
//! same two hazards, so the lookup lives here once rather than twice:
//!
//! * Tauri resolves resources through canonicalisation, which on Windows yields
//!   a `\\?\` verbatim path. Node cannot run a script addressed that way - it
//!   calls `realpathSync` on it and dies with `EISDIR: lstat 'C:'` - so the
//!   prefix comes off before any path reaches a child process.
//! * `cargo run` does not stage bundle resources at all, so a development build
//!   has to fall back to the checkout.

use std::path::{Path, PathBuf};

use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};

/// Resolve a bundled resource, or `None` when this build has none staged.
pub fn find(app: &AppHandle, relative: &str) -> Option<PathBuf> {
    let resolved = app.path().resolve(relative, BaseDirectory::Resource).ok()?;
    let path = strip_verbatim(resolved);
    path.is_file().then_some(path)
}

/// Search upward from the executable and the working directory for a checkout
/// path. Only development builds reach this; release builds find the resource.
pub fn find_in_checkout(relative: &str) -> Option<PathBuf> {
    let starts = [
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf)),
        std::env::current_dir().ok(),
    ];
    for start in starts.into_iter().flatten() {
        for base in start.ancestors() {
            let candidate = base.join(relative);
            if candidate.is_file() {
                return Some(strip_verbatim(candidate));
            }
        }
    }
    None
}

/// Drop a Windows `\\?\` extended-length prefix.
pub fn strip_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|p| p.strip_prefix(VERBATIM)) {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
    }
}

const VERBATIM: &str = r"\\?\";

/// Make a bundled binary executable.
///
/// Archive formats and bundlers do not agree about preserving the executable
/// bit, and a node that cannot be executed is indistinguishable from a missing
/// one. Setting it costs nothing and removes a whole class of platform-specific
/// packaging failure.
#[cfg(unix)]
pub fn ensure_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let mut permissions = metadata.permissions();
    if permissions.mode() & 0o111 != 0 {
        return;
    }
    permissions.set_mode(0o755);
    let _ = std::fs::set_permissions(path, permissions);
}

#[cfg(not(unix))]
pub fn ensure_executable(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::strip_verbatim;
    use std::path::PathBuf;

    #[test]
    fn removes_the_extended_length_prefix() {
        let verbatim = concat!(r"\\", r"?\", r"C:\apps\pnpm\bin\pnpm.cjs");
        assert_eq!(
            strip_verbatim(PathBuf::from(verbatim)),
            PathBuf::from(r"C:\apps\pnpm\bin\pnpm.cjs")
        );
    }

    #[test]
    fn leaves_ordinary_paths_alone() {
        for path in [r"C:\apps\pnpm.cjs", "/usr/local/lib/node"] {
            assert_eq!(strip_verbatim(PathBuf::from(path)), PathBuf::from(path));
        }
    }
}
