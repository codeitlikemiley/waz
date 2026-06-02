use anyhow::{anyhow, Result};
use chrono::{DateTime, FixedOffset, Utc};
use instant::Instant;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Ord, PartialOrd)]
pub struct ServerTimestamp(DateTime<Utc>);

impl ServerTimestamp {
    pub fn new(time: DateTime<Utc>) -> Self {
        Self(time)
    }

    pub fn from_unix_timestamp_micros(ms_since_epoch: i64) -> Result<Self> {
        let date_time = DateTime::from_timestamp_micros(ms_since_epoch)
            .ok_or_else(|| anyhow!("Unable to convert microseconds into NaiveDateTime"))?;
        Ok(ServerTimestamp::new(date_time))
    }

    pub fn timestamp_micros(&self) -> i64 {
        self.0.timestamp_micros()
    }

    pub fn utc(&self) -> DateTime<Utc> {
        self.0
    }
}

impl From<DateTime<Utc>> for ServerTimestamp {
    fn from(value: DateTime<Utc>) -> Self {
        ServerTimestamp::new(value)
    }
}

/// Local estimation of server time.
///
/// Waz no longer requests `/current_time` from the cloud; the startup path is initialized with the local current time,
/// and callers can still obtain the wall-clock time that advances with the monotonic clock via this type.
#[derive(Debug, Clone)]
pub struct ServerTime {
    time_at_fetch: DateTime<FixedOffset>,
    fetched_at: Instant,
}

impl ServerTime {
    pub(crate) fn local_now() -> Self {
        Self {
            time_at_fetch: chrono::Utc::now().into(),
            fetched_at: Instant::now(),
        }
    }

    pub(crate) fn current_time(&self) -> DateTime<FixedOffset> {
        let elapsed = chrono::Duration::from_std(self.fetched_at.elapsed())
            .expect("duration should not be bigger than limit");
        self.time_at_fetch + elapsed
    }
}
