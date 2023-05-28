use chrono::{serde::ts_milliseconds, DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ClientFlow {
    // Creation data of the data point
    #[serde(with = "ts_milliseconds")]
    pub creation: DateTime<Utc>,
    // Flow rate of a device at a given time.
    pub flow: u32,
}
