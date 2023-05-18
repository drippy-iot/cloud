mod router;

fn main() -> anyhow::Result<()> {
    use std::net::{Ipv4Addr, TcpListener};
    let port: u16 = std::env::var("PORT")?.parse()?;
    let tcp = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))?;
    tcp.set_nonblocking(true)?;

    env_logger::init();

    let addr = tcp.local_addr()?;
    log::info!("listening to {addr}");

    let http = hyper::server::conn::http1::Builder::new();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_io().build()?;
    rt.block_on(async {
        let tcp = tokio::net::TcpListener::from_std(tcp)?;
        let mut stop = core::pin::pin!(tokio::signal::ctrl_c());

        loop {
            let res = tokio::select! {
                accept_res = tcp.accept() => accept_res,
                stop_res = &mut stop => break stop_res,
            };

            let (stream, other) = match res {
                Ok(pair) => pair,
                Err(err) => {
                    log::error!("{err:?}");
                    continue;
                }
            };

            log::info!("new connection from {other}");

            let svc = hyper::service::service_fn(router::handle_http);
            rt.spawn(http.serve_connection(stream, svc));
        }
    })?;

    log::warn!("stop signal received");
    Ok(())
}
