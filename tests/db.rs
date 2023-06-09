use chrono::Utc;
use cloud::model::Flow;
use core::pin::pin;
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
    let [a, b, c, d, e, f, ..] = rng.rand();
    let other = MacAddress([a, b, c, d, e, f]);
    let start = Utc::now();

    // Register the unit twice just to check if we handle the upsert gracefully

    assert_eq!(db.register_unit(addr).await, None); // first registration
    assert_eq!(db.register_unit(addr).await, None); // existing registration

    assert_eq!(db.register_unit(other).await, None); // first registration
    assert_eq!(db.register_unit(other).await, None); // existing registration

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

    // Compute elapsed time since test started
    let later = Utc::now() - start;
    let secs = later.to_std().unwrap().as_secs_f64();

    // Aggregate all the timestamps since the start of this test.
    // Everything should thus fall under a single bucket.

    let mut stream = pin!(db.get_user_metrics_since(addr, start, secs).await);
    let Flow { end, flow } = stream.next().await.unwrap();
    assert!(start < end);
    assert_eq!(flow, 206.75);
    assert_eq!(stream.next().await, None);

    let mut stream = pin!(db.get_system_metrics_since(start, secs).await);
    let Flow { end, flow } = stream.next().await.unwrap();
    assert!(start < end);
    assert_eq!(flow, 206.75);
    assert_eq!(stream.next().await, None);

    // System metrics should account for **all** units.

    let (creation, state) = db.report_ping(Ping { addr: other, flow: 4, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);
    let (creation, state) = db.report_ping(Ping { addr: other, flow: 14, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);

    let later = Utc::now() - start;
    let secs = later.to_std().unwrap().as_secs_f64();

    let mut stream = pin!(db.get_user_metrics_since(other, start, secs).await);
    let Flow { end, flow } = stream.next().await.unwrap();
    assert!(start < end);
    assert_eq!(flow, 9.0);
    assert_eq!(stream.next().await, None);
    let user_end = end;

    let mut stream = pin!(db.get_system_metrics_since(start, secs).await);
    let Flow { end, flow } = stream.next().await.unwrap();
    assert!(start < end);
    assert_eq!(flow, 178.5);
    assert_eq!(stream.next().await, None);
    let system_end = end;

    // Resume metrics from a checkpoint

    let (creation, state) = db.report_ping(Ping { addr, flow: 102, leak: false }).await;
    assert!(start < creation);
    assert_eq!(state, None);

    let later = Utc::now() - user_end;
    let secs = later.to_std().unwrap().as_secs_f64();

    let mut stream = pin!(db.get_user_metrics_since(addr, user_end, secs).await);
    let Flow { end, flow } = stream.next().await.unwrap();
    assert!(start < end);
    assert_eq!(flow, 102.0);
    assert_eq!(stream.next().await, None);

    let later = Utc::now() - system_end;
    let secs = later.to_std().unwrap().as_secs_f64();

    let mut stream = pin!(db.get_system_metrics_since(system_end, secs).await);
    let Flow { end, flow } = stream.next().await.unwrap();
    assert_eq!(stream.next().await, None);
    assert!(start < end);
    assert_eq!(flow, 102.0);

    // Aggregate the timestamps according to 60-second buckets. Note that it
    // is unlikely for the test suite to last more than a minute. Here, we
    // expect that the quantum has not passed yet, so there are no aggregations
    // that may be returned yet.
    let count = db.get_user_metrics_since(addr, start, 60.0).await.count().await;
    assert_eq!(count, 0);
    let count = db.get_system_metrics_since(start, 60.0).await.count().await;
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
