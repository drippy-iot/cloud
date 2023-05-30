use crate::model::UserMessage;

use chrono::{DateTime, Utc};
use tokio_postgres::error::SqlState;

pub use model::{report::Flow, MacAddress};
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

    /// Reports to the database the current water flow of the device.
    /// Returns the latest shutdown state of the unit.
    pub async fn report_flow(&self, Flow { addr, flow }: Flow) -> (DateTime<Utc>, bool) {
        let flow = i16::try_from(flow).unwrap();
        let row = self
            .db
            .query_one(
                "WITH _ AS (INSERT INTO flow (mac, flow) VALUES ($1, $2) RETURNING creation, mac), \
                old AS (SELECT shutdown FROM _ INNER JOIN unit USING (mac)) \
                UPDATE unit SET shutdown = DEFAULT FROM _, old WHERE unit.mac = _.mac RETURNING _.creation, old.shutdown",
                &[&addr, &flow],
            )
            .await
            .unwrap();

        let creation = row.get(0);
        let shutdown = row.get(1);
        (creation, shutdown)
    }

    /// Reports to the database a single instance of a leak detection.
    /// Returns the latest shutdown state of the unit.
    pub async fn report_leak(&self, mac: MacAddress) -> (DateTime<Utc>, bool) {
        let row = self
            .db
            .query_one(
                "WITH _ AS (INSERT INTO leak (mac) VALUES ($1) RETURNING creation, mac), \
                old AS (SELECT shutdown FROM _ INNER JOIN unit USING (mac)) \
                UPDATE unit SET shutdown = FALSE FROM _, old WHERE unit.mac = _.mac RETURNING _.creation, old.shutdown",
                &[&mac],
            )
            .await
            .unwrap();

        let creation = row.get(0);
        let shutdown = row.get(1);
        (creation, shutdown)
    }

    /// Sets the shutdown flag for the unit associated with the given
    /// MAC address. Returns the previously set value for the flag.
    pub async fn request_shutdown(&self, mac: MacAddress) -> (DateTime<Utc>, bool) {
        let row = self
            .db
            .query_one(
                "WITH _ AS (INSERT INTO control (mac, shutdown) VALUES ($1, TRUE) RETURNING creation, mac, shutdown), \
                old AS (SELECT mac, unit.shutdown FROM _ INNER JOIN unit USING (mac)) \
                UPDATE unit SET shutdown = _.shutdown FROM _, old WHERE unit.mac = old.mac RETURNING _.creation, old.shutdown",
                &[&mac],
            )
            .await
            .unwrap();

        let creation = row.get(0);
        let shutdown = row.get(1);
        (creation, shutdown)
    }

    /// Resets the shutdown flag for the unit associated with the given
    /// MAC address. Returns the previously set value for the flag.
    pub async fn request_reset(&self, mac: MacAddress) -> (DateTime<Utc>, bool) {
        let row = self
            .db
            .query_one(
                "WITH _ AS (INSERT INTO control (mac, shutdown) VALUES ($1, FALSE) RETURNING creation, mac, shutdown), \
                old AS (SELECT mac, unit.shutdown FROM _ INNER JOIN unit USING (mac)) \
                UPDATE unit SET shutdown = _.shutdown FROM _, old WHERE unit.mac = old.mac RETURNING _.creation, old.shutdown",
                &[&mac],
            )
            .await
            .unwrap();

        let creation = row.get(0);
        let shutdown = row.get(1);
        (creation, shutdown)
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
    /// its latest shutdown request state. If no such sessions exist, we return [`None`].
    pub async fn get_unit_from_session(&self, sid: Uuid) -> Option<(MacAddress, bool)> {
        let row = self
            .db
            .query_opt("SELECT mac, shutdown FROM session INNER JOIN unit USING (mac) WHERE id = $1 LIMIT 1", &[&sid])
            .await
            .unwrap()?;

        let mac = row.get(0);
        let shutdown = row.get(1);
        Some((mac, shutdown))
    }

    /// Get a unified [`Vec`] of [`UserMessage`] JSON objects.
    pub async fn get_metrics_since(&self, mac: MacAddress, since: DateTime<Utc>) -> Vec<UserMessage> {
        let row = self.db
            .query_one(
                "WITH _ AS (\
                    SELECT 'flow' AS ty, mac, creation, flow, NULL::BOOLEAN AS shutdown FROM flow \
                        UNION ALL \
                    SELECT 'leak' AS ty, mac, creation, NULL AS flow, NULL AS shutdown FROM leak \
                        UNION ALL \
                    SELECT 'control' AS ty, mac, creation, NULL AS flow, shutdown FROM control\
                ) SELECT coalesce(json_strip_nulls(json_agg(_)), '[]') AS items FROM _ WHERE mac = $1 AND creation > $2",
                &[&mac, &since],
            )
            .await
            .unwrap();
        let val = row.get(0);
        serde_json::from_value(val).unwrap()
    }
}
