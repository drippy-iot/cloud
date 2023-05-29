use chrono::{serde::ts_milliseconds, DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::types::{accepts, FromSql, Type};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[serde(tag = "ty")]
pub enum Payload {
    Flow { flow: u16 },
    Control { shutdown: bool },
    Leak,
}

#[derive(Deserialize, Serialize)]
pub struct UserMessage {
    pub creation: DateTime<Utc>,
    #[serde(flatten)]
    pub data: Payload,
}

impl<'a> FromSql<'a> for UserMessage {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        assert_eq!(*ty, Type::JSON);
        let payload = serde_json::from_slice(raw)?;
        Ok(payload)
    }

    accepts!(JSON);
}

#[derive(Serialize)]
pub struct ClientFlow {
    // Creation data of the data point
    #[serde(with = "ts_milliseconds")]
    pub creation: DateTime<Utc>,
    // Flow rate of a device at a given time.
    pub flow: u16,
}
