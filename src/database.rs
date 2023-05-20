use model::MacAddress;
use tokio_postgres::Client;

pub struct Database {
    db: Client,
}

impl From<Client> for Database {
    fn from(db: Client) -> Self {
        Self { db }
    }
}

impl Database {
    pub async fn create_session(&self, mac: MacAddress) -> uuid::Uuid {
        let row = self.db.query_one("INSERT INTO session (mac) VALUES ($1) RETURNING id", &[&mac]).await.unwrap();
        row.get(0)
    }
}
