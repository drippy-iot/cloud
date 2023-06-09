use crate::model::Flow;

use chrono::{DateTime, Utc};
use futures_util::Stream;
use tokio_postgres::error::SqlState;

pub use model::{report::Ping, MacAddress};
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
    /// Whenever the ESP32 board starts up, it must ping the database about its registration.
    /// If the MAC address had not been previously registered, we insert it into the database.
    /// Otherwise, we do nothing. In both cases, we return the latest shutdown state of the unit.
    pub async fn register_unit(&self, mac: MacAddress) -> Option<bool> {
        let row = self
            .db
            .query_one(
                "INSERT INTO unit (mac) VALUES ($1) ON CONFLICT (mac) DO UPDATE SET mac = $1 RETURNING request",
                &[&mac],
            )
            .await
            .unwrap();
        row.get(0)
    }

    /// Requests a remote shutdown/close. Returns the old value for the `unit.request` flag.
    pub async fn request_close(&self, mac: MacAddress) -> (DateTime<Utc>, Option<bool>) {
        let row = self
            .db
            .query_one(
                "WITH ins AS (INSERT INTO control (mac, request) VALUES ($1, FALSE) RETURNING creation), \
                old AS (SELECT request FROM unit WHERE mac = $1) \
                UPDATE unit SET request = CASE old.request WHEN TRUE THEN NULL ELSE FALSE END \
                FROM ins, old WHERE unit.mac = $1 RETURNING ins.creation, old.request",
                &[&mac],
            )
            .await
            .unwrap();

        let creation = row.get(0);
        let shutdown = row.get(1);
        (creation, shutdown)
    }

    /// Requests a remote bypass. Returns the old value for the `unit.request` flag.
    pub async fn request_open(&self, mac: MacAddress) -> (DateTime<Utc>, Option<bool>) {
        let row = self
            .db
            .query_one(
                "WITH ins AS (INSERT INTO control (mac, request) VALUES ($1, TRUE) RETURNING creation), \
                old AS (SELECT request FROM unit WHERE mac = $1) \
                UPDATE unit SET request = CASE old.request WHEN FALSE THEN NULL ELSE TRUE END \
                FROM ins, old WHERE unit.mac = $1 RETURNING ins.creation, old.request",
                &[&mac],
            )
            .await
            .unwrap();

        let creation = row.get(0);
        let request = row.get(1);
        (creation, request)
    }

    /// Reports a ping to the server. Also consumes the lateest `unit.request` command.
    pub async fn report_ping(&self, Ping { addr, flow, leak }: Ping) -> (DateTime<Utc>, Option<bool>) {
        let flow = i16::try_from(flow).unwrap();
        let row = self
            .db
            .query_one(
                "WITH ins AS (INSERT INTO ping (mac, flow, leak) VALUES ($1, $2, $3) RETURNING creation), \
                old AS (SELECT request FROM unit WHERE mac = $1) \
                UPDATE unit SET request = NULL FROM ins, old WHERE unit.mac = $1 RETURNING ins.creation, old.request",
                &[&addr, &flow, &leak],
            )
            .await
            .unwrap();

        let creation = row.get(0);
        let request = row.get(1);
        (creation, request)
    }

    /// Reports a bypass deactivation to the server.
    pub async fn report_bypass(&self, mac: MacAddress) -> DateTime<Utc> {
        let row = self.db.query_one("INSERT INTO bypass (mac) VALUES ($1) RETURNING creation", &[&mac]).await.unwrap();
        row.get(0)
    }

    /// Creates a new session from the given address. If the MAC address had not been previously
    /// registered (i.e., the ESP32 failed to ping the server before user logged in), this returns
    /// [`None`]. Otherwise, it returns the [`Uuid`] of the newly created session.
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
    /// its latest request state. If no such sessions exist, we return [`None`].
    pub async fn get_unit_from_session(&self, sid: Uuid) -> Option<(MacAddress, Option<bool>)> {
        let row = self
            .db
            .query_opt("SELECT mac, request FROM session INNER JOIN unit USING (mac) WHERE id = $1 LIMIT 1", &[&sid])
            .await
            .unwrap()?;

        let mac = row.get(0);
        let request = row.get(1);
        Some((mac, request))
    }

    pub async fn get_user_metrics_since(
        &self,
        mac: MacAddress,
        secs: f64,
        since: DateTime<Utc>,
    ) -> impl Stream<Item = Flow> {
        use futures_util::StreamExt as _;
        use tokio_postgres::types::ToSql;
        self
            .db
            .query_raw(
                "WITH _ AS (\
                    SELECT generate_series($3, NOW(), make_interval(secs => $2)) AS endpoint EXCEPT ALL SELECT $3\
                ) SELECT DISTINCT \
                    endpoint, \
                    COALESCE(\
                        AVG(flow) OVER (ORDER BY endpoint RANGE BETWEEN make_interval(secs => $2) PRECEDING AND CURRENT ROW), 0\
                    )::DOUBLE PRECISION AS mean \
                    FROM _ LEFT JOIN ping \
                        ON endpoint - make_interval(secs => $2) <= creation AND creation < endpoint \
                    WHERE mac = $1 ORDER BY endpoint",
                [&mac as &(dyn ToSql + Sync), &secs as _, &since as _],
            )
            .await
            .unwrap()
            .map(|row| {
                let row = row.unwrap();
                let end = row.get(0);
                let flow = row.get(1);
                Flow { end, flow }
            })
    }
}
