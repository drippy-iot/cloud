use crate::database::Database;

use alloc::sync::Arc;
use cookie::Cookie;
use core::future::Future;
use futures_util::TryFutureExt;
use http_body_util::{BodyExt, Empty};
use hyper::{body::Incoming, http::request::Parts, Method, Request, Response, StatusCode};
use model::{decode, report::Flow, MacAddress};

async fn try_handle<D>(db: Arc<Database>, req: Request<Incoming>) -> Result<Response<Empty<D>>, StatusCode> {
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
            "/api/shutdown" => {
                let text = core::str::from_utf8(&bytes).unwrap();
                let Some(sid) = Cookie::split_parse(text).find_map(|cookie| {
                    let c = cookie.ok()?;
                    if c.name() == "sid" {
                        let uuid = uuid::Uuid::parse_str(c.value()).ok()?;
                        Some(uuid)
                    } else {
                        None
                    }
                }) else {
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let Some((mac, _)) = db.get_unit_from_session(sid).await else {
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let mut res = Response::new(Empty::new());
                *res.status_mut() = if db.request_shutdown(mac).await { StatusCode::ACCEPTED } else { StatusCode::OK };
                Ok(res)
            }
            "/report/flow" => {
                let Ok(flow) = decode::<Flow>(&bytes) else {
                    log::error!("malformed water flow reported");
                    return Err(StatusCode::BAD_REQUEST);
                };

                log::info!("{} reported {} ticks for this interval", flow.addr, flow.flow);

                let mut res = Response::new(Empty::new());
                *res.status_mut() =
                    if db.report_flow(flow).await { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::CREATED };
                Ok(res)
            }
            "/report/leak" => {
                let Ok(mac) = decode::<MacAddress>(&bytes) else {
                    log::error!("malformed leak reported");
                    return Err(StatusCode::BAD_REQUEST);
                };

                log::warn!("leak detected from {mac}");

                let mut res = Response::new(Empty::new());
                *res.status_mut() =
                    if db.report_leak(mac).await { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::CREATED };
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

pub fn handle<D>(db: Arc<Database>, req: Request<Incoming>) -> impl Future<Output = Response<Empty<D>>> {
    try_handle(db, req).unwrap_or_else(|code| {
        let mut res = Response::new(Empty::new());
        *res.status_mut() = code;
        res
    })
}
