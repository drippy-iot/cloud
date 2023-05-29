use chrono::{serde::ts_milliseconds, DateTime, Utc};
use model::MacAddress;
use serde::{Deserialize, Serialize, Serializer};

fn mac_to_int<S: Serializer>(mac: &Option<MacAddress>, serializer: S) -> Result<S::Ok, S::Error> {
    if let Some(MacAddress([a, b, c, d, e, f])) = *mac {
        let value = u64::from_be_bytes([0, 0, a, b, c, d, e, f]);
        serializer.serialize_some(&value)
    } else {
        serializer.serialize_none()
    }
}

#[derive(Deserialize, Serialize)]
pub struct Header {
    /// MAC address to which the metrics is associated with.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(serialize_with = "mac_to_int")]
    pub mac: Option<MacAddress>,
    /// Exact time of the snapshot.
    pub timestamp: DateTime<Utc>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[serde(tag = "ty")]
pub enum Payload {
    Flow { flow: u16 },
    Control { shutdown: bool },
    Leak,
}

#[derive(Deserialize, Serialize)]
pub struct Message {
    pub head: Header,
    pub data: Payload,
}

#[derive(Serialize)]
pub struct ClientFlow {
    // Creation data of the data point
    #[serde(with = "ts_milliseconds")]
    pub creation: DateTime<Utc>,
    // Flow rate of a device at a given time.
    pub flow: u16,
}
