use crate::database::Database;

use alloc::sync::Arc;
use cookie::Cookie;
use core::future::Future;
use futures_util::TryFutureExt;
use http_body_util::{BodyExt, Empty};
use hyper::{
    body::Incoming,
    header::{COOKIE, SET_COOKIE},
    http::{request::Parts, HeaderValue},
    HeaderMap, Method, Request, Response, StatusCode,
};
use model::{decode, report::Flow, MacAddress};
use uuid::Uuid;

fn extract_session_id(headers: &HeaderMap) -> Option<Uuid> {
    let header = headers.get(COOKIE)?.to_str().ok()?;
    Cookie::split_parse(header).find_map(|cookie| {
        let c = cookie.ok()?;
        if c.name() == "sid" {
            Uuid::parse_str(c.value()).ok()
        } else {
            None
        }
    })
}

async fn try_handle<D>(db: Arc<Database>, req: Request<Incoming>) -> Result<Response<Empty<D>>, StatusCode> {
    let (Parts { uri, method, headers, .. }, incoming) = req.into_parts();
    let bytes = match incoming.collect().await {
        Ok(body) => body.to_bytes(),
        Err(err) => {
            log::error!("{err}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    match method {
        Method::GET => match uri.path() {
            "/auth/session" => {
                let Some(sid) = extract_session_id(&headers) else {
                    return Err(StatusCode::UNAUTHORIZED);
                };
                let mut res = Response::new(Empty::new());
                *res.status_mut() =
                    if db.get_unit_from_session(sid).await.is_some() { StatusCode::OK } else { StatusCode::NOT_FOUND };
                Ok(res)
            }
            path => {
                log::warn!("unexpected request to GET {path}");
                Err(StatusCode::NOT_FOUND)
            }
        },
        Method::POST => match uri.path() {
            "/api/shutdown" => {
                let Some(sid) = extract_session_id(&headers) else {
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let Some((mac, _)) = db.get_unit_from_session(sid).await else {
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let mut res = Response::new(Empty::new());
                *res.status_mut() = if db.request_shutdown(mac).await { StatusCode::ACCEPTED } else { StatusCode::OK };
                Ok(res)
            }
            "/auth/session" => {
                if bytes.len() < 6 {
                    return Err(StatusCode::BAD_REQUEST);
                }

                let mut mac = [0; 6];
                mac.copy_from_slice(&bytes[..6]);

                let Some(uuid) = db.create_session(MacAddress(mac)).await else {
                    return Err(StatusCode::NOT_FOUND);
                };

                let fmt = uuid.simple();
                let cookie = alloc::format!("sid={fmt}; HttpOnly; SameSite=None; Secure");
                let cookie = HeaderValue::from_str(&cookie).unwrap();

                let mut res = Response::new(Empty::new());
                res.headers_mut().insert(SET_COOKIE, cookie);
                *res.status_mut() = StatusCode::CREATED;
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
                log::warn!("unexpected request to POST {path}");
                Err(StatusCode::NOT_FOUND)
            }
        },
        method => {
            log::warn!("unexpected {method} method received");
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
