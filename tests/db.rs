use cloud::database::Database;
use model::MacAddress;
use tokio_postgres::NoTls;
use uuid::Uuid;

#[tokio::test]
async fn database_tests() -> anyhow::Result<()> {
    let url = std::env::var("PG_URL")?.into_boxed_str();
    let (client, conn) = tokio_postgres::connect(&url, NoTls).await?;
    let db = Database::from(client);
    let handle = tokio::spawn(conn);

    let id = db.create_session(MacAddress([0x55, 0x44, 0x33, 0x22, 0x11, 0x00])).await;
    assert!(db.is_valid_session(id).await);
    assert!(!db.is_valid_session(Uuid::nil()).await);

    drop(db);
    handle.await??;
    Ok(())
}
