//! Finding the Node.js runtime that everything else runs on.
//!
//! Release builds ship their own node beside the application, so an installed
//! app needs nothing preinstalled - that is the whole point of bundling it.
//! Development builds have no staged resources and fall back to PATH, which is
//! also the escape hatch if a bundled copy is ever unusable on some machine.
//!
//! npm is deliberately not looked for. The harness is installed with the pnpm
//! shipped alongside instead - see `pnpm` for the measurements that forced that.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use tauri::AppHandle;

use crate::resources;

#[cfg(windows)]
const BUNDLED: &str = "node/node.exe";
#[cfg(not(windows))]
const BUNDLED: &str = "node/node";

/// Where a located node came from, so the log can say which one is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Bundled,
    System,
}

/// Locate a usable node: the bundled copy first, then PATH.
pub fn locate(app: &AppHandle) -> Result<(PathBuf, Origin)> {
    if let Some(bundled) = resources::find(app, BUNDLED) {
        resources::ensure_executable(&bundled);
        // A bundled binary that will not run is a packaging fault, not a reason
        // to fail: PATH may still hold a perfectly good node.
        match version(&bundled) {
            Ok(found) if is_supported(&found) => return Ok((bundled, Origin::Bundled)),
            _ => crate::log::line("bundled node is unusable, falling back to PATH"),
        }
    }

    let on_path = which_node().context(
        "Node.js was not found. Install Node.js 22.19+ or 24+ (https://nodejs.org) and start the app again.",
    )?;
    // The harness declares `engines: ^22.19.0 || >=24`. Checking here turns a
    // confusing downstream crash into one sentence the user can act on.
    let found = version(&on_path)?;
    if !is_supported(&found) {
        bail!("Node.js {found} is too old: DeepSeek Harness needs 22.19+ or 24+.");
    }
    Ok((on_path, Origin::System))
}

/// Does this `vMAJOR.MINOR.PATCH` satisfy the harness's engine range?
fn is_supported(reported: &str) -> bool {
    let mut parts = reported.trim_start_matches('v').split('.');
    let major: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    major >= 24 || (major == 22 && minor >= 19)
}

/// Resolve `node` through PATH by asking it to report its own location, which
/// works the same on every platform and needs no `where`/`which` binary.
fn which_node() -> Result<PathBuf> {
    #[cfg(windows)]
    const EXE: &str = "node.exe";
    #[cfg(not(windows))]
    const EXE: &str = "node";

    let mut command = Command::new(EXE);
    let output = crate::proc::quiet(&mut command)
        .arg("-p")
        .arg("process.execPath")
        .output()
        .context("cannot execute node")?;
    if !output.status.success() {
        bail!("node exited with {}", output.status);
    }
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
    if !path.is_file() {
        bail!(
            "node reported a path that does not exist: {}",
            path.display()
        );
    }
    Ok(path)
}

/// PATH for children, with the chosen node's directory in front.
///
/// Necessary, not tidy: pnpm runs the harness's own lifecycle scripts, and those
/// are launched as plain `node ...` through a shell. On a machine with no system
/// Node - exactly the machine this app exists to serve - that lookup fails and
/// the install dies after downloading everything. Handing children a PATH that
/// contains the bundled runtime makes the bundling actually complete.
pub fn augmented_path(node: &Path) -> std::ffi::OsString {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let Some(dir) = node.parent() else {
        return existing;
    };
    let mut entries = vec![dir.to_path_buf()];
    entries.extend(std::env::split_paths(&existing));
    std::env::join_paths(entries).unwrap_or(existing)
}

/// Node's own version string, for the engine check and for diagnostics.
pub fn version(node: &Path) -> Result<String> {
    let mut command = Command::new(node);
    let output = crate::proc::quiet(&mut command).arg("--version").output()?;
    if !output.status.success() {
        bail!("node --version exited with {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::is_supported;

    #[test]
    fn accepts_the_declared_engine_range() {
        assert!(is_supported("v24.20.0"));
        assert!(is_supported("v22.19.0"));
        assert!(is_supported("v26.0.0"));
    }

    #[test]
    fn rejects_below_the_range() {
        assert!(!is_supported("v22.18.0"));
        assert!(!is_supported("v20.11.0"));
        // 23 is an odd-numbered line the harness does not list.
        assert!(!is_supported("v23.5.0"));
    }

    #[test]
    fn treats_garbage_as_unsupported() {
        assert!(!is_supported(""));
        assert!(!is_supported("not-a-version"));
    }
}
