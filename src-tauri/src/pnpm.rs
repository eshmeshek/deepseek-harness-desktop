//! Locating the bundled pnpm.
//!
//! The harness dependency graph is ~500 packages across roughly a hundred
//! separately published `@deepseek-ai/dsh-*` modules. Measured on one machine,
//! same network and registry:
//!
//! | installer | result                                     |
//! |-----------|--------------------------------------------|
//! | npm       | still resolving after 12 min, 0 B written  |
//! | pnpm      | done in 22 s, 243 MB, 504 packages         |
//!
//! That is why pnpm is shipped inside the app rather than shelling out to
//! whatever package manager happens to be installed: with npm, first launch is
//! not slow, it is effectively broken.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tauri::AppHandle;

use crate::resources;

const BUNDLED: &str = "pnpm/bin/pnpm.cjs";
const IN_CHECKOUT: &str = "node_modules/pnpm/bin/pnpm.cjs";

/// Find pnpm's entry script: bundled beside the app, or in a dev checkout.
pub fn locate(app: &AppHandle) -> Result<PathBuf> {
    if let Some(bundled) = resources::find(app, BUNDLED) {
        return Ok(bundled);
    }
    resources::find_in_checkout(IN_CHECKOUT).context(
        "pnpm was not found: this build has no staged resources. Build with `npm run build`.",
    )
}
