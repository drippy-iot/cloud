pub use model::MacAddress;
pub use tokio_postgres::Client;
pub use uuid::Uuid;

pub struct Database {
    db: Client,
}

impl From<Client> for Database {
    fn from(db: Client) -> Self {
        Self { db }
    }
}

impl Database {
    pub async fn create_session(&self, mac: MacAddress) -> Uuid {
        let row = self.db.query_one("INSERT INTO session (mac) VALUES ($1) RETURNING id", &[&mac]).await.unwrap();
        row.get(0)
    }

    pub async fn is_valid_session(&self, id: Uuid) -> bool {
        let row = self.db.query_one("SELECT EXISTS(SELECT 1 FROM session WHERE id = $1)", &[&id]).await.unwrap();
        row.get(0)
    }
}
