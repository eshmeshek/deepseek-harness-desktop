//! Owning the `dsh web` host process.
//!
//! The host is the thing that must outlive every window: agent turns run in it,
//! not in the UI, so closing a window may not stop it. Only an explicit Quit
//! does - see `Supervisor::stop`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// How long the host may take to report its URL before we call it a failure.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);

/// A running host and the tokenized URL its UI must be opened with.
pub struct Host {
    child: Child,
    pub url: String,
}

impl Host {
    /// The host's process id, recorded so an orphan can be found after a crash.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Has the host exited on its own? `Ok(None)` means it is still running.
    pub fn exited(&mut self) -> Result<Option<i32>> {
        Ok(self
            .child
            .try_wait()?
            .map(|status| status.code().unwrap_or(-1)))
    }

    /// Stop the host and everything it spawned.
    ///
    /// The host starts tool subprocesses (shells, language servers); killing
    /// only the parent would orphan them, so the whole tree goes.
    pub fn stop(&mut self) {
        #[cfg(windows)]
        {
            let mut command = Command::new("taskkill");
            let _ = crate::proc::quiet(&mut command)
                .args(["/PID", &self.child.id().to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(not(windows))]
        {
            // The child leads its own process group (see `start`), so a single
            // signal to the negated pid reaches every descendant.
            unsafe {
                libc_kill_group(self.child.id() as i32);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(not(windows))]
unsafe fn libc_kill_group(pid: i32) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    kill(-pid, SIGTERM);
}

/// Start `dsh web` and wait until it publishes its URL.
///
/// Port 0 lets the OS pick a free port, so two installations, or a leftover
/// process, cannot collide on a fixed one.
pub fn start(node: &Path, bin: &Path, workspace: &Path, log_dir: &Path) -> Result<Host> {
    std::fs::create_dir_all(log_dir).ok();

    let mut command = Command::new(node);
    command
        .arg(bin)
        .arg("web")
        .arg("--no-open")
        .arg("--port")
        .arg("0")
        .current_dir(workspace)
        // The host spawns tool subprocesses of its own; they must find node as
        // well, and on a machine without one only the bundled copy exists.
        .env("PATH", crate::node::augmented_path(node))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        // Own process group, so stop() can take the whole tree down at once.
        unsafe {
            command.pre_exec(|| {
                extern "C" {
                    fn setsid() -> i32;
                }
                setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: the host is a service, it must not flash a console.
        command.creation_flags(0x0800_0000);
    }

    let mut child = command.spawn().context("cannot start the dsh host")?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // Both streams are drained to files; a full pipe buffer would otherwise
    // block the host once its logs exceed the OS buffer.
    let (tx, rx) = mpsc::channel::<String>();
    spawn_drain(stdout, log_dir.join("host.out.log"), Some(tx));
    spawn_drain(stderr, log_dir.join("host.err.log"), None);

    let deadline = std::time::Instant::now() + STARTUP_TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            child.kill().ok();
            bail!("the dsh host did not report a URL within 90s");
        }
        match rx.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(line) => {
                if let Some(url) = extract_url(&line) {
                    return Ok(Host { child, url });
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(status) = child.try_wait()? {
                    bail!("the dsh host exited with {status} before reporting a URL");
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                child.kill().ok();
                bail!("the dsh host closed its output before reporting a URL");
            }
        }
    }
}

/// Copy a stream to a log file, optionally forwarding lines to the caller.
fn spawn_drain<R>(stream: R, log: PathBuf, tx: Option<mpsc::Sender<String>>)
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut file = std::fs::File::create(&log).ok();
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if let Some(file) = file.as_mut() {
                use std::io::Write;
                let _ = writeln!(file, "{line}");
            }
            if let Some(tx) = &tx {
                // A closed receiver just means startup finished; keep draining.
                let _ = tx.send(line);
            }
        }
    });
}

/// Pull the startup URL out of a host log line.
///
/// The test is "is it a loopback URL", not "does it carry a token". Upstream
/// versions differ here: 0.1.1-rc.2 as published on npm prints a bare
/// `http://127.0.0.1:<port>` and serves it without any fence, while later builds
/// append a `?token=` the browser exchanges for a cookie. Keying on the token
/// would silently fail to start on the very version this app installs today.
///
/// Loopback is the durable signal: the harness binds only the loopback
/// interface (serving all interfaces is explicitly unsupported upstream), so no
/// other URL in its output can be the address of its own UI.
fn extract_url(line: &str) -> Option<String> {
    let start = line.find("http://").or_else(|| line.find("https://"))?;
    let url: String = line[start..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    is_loopback(&url).then_some(url)
}

/// Does this URL address the local machine?
fn is_loopback(url: &str) -> bool {
    let after_scheme = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => return false,
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = match authority.rsplit_once(':') {
        // Keep the brackets of an IPv6 literal; only a trailing :port is split.
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => authority,
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::extract_url;

    #[test]
    fn finds_the_tokenized_url() {
        let line = "dsh web: http://127.0.0.1:64410/?token=abc-DEF_123";
        assert_eq!(
            extract_url(line).as_deref(),
            Some("http://127.0.0.1:64410/?token=abc-DEF_123")
        );
    }

    /// The version published on npm prints no token at all.
    #[test]
    fn finds_a_bare_loopback_url() {
        assert_eq!(
            extract_url("dsh web: http://127.0.0.1:52686").as_deref(),
            Some("http://127.0.0.1:52686")
        );
    }

    #[test]
    fn accepts_localhost_and_ipv6_loopback() {
        assert!(extract_url("dsh web: http://localhost:3080/").is_some());
        assert!(extract_url("dsh web: http://[::1]:3080/").is_some());
    }

    #[test]
    fn ignores_urls_that_are_not_the_local_ui() {
        assert!(extract_url("see https://deepseek.com/docs for help").is_none());
        // A remote host whose name merely mentions localhost must not pass.
        assert!(extract_url("proxy at http://localhost.example.com/x").is_none());
    }

    #[test]
    fn ignores_lines_without_a_url() {
        assert!(extract_url("dsh web: starting").is_none());
    }
}
