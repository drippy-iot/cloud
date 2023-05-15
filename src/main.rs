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

        loop {
            let (stream, other) = match tcp.accept().await {
                Ok(pair) => pair,
                Err(err) => {
                    log::error!("{err:?}");
                    continue;
                }
            };

            log::info!("new connection from {other}");

            use core::{convert::Infallible, future::ready};
            use hyper::Response;
            let svc = hyper::service::service_fn(|_| ready(Ok::<_, Infallible>(Response::new(String::new()))));
            rt.spawn(http.serve_connection(stream, svc));
        }
    })
}
