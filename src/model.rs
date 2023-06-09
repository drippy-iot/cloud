use chrono::{DateTime, Utc};
use serde::Serialize;

/// Aggregated flow data.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Flow {
    /// The upper bound of the interval.
    pub end: DateTime<Utc>,
    /// Average ticks per second over this interval.
    pub flow: f64,
}
