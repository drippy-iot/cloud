use crate::{
    database::Database,
    model::{Payload, UserMessage},
};

use chrono::{DateTime, NaiveDateTime, Utc};
use cookie::Cookie;
use core::{convert::Infallible, future::Future};
use futures_util::{FutureExt as _, Stream, StreamExt as _, TryFutureExt as _};
use http_body_util::{BodyExt as _, Either, Full, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    header::{CONTENT_TYPE, COOKIE, SET_COOKIE},
    http::{request::Parts, HeaderValue},
    HeaderMap, Method, Request, Response, StatusCode,
};
use model::{decode, report::Flow, MacAddress};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::broadcast::{channel, Sender};
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

fn to_sse_message<T: Serialize>(value: &T) -> serde_json::Result<Bytes> {
    let mut buffer = String::from("data: ").into_bytes();
    serde_json::to_writer(&mut buffer, value)?;
    buffer.extend_from_slice(b"\n\n");
    Ok(Bytes::from(buffer))
}

#[derive(Clone)]
pub struct Router {
    tx: Sender<(MacAddress, Bytes)>,
    db: Arc<Database>,
}

impl Router {
    /// Maximum capacity of the internal broadcast channel.
    const CAPACITY: usize = 16;

    pub fn new(db: Arc<Database>) -> Self {
        let (tx, _) = channel(Self::CAPACITY);
        Self { tx, db }
    }

