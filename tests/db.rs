use cloud::database::Database;
use model::{MacAddress, report::Flow};
use nanorand::{WyRand, Rng};
use tokio_postgres::NoTls;
use uuid::Uuid;

#[tokio::test]
async fn database_tests() -> anyhow::Result<()> {
    let url = std::env::var("PG_URL")?.into_boxed_str();
    let (client, conn) = tokio_postgres::connect(&url, NoTls).await?;
    let db = Database::from(client);
    let handle = tokio::spawn(conn);

    // Generate random MAC address for testing purposes.
    let mut rng = WyRand::new();
    let [a, b, c, d, e, f, ..] = rng.rand();
    drop(rng);
    let mac = MacAddress([a, b, c, d, e, f]);

    assert!(!db.register_unit(mac).await); // first registration
    assert!(!db.register_unit(mac).await); // existing registration

    assert!(!db.report_flow(Flow { addr: mac, flow: 50 }).await);
    assert!(!db.report_flow(Flow { addr: mac, flow: 78 }).await);

    let id = db.create_session(mac).await.unwrap();
    assert!(db.is_valid_session(id).await);
    assert!(!db.is_valid_session(Uuid::nil()).await);

    drop(db);
    handle.await??;
    Ok(())
}
