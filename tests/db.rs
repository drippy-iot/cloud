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

    // Register the unit twice just to check if we handle the upsert gracefully
    assert!(!db.register_unit(mac).await); // first registration
    assert!(!db.register_unit(mac).await); // existing registration

    // Ping water flow thrice; no shutdown requests must occur
    assert!(!db.report_flow(Flow { addr: mac, flow: 50 }).await.1);
    assert!(!db.report_flow(Flow { addr: mac, flow: 78 }).await.1);
    assert!(!db.report_flow(Flow { addr: mac, flow: 100 }).await.1);

    // Report leaks thrice; no shutdown requests must occur
    assert!(!db.report_leak(mac).await.1);
    assert!(!db.report_leak(mac).await.1);
    assert!(!db.report_leak(mac).await.1);

    // Request shutdown when reporting water flow
    assert!(!db.request_shutdown(mac).await.1);                    // request
    assert!(db.report_flow(Flow { addr: mac, flow: 50 }).await.1); // acknowledge & reset
    assert!(!db.report_flow(Flow { addr: mac, flow: 0 }).await.1); // proceed

    // Request shutdown when reporting leaks
    assert!(!db.request_shutdown(mac).await.1); // request
    assert!(db.report_leak(mac).await.1);       // acknowledge & reset
    assert!(!db.report_leak(mac).await.1);      // proceed

    let id = db.create_session(mac).await.unwrap();
    let (other_mac, shutdown) = db.get_unit_from_session(id).await.unwrap();
    assert_eq!(mac, other_mac);
    assert!(!shutdown);
    assert!(db.get_unit_from_session(Uuid::nil()).await.is_none());

    drop(db);
    handle.await??;
    Ok(())
}
