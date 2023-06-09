use chrono::Utc;
use futures_util::StreamExt as _;
use model::{report::Ping, MacAddress};
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
    let addr = MacAddress([a, b, c, d, e, f]);
    let start = Utc::now();

    // Register the unit twice just to check if we handle the upsert gracefully
    assert_eq!(db.register_unit(addr).await, None); // first registration
    assert_eq!(db.register_unit(addr).await, None); // existing registration

    // Ping water flow thrice without leaks; no shutdown requests must occur
    let (creation, state) = db.report_ping(Ping { addr, flow: 50, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);
    let (creation, state) = db.report_ping(Ping { addr, flow: 78, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);
    let (creation, state) = db.report_ping(Ping { addr, flow: 100, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);

    // Hardware should be able to report bypass deactivation
    let creation = db.report_bypass(addr).await;
    assert!(start < creation);
    let creation = db.report_bypass(addr).await;
    assert!(start < creation);
    let creation = db.report_bypass(addr).await;
    assert!(start < creation);

    // Report leaks thrice; no shutdown requests must occur
    let (creation, state) = db.report_ping(Ping { addr, flow: 100, leak: true }).await;
    assert!(start < creation);
    assert_eq!(state, None);
    let (creation, state) = db.report_ping(Ping { addr, flow: 256, leak: true }).await;
    assert!(start < creation);
    assert_eq!(state, None);
    let (creation, state) = db.report_ping(Ping { addr, flow: 512, leak: true }).await;
    assert!(start < creation);
    assert_eq!(state, None);

    // Request shutdown when reporting water flow
    let (creation, state) = db.request_close(addr).await;
    assert!(start < creation);
    assert_eq!(state, None);
    let (creation, state) = db.report_ping(Ping { addr, flow: 512, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, Some(false));
    let (creation, state) = db.report_ping(Ping { addr, flow: 100, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);

    // Request reset when reporting water flow
    let (creation, state) = db.request_open(addr).await;
    assert!(start < creation);
    assert_eq!(state, None);
    let (creation, state) = db.report_ping(Ping { addr, flow: 300, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, Some(true));
    let (creation, state) = db.report_ping(Ping { addr, flow: 457, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);

    // Remote bypass should override remote shutdown
    let (creation, state) = db.request_close(addr).await;
    assert!(start < creation);
    assert_eq!(state, None);
    let (creation, state) = db.request_open(addr).await;
    assert!(start < creation);
    assert_eq!(state, Some(false));
    let (creation, state) = db.report_ping(Ping { addr, flow: 12, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);

    // Remote shutdown should override remote bypass
    let (creation, state) = db.request_open(addr).await;
    assert!(start < creation);
    assert_eq!(state, None);
    let (creation, state) = db.request_close(addr).await;
    assert!(start < creation);
    assert_eq!(state, Some(true));
    let (creation, state) = db.report_ping(Ping { addr, flow: 4, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);

    // Aggregate all the timestamps since the start of this test.
    // Everything should thus fall under a single bucket.
    let later = Utc::now() - start;
    let secs = later.to_std().unwrap().as_secs_f64();
    let count = db.get_user_metrics_since(addr, start, secs).await.count().await;
    assert_eq!(count, 1);

    // Aggregate the timestamps according to 60-second buckets. Note that it
    // is unlikely for the test suite to last more than a minute. Here, we
    // expect that the quantum has not passed yet, so there are no aggregations
    // that may be returned yet.
    let count = db.get_user_metrics_since(addr, start, 60.0).await.count().await;
    assert_eq!(count, 0);

    // Test user login flow
    let id = db.create_session(addr).await.unwrap();
    let (other, state) = db.get_unit_from_session(id).await.unwrap();
    assert_eq!(addr, other);
    assert_eq!(state, None);
    assert!(db.get_unit_from_session(uuid::Uuid::nil()).await.is_none());

    drop(db);
    handle.await??;
    Ok(())
}
