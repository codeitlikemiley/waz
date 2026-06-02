use std::time::Duration;

use instant::Instant;
use serde::{Deserialize, Serialize};
use warpui::{Entity, SingletonEntity};

/// This represents one interval, i.e. one stage in a multiple-stage timer.
struct TimingInterval {
    /// Assign each interval a unique name.
    name: String,
    /// When this interval ended.
    instant: Instant,
}

impl TimingInterval {
    fn new(name: String, instant: Instant) -> Self {
        Self { name, instant }
    }
}

/// This is a collection of points in time for a multiple-stage process that we want to time, and
/// we want to measure durations between each stage, or "interval".
pub struct IntervalTimer {
    /// The timer starts when this struct is instantiated. The first interval is measured as a
    /// duration from this instant, with each subsequent interval being measured from the end of
    /// the prior interval.
    start_instant: Instant,
    intervals: Vec<TimingInterval>,
}

impl IntervalTimer {
    pub fn new() -> Self {
        Self {
            start_instant: Instant::now(),
            intervals: Vec::new(),
        }
    }

    pub fn mark_interval_end(&mut self, name: impl Into<String>) {
        self.intervals
            .push(TimingInterval::new(name.into(), Instant::now()))
    }

    pub fn compute_duration_for_interval(&self, name: &str) -> Option<Duration> {
        self.intervals
            .iter()
            .enumerate()
            .find_map(|(idx, interval)| {
                if interval.name == name {
                    let since = if idx == 0 {
                        self.start_instant
                    } else {
                        self.intervals[idx - 1].instant
                    };

                    let marginal = interval.instant.duration_since(since);
                    Some(marginal)
                } else {
                    None
                }
            })
    }

    /// When the `WARP_STARTUP_TRACE=1` environment variable is enabled, write the collected timing
    /// table (marginal_ms per section / cumulative_ms / name) of the IntervalTimer as an ASCII table to stderr.
    /// Primarily used for local tuning - does not depend on any telemetry backend, purely local diagnostics.
    /// Has no side effects and does not modify any state.
    pub fn print_trace_to_stderr_if_enabled(&self) {
        let enabled = std::env::var("WARP_STARTUP_TRACE")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false);
        if !enabled {
            return;
        }
        let stats = self.compute_stats();
        eprintln!();
        eprintln!("=== WARP_STARTUP_TRACE ===");
        eprintln!("{:>8} {:>10}  name", "step_ms", "total_ms");
        for point in &stats {
            eprintln!(
                "{:>8} {:>10}  {}",
                point.marginal_duration_ms, point.cumulative_duration_ms, point.name
            );
        }
        eprintln!("==========================");
    }

    /// Once you are done with all the intervals in your process, we compute a cumulative sum of the
    /// time at each interval, as well as an individual time between each interval.
    pub fn compute_stats(&self) -> Vec<TimingDataPoint> {
        let mut cumulative_duration_ms = 0;
        self.intervals
            .iter()
            .enumerate()
            .map(|(i, interval)| {
                let since = if i == 0 {
                    self.start_instant
                } else {
                    self.intervals[i - 1].instant
                };
                // Converting a duration to an int in ms returns a u128 which is excessively large
                // for our purposes. It's also inconvenient as it isn't serializable by default.
                let marginal_duration_ms =
                    interval.instant.duration_since(since).as_millis() as u64;
                cumulative_duration_ms += marginal_duration_ms;
                TimingDataPoint::new(
                    marginal_duration_ms,
                    cumulative_duration_ms,
                    interval.name.clone(),
                )
            })
            .collect()
    }
}

impl Default for IntervalTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Entity for IntervalTimer {
    type Event = ();
}

impl SingletonEntity for IntervalTimer {}

/// Used for reporting the timing results after timing is complete.
#[derive(Clone, Deserialize, Serialize)]
pub struct TimingDataPoint {
    name: String,
    marginal_duration_ms: u64,
    cumulative_duration_ms: u64,
}

impl TimingDataPoint {
    fn new(marginal_duration_ms: u64, cumulative_duration_ms: u64, name: String) -> Self {
        Self {
            marginal_duration_ms,
            cumulative_duration_ms,
            name,
        }
    }

    /// Duration of a single segment (milliseconds).
    pub fn marginal_duration_ms(&self) -> u64 {
        self.marginal_duration_ms
    }

    /// Cumulative duration since startup (milliseconds).
    pub fn cumulative_duration_ms(&self) -> u64 {
        self.cumulative_duration_ms
    }

    /// The name of this interval.
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
#[path = "interval_timer_tests.rs"]
mod tests;