    pub async fn try_handle(
        self,
        req: Request<Incoming>,
    ) -> Result<Response<Either<Full<Bytes>, impl Stream<Item = Frame<Bytes>>>>, StatusCode> {
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

                    let Some((mac, shutdown)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {sid}");
                        return Err(StatusCode::UNAUTHORIZED);
                    };

                    let fmt = sid.simple();
                    log::info!("session {fmt} retrieved session details for unit {mac} [{shutdown}]");

                    let bytes = Box::<[_]>::from(mac.0);
                    let body = Either::Left(Full::new(Bytes::from(bytes)));

                    let mut res = Response::new(body);
                    *res.status_mut() = if shutdown { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::OK };
                    Ok(res)
                }
                "/api/metrics" => {
                    let Some(query) = uri.query() else {
                        log::error!("query string is absent");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let Some(start) = query.split('&').find_map(|pair| {
                        let (key, value) = pair.split_once('=')?;
                        if key != "start" {
                            return None;
                        }
                        let millis = value.parse().ok()?;
                        let datetime = NaiveDateTime::from_timestamp_millis(millis)?;
                        Some(DateTime::from_utc(datetime, Utc))
                    }) else {
                        log::error!("query did not contain a valid timestamp");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let Some(sid) = extract_session_id(&headers) else {
                        log::error!("absent session");
                        return Err(StatusCode::UNAUTHORIZED);
                    };

                    let Some((mac, shutdown)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {sid}");
                        return Err(StatusCode::UNAUTHORIZED);
                    };

                    let data: Vec<_> = self.db.get_flows(mac, start).await.collect().await;
                    let fmt = sid.simple();
                    log::info!("session {fmt} retrieved metrics for unit {mac} [{shutdown}]");
                    let json = Frame::data(to_sse_message(&data).unwrap());

                    use tokio_stream::wrappers::{errors::BroadcastStreamRecvError, BroadcastStream};
                    let stream = BroadcastStream::new(self.tx.subscribe()).filter_map(move |res| {
                        use core::future::ready;
                        let (addr, bytes) = match res {
                            Ok(pair) => pair,
                            Err(BroadcastStreamRecvError::Lagged(val)) => {
                                log::warn!("session {sid} lagged behind {val} messages in the broadcast channel with capacity {}", Self::CAPACITY);
                                return ready(None);
                            }
                        };
                        ready(if addr == mac {
                            log::info!("session {sid} received a new data point live");
                            Some(Frame::data(bytes))
                        } else {
                            log::trace!("session {sid} ignored data point from {addr}");
                            None
                        })
                    });

                    let stream = tokio_stream::once(json).chain(stream);
                    let body = Either::Right(StreamBody::new(stream));

                    let mut res = Response::new(body);
                    res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
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
                    let Some((mac, _)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {fmt}");
                        return Err(StatusCode::UNAUTHORIZED);
                    };

                    let mut res = Response::new(Either::Left(Default::default()));
                    let (timestamp, shutdown) = self.db.request_shutdown(mac).await;
                    let message = UserMessage { head: timestamp, data: Payload::Control { shutdown: true } };
                    let json = to_sse_message(&message).unwrap();

                    if let Ok(receivers) = self.tx.send((mac, json)) {
                        log::trace!("unit {mac} notified {receivers} listeners");
                    } else {
                        log::trace!("no active listeners for unit {mac}");
                    }

                    *res.status_mut() = if shutdown { StatusCode::ACCEPTED } else { StatusCode::OK };
                    log::info!("session {fmt} requested shutdown of unit {mac}");
                    Ok(res)
                }
                "/api/reset" => {
                    let Some(sid) = extract_session_id(&headers) else {
                        log::error!("absent session");
                        return Err(StatusCode::UNAUTHORIZED);
                    };

                    let fmt = sid.simple();
                    let Some((mac, _)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {fmt}");
                        return Err(StatusCode::UNAUTHORIZED);
                    };

                    let mut res = Response::new(Either::Left(Default::default()));
                    let (timestamp, shutdown) = self.db.request_reset(mac).await;
                    let message = UserMessage { head: timestamp, data: Payload::Control { shutdown: false } };
                    let json = to_sse_message(&message).unwrap();

                    if let Ok(receivers) = self.tx.send((mac, json)) {
                        log::trace!("unit {mac} notified {receivers} listeners");
                    } else {
                        log::trace!("no active listeners for unit {mac}");
                    }

                    *res.status_mut() = if shutdown { StatusCode::ACCEPTED } else { StatusCode::OK };
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

                    let Some(uuid) = self.db.create_session(mac).await else {
                        log::error!("cannot create session because unit {mac} does not exist yet");
                        return Err(StatusCode::NOT_FOUND);
                    };

                    let fmt = uuid.simple();
                    log::info!("created new session {fmt} for unit {mac}");

                    let cookie = format!("sid={fmt}; HttpOnly; SameSite=None; Secure");
                    let cookie = HeaderValue::from_str(&cookie).unwrap();

                    let mut res = Response::new(Either::Left(Default::default()));
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

                    let (timestamp, shutdown) = self.db.report_flow(flow).await;
                    let message = UserMessage { head: timestamp, data: Payload::Flow { flow: data } };
                    let json = to_sse_message(&message).unwrap();

                    if let Ok(receivers) = self.tx.send((addr, json)) {
                        log::trace!("unit {addr} notified {receivers} listeners");
                    } else {
                        log::trace!("no active listeners for unit {addr}");
                    }

                    let mut res = Response::new(Either::Left(Default::default()));
                    *res.status_mut() = if shutdown { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::CREATED };
                    Ok(res)
                }
                "/report/leak" => {
                    let Ok(mac) = decode::<MacAddress>(&bytes) else {
                        log::error!("malformed leak reported");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    log::warn!("leak detected from {mac}");

                    let (timestamp, shutdown) = self.db.report_leak(mac).await;
                    let message = UserMessage { head: timestamp, data: Payload::Leak };
                    let json = to_sse_message(&message).unwrap();

                    if let Ok(receivers) = self.tx.send((mac, json)) {
                        log::trace!("unit {mac} notified {receivers} listeners");
                    } else {
                        log::trace!("no active listeners for unit {mac}");
                    }

                    let mut res = Response::new(Either::Left(Default::default()));
                    *res.status_mut() = if shutdown { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::CREATED };
                    Ok(res)
                }
                "/report/register" => {
                    let Ok(mac) = decode::<MacAddress>(&bytes) else {
                        log::error!("malformed MAC registration");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let mut res = Response::new(Either::Left(Default::default()));
                    *res.status_mut() = if self.db.register_unit(mac).await {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::CREATED
                    };
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

    pub fn handle(
        self,
        req: Request<Incoming>,
    ) -> impl Future<Output = Response<Either<Full<Bytes>, StreamBody<impl Stream<Item = Result<Frame<Bytes>, Infallible>>>>>>
    {
        self.try_handle(req)
            .unwrap_or_else(|code| {
                let mut res = Response::new(Either::Left(Default::default()));
                *res.status_mut() = code;
                res
            })
            .map(|res| {
                let (parts, body) = res.into_parts();
                let body = match body {
                    Either::Left(body) => Either::Left(body),
                    Either::Right(stream) => Either::Right(StreamBody::new(stream.map(Ok::<_, Infallible>))),
                };
                Response::from_parts(parts, body)
            })
    }
}
