//! Logging (docs/08-backend.md "Errors"): everything goes to stderr → journald; every ERROR is also
//! appended to the error log file and forwarded to Telegram.
//!
//! The file is opened in append mode per event: errors are rare, and this survives logrotate without
//! signals or `copytruncate`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::server::ROBOT_EVENT_TARGET;

/// Events from these targets are not sent to Telegram (avoids loops when Telegram itself fails).
pub const NO_TELEGRAM_TARGET: &str = "aerolink::telegram";

struct ErrorLayer {
    file: PathBuf,
    telegram: Option<mpsc::Sender<String>>,
}

impl<S: Subscriber> Layer<S> for ErrorLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() != Level::ERROR {
            return;
        }
        let mut text = String::new();
        event.record(&mut FieldWriter(&mut text));
        // Robot errors are stored with the robot (ADM-003); the file is about the server itself.
        if meta.target() != ROBOT_EVENT_TARGET {
            let line = format!("{} ERROR {}: {}\n", rfc3339(SystemTime::now()), meta.target(), text);
            let written = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.file)
                .and_then(|mut f| f.write_all(line.as_bytes()));
            if let Err(e) = written {
                eprintln!("cannot write {}: {e}", self.file.display());
            }
        }
        if let Some(tx) = &self.telegram {
            if !meta.target().starts_with(NO_TELEGRAM_TARGET) {
                // Full queue = error storm; the file still has everything.
                let _ = tx.try_send(format!("{}: {}", meta.target(), text));
            }
        }
    }
}

struct FieldWriter<'a>(&'a mut String);

impl Visit for FieldWriter<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else {
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }
}

pub fn init(filter: &str, error_log: PathBuf, telegram: Option<mpsc::Sender<String>>) {
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .without_time() // journald stamps lines itself
        .with_filter(EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info")));
    tracing_subscriber::registry().with(stderr).with(ErrorLayer { file: error_log, telegram }).init();

    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        tracing::error!(target: "aerolink::panic", "panic{location}: {payload}");
    }));
}

/// `2026-10-29T12:34:56Z` without a date crate (days → civil date, H. Hinnant).
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_utc_timestamps() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(UNIX_EPOCH + Duration::from_secs(1_793_276_096)), "2026-10-29T12:14:56Z");
    }
}
