//! JSON line logs on stderr, as every DiffLens analyzer writes them: one
//! object per line with `time`, `level`, `service` and `msg`, then the
//! fields given. The level comes from DIFFLENS_LOG_LEVEL; an unknown name
//! falls back to info, because losing logs is worse than logging too much.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "trace" | "debug" => Self::Debug,
            "warn" | "warning" => Self::Warn,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
}

pub struct Logger<W: Write> {
    out: W,
    service: &'static str,
    threshold: Level,
}

impl<W: Write> Logger<W> {
    pub fn new(out: W, service: &'static str, level: &str) -> Self {
        Self {
            out,
            service,
            threshold: Level::parse(level),
        }
    }

    pub fn info(&mut self, msg: &str, fields: &[(&str, Value)]) {
        self.write(Level::Info, msg, fields);
    }

    pub fn error(&mut self, msg: &str, fields: &[(&str, Value)]) {
        self.write(Level::Error, msg, fields);
    }

    pub fn write(&mut self, level: Level, msg: &str, fields: &[(&str, Value)]) {
        if level < self.threshold {
            return;
        }
        let mut record = Map::new();
        record.insert("time".into(), Value::String(rfc3339_now()));
        record.insert("level".into(), Value::String(level.name().into()));
        record.insert("service".into(), Value::String(self.service.into()));
        record.insert("msg".into(), Value::String(msg.into()));
        for (key, value) in fields {
            record.insert((*key).into(), value.clone());
        }
        // A log line that cannot be written is only lost.
        let _ = writeln!(self.out, "{}", Value::Object(record));
    }
}

/// The current time in UTC as RFC 3339 with milliseconds, without a date
/// library.
fn rfc3339_now() -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = elapsed.as_secs();
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        elapsed.subsec_millis()
    )
}
