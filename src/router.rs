use crate::{database::Database, model::Flow};

use alloc::{boxed::Box, format, string::String, sync::Arc, vec::Vec};
use chrono::{DateTime, NaiveDateTime, Utc};
use cookie::Cookie;
use core::{convert::Infallible, future::Future, time::Duration};
use futures_util::{FutureExt as _, Stream, StreamExt as _, TryFutureExt as _};
use http_body_util::{BodyExt as _, Either, Full, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    header::{CONTENT_TYPE, COOKIE, SET_COOKIE},
    http::{request::Parts, HeaderValue},
    HeaderMap, Method, Request, Response, StatusCode,
};
use model::{
    decode,
    report::{Ping, POLL_QUANTUM},
    MacAddress,
};
use tokio::sync::broadcast::{channel, error::RecvError, Sender};
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

fn to_sse<T>(prefix: &str, value: &T) -> serde_json::Result<Bytes>
where
    T: serde::Serialize + ?Sized,
{
    let mut buffer = String::from(prefix).into_bytes();
    serde_json::to_writer(&mut buffer, value)?;
    buffer.extend_from_slice(b"\n\n");
    Ok(Bytes::from(buffer))
}

fn to_sse_flow(value: &[Flow]) -> serde_json::Result<Bytes> {
    to_sse("event: flow\ndata: ", value)
}

fn to_sse_open(value: &DateTime<Utc>) -> serde_json::Result<Bytes> {
    to_sse("event: open\ndata: ", value)
}

fn to_sse_close(value: &DateTime<Utc>) -> serde_json::Result<Bytes> {
    to_sse("event: close\ndata: ", value)
}

