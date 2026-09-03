//! Spawning child processes without flashing console windows.
//!
//! Every helper this app runs - the package manager, `tasklist`, `taskkill`,
//! `node --version` - is a console program. On Windows each one pops a console
//! window for as long as it runs unless told not to, and the install step runs
//! for twenty seconds. A tray application must be silent, so nothing is spawned
//! here except through `quiet`.

use std::process::Command;

/// CREATE_NO_WINDOW: run the child without allocating a console.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Suppress the console window a child would otherwise show.
pub fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
