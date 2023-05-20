use core::future::Future;
use futures_util::TryFutureExt;
use http_body_util::{BodyExt, Empty};
use hyper::{body::Incoming, http::request::Parts, Method, Request, Response, StatusCode};
use model::{decode, report::Flow, MacAddress};

async fn try_handle<D>(req: Request<Incoming>) -> Result<Response<Empty<D>>, StatusCode> {
    let (Parts { uri, method, .. }, incoming) = req.into_parts();
    let bytes = match incoming.collect().await {
        Ok(body) => body.to_bytes(),
        Err(err) => {
            log::error!("{err}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    match method {
        Method::POST => match uri.path() {
            "/report/flow" => {
                let Ok(Flow { addr, flow }) = decode(&bytes) else {
                    log::error!("malformed water flow reported");
                    return Err(StatusCode::BAD_REQUEST);
                };

                log::info!("{addr} reported {flow} ticks for this interval");

                // TODO: Send to the database

                let mut res = Response::new(Empty::new());
                *res.status_mut() = StatusCode::CREATED;
                Ok(res)
            }
            "/report/leak" => {
                let Ok(ping) = decode::<MacAddress>(&bytes) else {
                    log::error!("malformed leak reported");
                    return Err(StatusCode::BAD_REQUEST);
                };

                log::warn!("leak detected from {ping}");

                // TODO: Send to the database

                let mut res = Response::new(Empty::new());
                *res.status_mut() = StatusCode::CREATED;
                Ok(res)
            }
            path => {
                log::error!("unexpected request to POST {path}");
                Err(StatusCode::NOT_FOUND)
            }
        },
        method => {
            log::error!("unexpected {method} method received");
            Err(StatusCode::METHOD_NOT_ALLOWED)
        }
    }
}

pub fn handle<D>(req: Request<Incoming>) -> impl Future<Output = Response<Empty<D>>> {
    try_handle(req).unwrap_or_else(|code| {
        let mut res = Response::new(Empty::new());
        *res.status_mut() = code;
        res
    })
}
