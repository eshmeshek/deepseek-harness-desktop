//! Cleaning up a host that outlived the app that owned it.
//!
//! The host is deliberately hard to kill: it must survive closing the window.
//! The flip side is that killing the app the hard way - Task Manager, a crash, a
//! forced logoff - skips the shutdown handler and leaves the host running with
//! nothing owning it. It then has no tray, no way to be stopped, and it holds
//! the runtime directory open, so the next launch cannot even reinstall over it.
//!
//! So the pair (host pid, owning app pid) is written down while a host runs. On
//! startup a recorded host whose owner is gone is an orphan and is reaped.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    host_pid: u32,
    app_pid: u32,
}

fn record_path(data: &Path) -> PathBuf {
    data.join("host.json")
}

/// Note that this app owns this host.
pub fn record(data: &Path, host_pid: u32) {
    let record = Record {
        host_pid,
        app_pid: std::process::id(),
    };
    if let Ok(json) = serde_json::to_string(&record) {
        let _ = std::fs::write(record_path(data), json);
    }
}

/// Forget the host: it stopped, or we stopped it.
pub fn clear(data: &Path) {
    let _ = std::fs::remove_file(record_path(data));
}

/// Kill a recorded host whose owning app is gone. Returns what it did, for the log.
pub fn reap(data: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(record_path(data)).ok()?;
    let record: Record = serde_json::from_str(&raw).ok()?;

    // Our own pid appearing here means a previous run in this same process,
    // which cannot be: treat it as stale and drop it.
    if record.app_pid == std::process::id() || !alive(record.app_pid) {
        if alive(record.host_pid) {
            kill_tree(record.host_pid);
            clear(data);
            return Some(format!("reaped orphaned host pid {}", record.host_pid));
        }
        clear(data);
    }
    None
}

fn alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        // tasklist filters exactly; its output names the process when it exists.
        let mut command = Command::new("tasklist");
        let Ok(output) = crate::proc::quiet(&mut command)
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
        else {
            return false;
        };
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    }
    #[cfg(not(windows))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
            || Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
    }
}

/// Kill the process and its children.
///
/// The image-name filter is a guard against pid reuse: between the record being
/// written and this running, the OS may have handed that number to something
/// else. Requiring it to be a node process keeps a stale record from taking down
/// an unrelated program.
fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        let _ = crate::proc::quiet(&mut command)
            .args([
                "/PID",
                &pid.to_string(),
                "/T",
                "/F",
                "/FI",
                "IMAGENAME eq node.exe",
            ])
            .output();
    }
    #[cfg(not(windows))]
    {
        // The host leads its own process group, so the negated pid reaches the
        // whole tree. Verified against the command line first, for the same
        // pid-reuse reason as the Windows filter.
        let is_node = Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("node"))
            .unwrap_or(false);
        if is_node {
            let _ = Command::new("kill")
                .args(["-TERM", &format!("-{pid}")])
                .status();
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    }
}
