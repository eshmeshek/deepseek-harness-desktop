//! Managing the DeepSeek Harness runtime itself.
//!
//! Upstream ships no updater of its own, so this is where "update" lives. Each
//! version is installed from npm into its own directory:
//!
//! ```text
//! <data>/runtimes/<version>/node_modules/@deepseek-ai/dsh/lib/bin.js
//! ```
//!
//! Nothing is ever installed over a working version. That makes an update a
//! directory switch rather than a mutation, so a bad release is one menu click
//! away from being undone - which matters, because upstream is a developer
//! preview that announces compatibility-breaking changes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

/// The upstream package. Its `bin.js` is the same entry the `dsh` command uses.
pub const PACKAGE: &str = "@deepseek-ai/dsh";
const REGISTRY: &str = "https://registry.npmjs.org/@deepseek-ai%2Fdsh";

/// Which upstream dist-tag to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Latest,
    Next,
}

impl Channel {
    pub fn tag(self) -> &'static str {
        match self {
            Channel::Latest => "latest",
            Channel::Next => "next",
        }
    }
}

#[derive(Debug, Deserialize)]
struct Packument {
    #[serde(rename = "dist-tags")]
    dist_tags: std::collections::HashMap<String, String>,
}

/// Ask npm which version the channel currently points at.
pub fn published_version(channel: Channel) -> Result<String> {
    let body = ureq::get(REGISTRY)
        // Only dist-tags are needed; the full packument is megabytes.
        .set("Accept", "application/vnd.npm.install-v1+json")
        .timeout(std::time::Duration::from_secs(20))
        .call()
        .context("npm registry is unreachable")?
        .into_string()?;
    let packument: Packument =
        serde_json::from_str(&body).context("unexpected registry payload")?;
    packument
        .dist_tags
        .get(channel.tag())
        .cloned()
        .ok_or_else(|| anyhow!("registry has no `{}` tag", channel.tag()))
}

/// Versions already on disk, newest-installed last. Only complete installs count:
/// a directory whose `bin.js` is missing is a torn install, not a usable version.
pub fn installed(runtimes: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(runtimes) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            bin_path(runtimes, &name).is_file().then_some(name)
        })
        .collect();
    found.sort();
    found
}

/// A runtime the app can start.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub version: String,
    pub bin: PathBuf,
    /// Shipped inside the installer rather than downloaded into app data.
    pub bundled: bool,
}

/// The harness shipped inside the installer, if this build has one.
///
/// It exists so that a fresh install is usable offline and immediately: without
/// it the first launch spends twenty seconds downloading before anything works.
/// Development builds stage no resources and simply have none.
pub fn bundled(app: &tauri::AppHandle) -> Option<Runtime> {
    let stamp = crate::resources::find(app, "harness/version.txt")?;
    let version = fs::read_to_string(stamp).ok()?.trim().to_string();
    let bin = crate::resources::find(app, "harness/node_modules/@deepseek-ai/dsh/lib/bin.js")?;
    (!version.is_empty()).then_some(Runtime {
        version,
        bin,
        bundled: true,
    })
}

/// Every runtime this installation can start, oldest first.
pub fn available(app: &tauri::AppHandle, runtimes: &Path) -> Vec<Runtime> {
    let mut all: Vec<Runtime> = installed(runtimes)
        .into_iter()
        .map(|version| Runtime {
            bin: bin_path(runtimes, &version),
            version,
            bundled: false,
        })
        .collect();
    if let Some(baseline) = bundled(app) {
        // A downloaded copy of the same version wins: it is writable, and there
        // is no reason to keep two.
        if !all.iter().any(|r| r.version == baseline.version) {
            all.push(baseline);
        }
    }
    all.sort_by(|a, b| compare(&a.version, &b.version));
    all
}

/// The newest runtime available, which is the one to run.
pub fn newest(app: &tauri::AppHandle, runtimes: &Path) -> Option<Runtime> {
    available(app, runtimes).pop()
}

/// Order two versions the way semver does.
///
/// String order is not enough here and the difference is not academic: upstream
/// publishes `rc.7`, `rc.8`, `rc.10`, which sort wrongly as text, and `0.1.10`
/// would compare below `0.1.2`. Getting this wrong means offering a downgrade as
/// an update, or refusing a real one.
pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    let (a_core, a_pre) = split_version(a);
    let (b_core, b_pre) = split_version(b);
    a_core
        .cmp(&b_core)
        .then_with(|| match (a_pre.is_empty(), b_pre.is_empty()) {
            // A release outranks any prerelease of the same core version.
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => compare_prerelease(&a_pre, &b_pre),
        })
}

/// `1.2.3-rc.4` becomes `([1, 2, 3], "rc.4")`.
fn split_version(version: &str) -> ([u64; 3], String) {
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, pre.to_string()),
        None => (version, String::new()),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        [
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        ],
        pre,
    )
}