fn to_sse_bypass(value: &DateTime<Utc>) -> serde_json::Result<Bytes> {
    to_sse("event: bypass\ndata: ", value)
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

                    let Some((mac, state)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {sid}");
                        return Err(StatusCode::UNAUTHORIZED);
                    };

                    let fmt = sid.simple();
                    log::info!("session {fmt} retrieved session details for unit {mac} [{state:?}]");

                    let bytes = Box::<[_]>::from(mac.0);
                    let body = Either::Left(Full::new(Bytes::from(bytes)));

                    let mut res = Response::new(body);
                    *res.status_mut() = match state {
                        // We're accepting some flow.
                        Some(true) => StatusCode::ACCEPTED,
                        // We can't have any flow because we requested a shutdown earlier.
                        Some(false) => StatusCode::NO_CONTENT,
                        // No pending commands have been issued.
                        None => StatusCode::OK,
                    };
                    Ok(res)
                }
                "/api/metrics/user" => {
                    let Some(query) = uri.query() else {
                        log::error!("query string is absent");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let mut since = None;
                    let mut secs = None;
                    for pair in query.split('&') {
                        if let Some((key, value)) = pair.split_once('=') {
                            match key {
                                "since" => since = value.parse().ok().and_then(NaiveDateTime::from_timestamp_millis),
                                "secs" => secs = value.parse().ok(),
                                _ => (),
                            }
                        }
                    }

                    let Some(since) = since.map(|datetime| DateTime::from_utc(datetime, Utc)) else {
                        log::error!("query did not contain a valid timestamp");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let secs = secs.map(Duration::from_secs).unwrap_or(POLL_QUANTUM);

                    let Some(sid) = extract_session_id(&headers) else {
                        log::error!("absent session");
                        return Err(StatusCode::UNAUTHORIZED);
                    };

                    let Some((mac, state)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {sid}");
                        return Err(StatusCode::UNAUTHORIZED);
                    };

                    let init: Vec<_> =
                        self.db.get_user_metrics_since(mac, since, secs.as_secs_f64()).await.collect().await;
                    log::info!("session {} retrieved {} metrics for unit {mac} [{state:?}]", init.len(), sid.simple());

                    // Spawn background task that periodically updates the SSE stream
                    let last = init.last().map(|Flow { end, .. }| end).copied().unwrap();
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    let mut broadcast = self.tx.subscribe();
                    tokio::spawn(async move {
                        let mut checkpoint = last;
                        loop {
                            use tokio::time::{sleep_until, Instant};
                            let bytes = tokio::select! {
                                // SSE stream has been closed
                                _ = tx.closed() => break,
                                // Get latest metrics since our last checkpoint
                                _ = sleep_until(Instant::now() + secs) => {
                                    let flow: Vec<_> = self
                                        .db
                                        .get_user_metrics_since(mac, checkpoint, secs.as_secs_f64())
                                        .await
                                        .collect()
                                        .await;
                                    checkpoint = flow.last().map(|Flow { end, .. }| end).copied().unwrap();
                                    to_sse_flow(&flow).unwrap()
                                }
                                // Receive extra events from the reporter APIs
                                result = broadcast.recv() => match result {
                                    Ok((addr, msg)) if addr == mac => msg,
                                    Err(RecvError::Closed) => unreachable!("broadcast channel closed"),
                                    Err(RecvError::Lagged(n)) => {
                                        log::warn!("broadcast receiver for unit {mac} [{state:?}] lagged behind {n} messages");
                                        continue
                                    },
                                    _ => continue,
                                },
                            };

                            // Notify the SSE stream of the new flow event
                            if tx.send(bytes).is_err() {
                                // SSE stream has been closed
                                break;
                            }

                            log::info!("updates to unit {mac} [{state:?}] have been dispatched to the stream");
                        }

                        log::warn!("stream for unit {mac} [{state:?}] has been closed");
                    });

                    use tokio_stream::wrappers::UnboundedReceiverStream;
                    let init = Frame::data(to_sse_flow(&init).unwrap());
                    let stream = UnboundedReceiverStream::new(rx).map(Frame::data);
                    let stream = tokio_stream::once(init).chain(stream);
                    let body = Either::Right(StreamBody::new(stream));
                    let mut res = Response::new(body);
                    res.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
                    Ok(res)
                }
                "/api/metrics/system" => {
                    let Some(query) = uri.query() else {
                        log::error!("query string is absent");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let mut since = None;
                    let mut secs = None;
                    for pair in query.split('&') {
                        if let Some((key, value)) = pair.split_once('=') {
                            match key {
                                "since" => since = value.parse().ok().and_then(NaiveDateTime::from_timestamp_millis),
                                "secs" => secs = value.parse().ok(),
                                _ => (),
                            }
                        }
                    }

                    let Some(since) = since.map(|datetime| DateTime::from_utc(datetime, Utc)) else {
                        log::error!("query did not contain a valid timestamp");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let secs = secs.map(Duration::from_secs).unwrap_or(POLL_QUANTUM);

                    let init: Vec<_> =
                        self.db.get_system_metrics_since(since, secs.as_secs_f64()).await.collect().await;
                    log::info!("retrieved {} system metrics", init.len());

                    // Spawn background task that periodically updates the SSE stream
                    let last = init.last().map(|Flow { end, .. }| end).copied().unwrap();
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    tokio::spawn(async move {
                        let mut checkpoint = last;
                        loop {
                            use core::pin::pin;
                            use tokio::time::{sleep_until, Instant};
                            let sleep = pin!(sleep_until(Instant::now() + secs));
                            let closed = pin!(tx.closed());

                            use futures_util::future::{select, Either};
                            if let Either::Right(_) = select(sleep, closed).await {
                                break;
                            }

                            // Get latest metrics since our last checkpoint
                            let metrics: Vec<_> =
                                self.db.get_system_metrics_since(checkpoint, secs.as_secs_f64()).await.collect().await;
                            checkpoint = metrics.last().map(|Flow { end, .. }| end).copied().unwrap();

                            // Notify the SSE stream of the new flow event
                            let json = to_sse_flow(&metrics).unwrap();
                            if tx.send(json).is_err() {
                                break;
                            }

                            log::info!("successfully dispatched system flow metrics");
                        }

                        log::warn!("system stream has been closed");
                    });

                    use tokio_stream::wrappers::UnboundedReceiverStream;
                    let init = Frame::data(to_sse_flow(&init).unwrap());
                    let stream = UnboundedReceiverStream::new(rx).map(Frame::data);
                    let stream = tokio_stream::once(init).chain(stream);
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

                    let (creation, state) = self.db.request_close(mac).await;
                    log::info!("session {fmt} requested shutdown of unit {mac}");

                    let json = to_sse_close(&creation).unwrap();
                    if let Ok(receivers) = self.tx.send((mac, json)) {
                        log::trace!("unit {mac} notified {receivers} listeners");
                    } else {
                        log::trace!("no active listeners for unit {mac}");
                    }

                    let mut res = Response::new(Either::Left(Default::default()));
                    *res.status_mut() = match state {
                        // We undid the previous command (i.e., nullified).
                        Some(true) => StatusCode::RESET_CONTENT,
                        // We ended up in the same state anyway.
                        Some(false) => StatusCode::NO_CONTENT,
                        // We successfully issued a new command.
                        None => StatusCode::CREATED,
                    };
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

                    let (creation, state) = self.db.request_open(mac).await;
                    log::info!("session {fmt} requested reset of unit {mac}");

                    let json = to_sse_open(&creation).unwrap();
                    if let Ok(receivers) = self.tx.send((mac, json)) {
                        log::trace!("unit {mac} notified {receivers} listeners");
                    } else {
                        log::trace!("no active listeners for unit {mac}");
                    }

                    let mut res = Response::new(Either::Left(Default::default()));
                    *res.status_mut() = match state {
                        // We ended up in the same state anyway.
                        Some(true) => StatusCode::NO_CONTENT,
                        // We undid the previous command (i.e., nullified).
                        Some(false) => StatusCode::RESET_CONTENT,
                        // We successfully issued a new command.
                        None => StatusCode::CREATED,
                    };
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
                "/report/register" => {
                    let Ok(mac) = decode::<MacAddress>(&bytes) else {
                        log::error!("malformed MAC registration");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let state = self.db.register_unit(mac).await;
                    log::info!("unit {mac} registered");

                    let mut res = Response::new(Either::Left(Default::default()));
                    *res.status_mut() = match state {
                        // We must "reset" by opening the valve.
                        Some(true) => StatusCode::RESET_CONTENT,
                        // We must make sure that there is "no water/content" by closing the valve.
                        Some(false) => StatusCode::NO_CONTENT,
                        // No pending commands issued.
                        None => StatusCode::CREATED,
                    };
                    Ok(res)
                }
                "/report/ping" => {
                    let Ok(flow) = decode::<Ping>(&bytes) else {
                        log::error!("malformed water flow reported");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let Ping { addr, flow: data, leak } = flow;
                    let (_, state) = self.db.report_ping(flow).await;
                    if leak {
                        log::warn!("unit {addr} reported {data} ticks with a leak");
                    } else {
                        log::info!("unit {addr} reported {data} ticks");
                    }

                    let mut res = Response::new(Either::Left(Default::default()));
                    *res.status_mut() = match state {
                        // We must "reset" by opening the valve.
                        Some(true) => StatusCode::RESET_CONTENT,
                        // We must make sure that there is "no water/content" by closing the valve.
                        Some(false) => StatusCode::NO_CONTENT,
                        // No pending commands issued.
                        None => StatusCode::CREATED,
                    };
                    Ok(res)
                }
                "/report/bypass" => {
                    let Ok(addr) = decode::<MacAddress>(&bytes) else {
                        log::error!("malformed MAC address received");
                        return Err(StatusCode::BAD_REQUEST);
                    };

                    let creation = self.db.report_bypass(addr).await;
                    log::warn!("unit {addr} reported a manual bypass");

                    let json = to_sse_bypass(&creation).unwrap();
                    if let Ok(receivers) = self.tx.send((addr, json)) {
                        log::trace!("unit {addr} notified {receivers} listeners");
                    } else {
                        log::trace!("no active listeners for unit {addr}");
                    }

                    let mut res = Response::new(Either::Left(Default::default()));
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
