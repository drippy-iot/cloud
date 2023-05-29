use model::{report::Flow, MacAddress};
use nanorand::Rng as _;

#[tokio::test]
async fn database_tests() -> anyhow::Result<()> {
    let url = std::env::var("PG_URL")?.into_boxed_str();
    let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await?;
    let db = cloud::database::Database::from(client);
    let handle = tokio::spawn(conn);

    // Generate random MAC address for testing purposes.
    let mut rng = nanorand::WyRand::new();
    let [a, b, c, d, e, f, ..] = rng.rand();
    drop(rng);
    let addr = MacAddress([a, b, c, d, e, f]);
    let start = chrono::Utc::now();

    // Register the unit twice just to check if we handle the upsert gracefully
    assert!(!db.register_unit(addr).await); // first registration
    assert!(!db.register_unit(addr).await); // existing registration

    // Ping water flow thrice; no shutdown requests must occur
    let (creation, shutdown) = db.report_flow(Flow { addr, flow: 50 }).await;
    assert!(start < creation);
    assert!(!shutdown);
    let (creation, shutdown) = db.report_flow(Flow { addr, flow: 78 }).await;
    assert!(start < creation);
    assert!(!shutdown);
    let (creation, shutdown) = db.report_flow(Flow { addr, flow: 100 }).await;
    assert!(start < creation);
    assert!(!shutdown);

    // Report leaks thrice; no shutdown requests must occur
    let (creation, shutdown) = db.report_leak(addr).await;
    assert!(start < creation);
    assert!(!shutdown);
    let (creation, shutdown) = db.report_leak(addr).await;
    assert!(start < creation);
    assert!(!shutdown);
    let (creation, shutdown) = db.report_leak(addr).await;
    assert!(start < creation);
    assert!(!shutdown);

    // Request shutdown when reporting water flow
    let (creation, shutdown) = db.request_shutdown(addr).await; // request
    assert!(start < creation);
    assert!(!shutdown);
    let (creation, shutdown) = db.report_flow(Flow { addr, flow: 48 }).await; // acknowledge & reset
    assert!(start < creation);
    assert!(shutdown);
    let (creation, shutdown) = db.report_flow(Flow { addr, flow: 0 }).await; // proceed
    assert!(start < creation);
    assert!(!shutdown);

    // Request shutdown when reporting leaks
    let (creation, shutdown) = db.request_shutdown(addr).await; // request
    assert!(start < creation);
    assert!(!shutdown);
    let (creation, shutdown) = db.report_leak(addr).await; // acknowledge & reset
    assert!(start < creation);
    assert!(shutdown);
    let (creation, shutdown) = db.report_leak(addr).await; // proceed
    assert!(start < creation);
    assert!(!shutdown);

    let id = db.create_session(addr).await.unwrap();
    let (other, shutdown) = db.get_unit_from_session(id).await.unwrap();
    assert_eq!(addr, other);
    assert!(!shutdown);
    assert!(db.get_unit_from_session(uuid::Uuid::nil()).await.is_none());

    drop(db);
    handle.await??;
    Ok(())
}