/// Compare dot-separated prerelease identifiers, numerically where both are numbers.
fn compare_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            // Fewer identifiers means lower precedence.
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    _ => x.cmp(y),
                };
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Entry point inside one npm prefix directory.
fn bin_in(prefix: &Path) -> PathBuf {
    prefix
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("lib")
        .join("bin.js")
}

/// Entry point of an installed version.
pub fn bin_path(runtimes: &Path, version: &str) -> PathBuf {
    bin_in(&runtimes.join(version))
}

/// Install one version into its own directory, using the bundled pnpm.
///
/// pnpm is driven through node rather than a shim so that a bundled node needs
/// no shell, no PATH entry and no platform-specific `.cmd` handling. It is pnpm
/// rather than npm because npm cannot install this graph in a usable time - see
/// the `pnpm` module for the measurements.
pub fn install(node: &Path, pnpm_cjs: &Path, runtimes: &Path, version: &str) -> Result<()> {
    let target = runtimes.join(version);
    // Install into a staging directory and promote it only on success, so an
    // interrupted download can never look like an installed version.
    let staging = runtimes.join(format!(".staging-{version}"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).context("cannot create the staging directory")?;
    // pnpm needs a manifest to add into; it is never read again.
    fs::write(
        staging.join("package.json"),
        r#"{"name":"dsh-runtime","version":"0.0.0","private":true}"#,
    )
    .context("cannot write the staging manifest")?;

    let spec = format!("{PACKAGE}@{version}");
    let mut command = Command::new(node);
    let output = crate::proc::quiet(&mut command)
        .arg(pnpm_cjs)
        .arg("add")
        .arg(&spec)
        .arg("--dir")
        .arg(&staging)
        // A hoisted layout keeps each version directory self-contained, so
        // removing an old one cannot break a newer one. Files are hardlinked
        // from the shared store, so versions do not each cost full size.
        .arg("--config.node-linker=hoisted")
        .arg("--store-dir")
        .arg(runtimes.join(".store"))
        .arg("--reporter=append-only")
        // The harness's lifecycle scripts are run by pnpm as plain `node ...`,
        // so the child needs a PATH where node exists even when the machine has
        // none of its own.
        .env("PATH", crate::node::augmented_path(node))
        .output()
        .context("cannot run pnpm through node")?;

    if !output.status.success() {
        // Both streams: pnpm reports the actual failure on stdout often enough
        // that stderr alone leaves you staring at a deprecation warning.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let _ = fs::remove_dir_all(&staging);
        bail!(
            "pnpm add {spec} failed:\n{}\n{}",
            stdout.trim(),
            stderr.trim()
        );
    }

    // An installer can exit 0 having produced nothing useful; check before promoting.
    if !bin_in(&staging).is_file() {
        let _ = fs::remove_dir_all(&staging);
        bail!("{spec} installed successfully but bin.js is missing");
    }

    // A target that will not go away is almost always a still-running host
    // holding it open. Saying that is far more useful than the rename's own
    // "directory not empty".
    if target.exists() {
        if let Err(error) = fs::remove_dir_all(&target) {
            let _ = fs::remove_dir_all(&staging);
            bail!(
                "cannot replace the installed version {version}: {error}. \
                 Its files are most likely held open by a running DeepSeek Harness."
            );
        }
    }
    fs::rename(&staging, &target).context("cannot promote the staged install")?;

    if !bin_path(runtimes, version).is_file() {
        bail!("{spec} installed without the expected bin.js");
    }
    Ok(())
}

/// Drop old versions, keeping the newest `keep` so a rollback target survives.
pub fn prune(runtimes: &Path, keep: usize) -> Result<()> {
    let versions = installed(runtimes);
    if versions.len() <= keep {
        return Ok(());
    }
    for stale in &versions[..versions.len() - keep] {
        fs::remove_dir_all(runtimes.join(stale))
            .with_context(|| format!("cannot remove old runtime {stale}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::compare;
    use std::cmp::Ordering;

    #[test]
    fn orders_numeric_parts_numerically() {
        assert_eq!(compare("0.1.10", "0.1.2"), Ordering::Greater);
        assert_eq!(compare("0.2.0", "0.10.0"), Ordering::Less);
        assert_eq!(compare("1.0.0", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn a_release_outranks_its_prereleases() {
        assert_eq!(compare("0.1.1", "0.1.1-rc.2"), Ordering::Greater);
        assert_eq!(compare("0.1.1-rc.2", "0.1.1"), Ordering::Less);
    }

    #[test]
    fn orders_upstreams_rc_numbers_correctly() {
        assert_eq!(compare("0.1.0-rc.10", "0.1.0-rc.8"), Ordering::Greater);
        assert_eq!(compare("0.1.1-rc.1", "0.1.1-rc.2"), Ordering::Less);
        assert_eq!(compare("0.1.2-alpha.1", "0.1.1-rc.2"), Ordering::Greater);
    }

    #[test]
    fn a_longer_prerelease_outranks_its_prefix() {
        assert_eq!(compare("1.0.0-rc.1", "1.0.0-rc"), Ordering::Greater);
    }
}
