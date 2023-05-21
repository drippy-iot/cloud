use cloud::database::Database;
use tokio_postgres::NoTls;

#[tokio::test]
async fn database_tests() -> anyhow::Result<()> {
    let url = std::env::var("PG_URL")?.into_boxed_str();
    let (client, conn) = tokio_postgres::connect(&url, NoTls).await?;
    let db = Database::from(client);
    let handle = tokio::spawn(conn);

    drop(db);
    handle.await??;
    Ok(())
}
