pub use model::{report::Flow, MacAddress};
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
    pub async fn register_unit(&self, mac: MacAddress) -> bool {
        let row = self
            .db
            .query_one(
                "INSERT INTO unit (mac) VALUES ($1) ON CONFLICT (mac) DO UPDATE SET mac = $1 RETURNING shutdown",
                &[&mac],
            )
            .await
            .unwrap();
        row.get(0)
    }

    pub async fn report_flow(&self, Flow { addr, flow }: Flow) -> bool {
        let flow = i16::try_from(flow).unwrap();
        let row = self
            .db
            .query_one(
                "WITH _ AS (INSERT INTO status (mac, flow) VALUES ($1, $2) RETURNING mac), \
                old AS (SELECT shutdown FROM _ INNER JOIN unit USING (mac)) \
                UPDATE unit SET shutdown = DEFAULT FROM _, old WHERE unit.mac = _.mac RETURNING old.shutdown",
                &[&addr, &flow],
            )
            .await
            .unwrap();
        row.get(0)
    }

    pub async fn report_leak(&self, mac: MacAddress) -> bool {
        let row = self
            .db
            .query_one(
                "WITH _ AS (INSERT INTO leak (mac) VALUES ($1) RETURNING mac), \
                old AS (SELECT shutdown FROM _ INNER JOIN unit USING (mac)) \
                UPDATE unit SET shutdown = DEFAULT FROM _, old WHERE unit.mac = _.mac RETURNING old.shutdown",
                &[&mac],
            )
            .await
            .unwrap();
        row.get(0)
    }

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

    /// Checks for the existence of a session. Returns the associated MAC address and
    /// its latest shutdown request state. If no such sessions exist, we return [`None`].
    pub async fn get_unit_from_session(&self, sid: Uuid) -> Option<(MacAddress, bool)> {
        let row = self
            .db
            .query_opt("SELECT mac, shutdown FROM session s INNER JOIN unit u USING (mac) WHERE id = $1 LIMIT 1", &[&sid])
            .await
            .unwrap()?;

        let mac = row.get(0);
        let shutdown = row.get(1);
        Some((mac, shutdown))
    }
}
