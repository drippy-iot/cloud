use cloud::router::Router;
use std::{
    env::var,
    net::{Ipv4Addr, TcpListener},
    sync::Arc,
};

fn main() -> anyhow::Result<()> {
    let config: tokio_postgres::Config = var("PG_URL")?.parse()?;
    let tls = native_tls::TlsConnector::builder().danger_accept_invalid_certs(true).build()?;
    let tls = postgres_native_tls::MakeTlsConnector::new(tls);

    let port: u16 = var("PORT")?.parse()?;
    let tcp = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))?;
    tcp.set_nonblocking(true)?;

    env_logger::init();

    let addr = tcp.local_addr()?;
    log::info!("listening to {addr}");

    let http = hyper::server::conn::http1::Builder::new();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_io().enable_time().build()?;
    rt.block_on(async {
        let tcp = tokio::net::TcpListener::from_std(tcp)?;
        let mut stop = core::pin::pin!(tokio::signal::ctrl_c());

        let (client, conn) = config.connect(tls).await?;
        let db = cloud::database::Database::from(client);
        let db = Arc::new(db);
        let handle = rt.spawn(conn);
        log::info!("connected to the database");

        let router = Router::new(db);
        loop {
            let res = tokio::select! {
                accept_res = tcp.accept() => accept_res,
                stop_res = &mut stop => break stop_res?,
            };

            let (stream, other) = match res {
                Ok(pair) => pair,
                Err(err) => {
                    log::error!("{err:?}");
                    continue;
                }
            };

            log::info!("new connection from {other}");

            let router = router.clone();
            let svc = hyper::service::service_fn(move |req| {
                use futures_util::FutureExt;
                router.clone().handle(req).map(Ok::<_, core::convert::Infallible>)
            });
            rt.spawn(http.serve_connection(stream, svc));
        }

        drop(router);
        handle.await??;
        anyhow::Ok(())
    })?;

    log::warn!("stop signal received");
    Ok(())
}
