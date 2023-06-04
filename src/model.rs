use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::types::{accepts, FromSql, Type};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[serde(tag = "ty")]
pub enum Payload {
    Ping { flow: u16, leak: bool },
    Open,
    Close,
}

#[derive(Deserialize, Serialize)]
pub struct UserMessage {
    #[serde(rename(serialize = "ts"))]
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
