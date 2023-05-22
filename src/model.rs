
use chrono::{DateTime, Utc};
use chrono::serde::ts_milliseconds;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct ClientFlow{
    // Creation data of the data point
    #[serde(with = "ts_milliseconds")]
    pub creation: DateTime<Utc>,
    // Flow rate of a device at a given time.
    pub flow: u32
} 