//! The app's own log.
//!
//! A background service that can only report failures through a modal dialog is
//! undiagnosable after the fact: the user clicks OK and the reason is gone. Every
//! startup step and every error is therefore also written here, next to the
//! host's own logs, which is where the tray's "Show logs" points.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Set once at startup; logging before that is a no-op rather than a panic.
static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn init(log_dir: &Path) {
    let _ = std::fs::create_dir_all(log_dir);
    *FILE.lock().unwrap() = Some(log_dir.join("app.log"));
}

/// Append one timestamped line. Failing to log must never fail the caller.
pub fn line(message: &str) {
    let Ok(guard) = FILE.lock() else { return };
    let Some(path) = guard.as_ref() else { return };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{}  {message}", stamp());
}

#[macro_export]
macro_rules! log_line {
    ($($arg:tt)*) => { $crate::log::line(&format!($($arg)*)) };
}

/// `YYYY-MM-DD HH:MM:SS` in UTC, computed from the epoch.
///
/// Hand-rolled rather than pulling a date crate in for one line of output: the
/// civil-from-days algorithm is short, exact, and has no dependencies.
fn stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let time = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let mut out = String::new();
    let _ = write!(
        out,
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    );
    out
}

/// Howard Hinnant's civil-from-days, days since 1970-01-01 to (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn converts_known_epochs() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 2024 was a leap year: day 60 of that year is 29 February.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn handles_dates_before_the_epoch() {
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
