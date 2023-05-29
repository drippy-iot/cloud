use chrono::{serde::ts_milliseconds, DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[serde(tag = "ty")]
pub enum Payload {
    Flow { flow: u16 },
    Control { shutdown: bool },
    Leak,
}

#[derive(Deserialize, Serialize)]
pub struct Message<Header> {
    pub head: Header,
    pub data: Payload,
}

pub type UserMessage = Message<DateTime<Utc>>;

#[derive(Serialize)]
pub struct ClientFlow {
    // Creation data of the data point
    #[serde(with = "ts_milliseconds")]
    pub creation: DateTime<Utc>,
    // Flow rate of a device at a given time.
    pub flow: u16,
}
