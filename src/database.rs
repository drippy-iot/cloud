pub use model::MacAddress;
pub use tokio_postgres::Client;
pub use uuid::Uuid;

use tokio_postgres::error::SqlState;

pub struct Database {
    db: Client,
}

impl From<Client> for Database {
    fn from(db: Client) -> Self {
        Self { db }
    }
}

impl Database {
    pub async fn create_session(&self, mac: MacAddress) -> Option<Uuid> {
        let err = match self.db.query_one("INSERT INTO session (mac) VALUES ($1) RETURNING id", &[&mac]).await {
            Ok(row) => return row.try_get(0).ok(),
            Err(err) => err,
        };

        let err = err.as_db_error()?;
        assert_eq!(*err.code(), SqlState::FOREIGN_KEY_VIOLATION);

        let constraint = err.constraint()?;
        assert_eq!(constraint, "FK_session.mac");
        None
    }

    pub async fn is_valid_session(&self, id: Uuid) -> bool {
        let row = self.db.query_one("SELECT EXISTS(SELECT 1 FROM session WHERE id = $1)", &[&id]).await.unwrap();
        row.get(0)
    }
}
