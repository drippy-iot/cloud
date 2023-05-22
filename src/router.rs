use crate::database::Database;

use alloc::sync::Arc;
use chrono::TimeZone;
use cookie::Cookie;
use core::future::Future;
use futures_util::{StreamExt, TryFutureExt};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::{Bytes, Incoming},
    header::{COOKIE, LAST_MODIFIED, SET_COOKIE},
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

fn extract_last_modified(headers: &HeaderMap) -> Option<chrono::DateTime<chrono::Utc>> {
    let header = headers.get(LAST_MODIFIED)?.to_str().ok()?;
    
    if header == "" {
        header.parse::<chrono::DateTime<chrono::Utc>>().ok()
    }else{
        chrono::Utc.with_ymd_and_hms(2023, 5, 20, 0, 0, 0).single()
    }
}

async fn try_handle(db: Arc<Database>, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, StatusCode> {
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
                    log::error!("absent session");
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let Some((mac, shutdown)) = db.get_unit_from_session(sid).await else {
                    log::error!("invalid session {sid}");
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let fmt = sid.simple();
                log::info!("session {fmt} retrieved session details for unit {mac} [{shutdown}]");

                let bytes = alloc::boxed::Box::<[_]>::from(mac.0);
                let body = Full::new(Bytes::from(bytes));

                let mut res = Response::new(body);
                *res.status_mut() = if shutdown { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::OK };
                Ok(res)
            }
            "/api/metrics" => {
                use alloc::vec::Vec;

                let Some(sid) = extract_session_id(&headers) else {
                    log::error!("absent session");
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let Some(last_modified) = extract_last_modified(&headers) else {
                    log::error!("something wrong with the parsing");
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                };

                let Some((mac, shutdown)) = db.get_unit_from_session(sid).await else {
                    log::error!("invalid session {sid}");
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let fmt = sid.simple();
                log::info!("session {fmt} retrieved metrics for unit {mac} [{shutdown}]");

                let data = db.get_flows(mac, last_modified).await.collect::<Vec<_>>().await;

                let body = Full::new(Bytes::from(serde_json::to_vec(&data).unwrap()));

                let mut res = Response::new(body);
                *res.status_mut() = if shutdown { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::OK };
                Ok(res)
            }
            path => {
                log::error!("unexpected request to GET {path}");
                Err(StatusCode::NOT_FOUND)
            }
        },
        Method::POST => match uri.path() {
            "/api/shutdown" => {
                let Some(sid) = extract_session_id(&headers) else {
                    log::error!("absent session");
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let fmt = sid.simple();
                let Some((mac, _)) = db.get_unit_from_session(sid).await else {
                    log::error!("invalid session {fmt}");
                    return Err(StatusCode::UNAUTHORIZED);
                };

                let mut res = Response::default();
                *res.status_mut() = if db.request_shutdown(mac).await { StatusCode::ACCEPTED } else { StatusCode::OK };
                log::info!("session {fmt} requested shutdown of unit {mac}");
                Ok(res)
            }
            "/auth/session" => {
                if bytes.len() < 6 {
                    log::error!("provided a MAC address that is too short");
                    return Err(StatusCode::BAD_REQUEST);
                }

                let mut mac = MacAddress([0; 6]);
                mac.0.copy_from_slice(&bytes[..6]);

                let Some(uuid) = db.create_session(mac).await else {
                    log::error!("cannot create session because unit {mac} does not exist yet");
                    return Err(StatusCode::NOT_FOUND);
                };

                let fmt = uuid.simple();
                log::info!("created new session {fmt} for unit {mac}");

                let cookie = alloc::format!("sid={fmt}; HttpOnly; SameSite=None; Secure");
                let cookie = HeaderValue::from_str(&cookie).unwrap();

                let mut res = Response::default();
                res.headers_mut().insert(SET_COOKIE, cookie);
                *res.status_mut() = StatusCode::CREATED;
                Ok(res)
            }
            "/report/flow" => {
                let Ok(flow) = decode::<Flow>(&bytes) else {
                    log::error!("malformed water flow reported");
                    return Err(StatusCode::BAD_REQUEST);
                };

                let Flow { addr, flow: data } = flow;
                log::info!("unit {addr} reported {data} ticks");

                let mut res = Response::default();
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

                let mut res = Response::default();
                *res.status_mut() =
                    if db.report_leak(mac).await { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::CREATED };
                Ok(res)
            }
            "/report/register" => {
                let Ok(mac) = decode::<MacAddress>(&bytes) else {
                    log::error!("malformed MAC registration");
                    return Err(StatusCode::BAD_REQUEST);
                };

                let mut res = Response::default();
                *res.status_mut() =
                    if db.register_unit(mac).await { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::CREATED };
                log::info!("unit {mac} registered");
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

pub fn handle(db: Arc<Database>, req: Request<Incoming>) -> impl Future<Output = Response<Full<Bytes>>> {
    try_handle(db, req).unwrap_or_else(|code| {
        let mut res = Response::default();
        *res.status_mut() = code;
        res
    })
}
