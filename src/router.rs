use crate::{database::Database, model::Flow};

use alloc::{boxed::Box, format, string::String, sync::Arc, vec::Vec};
use chrono::{DateTime, NaiveDateTime, Utc};
use cookie::Cookie;
use core::{convert::Infallible, future::Future, time::Duration};
use futures_util::{FutureExt as _, Stream, StreamExt as _, TryFutureExt as _};
use http_body_util::{BodyExt as _, Either, Full, StreamBody};
use hyper::{
    body::{Bytes, Frame, Incoming},
    header::{
        ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
        ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_REQUEST_HEADERS, CONTENT_TYPE, COOKIE, ORIGIN, SET_COOKIE,
    },
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
    to_sse("event: reset\ndata: ", value)
}

fn to_sse_close(value: &DateTime<Utc>) -> serde_json::Result<Bytes> {
    to_sse("event: shutdown\ndata: ", value)
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
    ) -> Result<Response<Either<Full<Bytes>, impl Stream<Item = Frame<Bytes>>>>, (StatusCode, Option<HeaderValue>)>
    {
        let (Parts { uri, method, mut headers, .. }, incoming) = req.into_parts();
        let bytes = match incoming.collect().await {
            Ok(body) => body.to_bytes(),
            Err(err) => {
                log::error!("{err}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, None));
            }
        };

        match method {
            Method::GET => match uri.path() {
                "/" => Ok(Response::new(Either::Left(Full::default()))),
                "/auth/session" => {
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None));
                    };

                    let Some(sid) = extract_session_id(&headers) else {
                        log::error!("absent session");
                        return Err((StatusCode::UNAUTHORIZED, Some(origin)));
                    };

                    let Some((mac, state)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {sid}");
                        return Err((StatusCode::UNAUTHORIZED, Some(origin)));
                    };

                    let fmt = sid.simple();
                    log::info!("session {fmt} retrieved session details for unit {mac} [{state:?}]");

                    let bytes = Box::<[_]>::from(mac.0);
                    let body = Either::Left(Full::new(Bytes::from(bytes)));

                    let mut res = Response::new(body);
                    let headers = res.headers_mut();
                    headers.append(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                    headers.append(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
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
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None));
                    };

                    let Some(query) = uri.query() else {
                        log::error!("query string is absent");
                        return Err((StatusCode::BAD_REQUEST, Some(origin)));
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
                        return Err((StatusCode::BAD_REQUEST, Some(origin)));
                    };

                    let secs = secs.map(Duration::from_secs).unwrap_or(POLL_QUANTUM);

                    let Some(sid) = extract_session_id(&headers) else {
                        log::error!("absent session");
                        return Err((StatusCode::UNAUTHORIZED, Some(origin)));
                    };

                    let Some((mac, state)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {sid}");
                        return Err((StatusCode::UNAUTHORIZED, Some(origin)));
                    };

                    let init: Vec<_> =
                        self.db.get_user_metrics_since(mac, since, secs.as_secs_f64()).await.collect().await;
                    log::info!("session {} retrieved {} metrics for unit {mac} [{state:?}]", init.len(), sid.simple());

                    // Spawn background task that periodically updates the SSE stream
                    let last = init.last().map(|Flow { end, .. }| end).copied().unwrap_or_else(Utc::now);
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
                                    let Some(last) = flow.last().map(|Flow { end, .. }| end).copied() else {
                                        log::warn!("no user metrics for {mac} [{state:?}] returned since last checkpoint {checkpoint}");
                                        continue;
                                    };
                                    checkpoint = last;
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
                    let headers = res.headers_mut();
                    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
                    headers.append(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                    headers.append(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
                    Ok(res)
                }
                "/api/metrics/system" => {
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None));
                    };

                    let Some(query) = uri.query() else {
                        log::error!("query string is absent");
                        return Err((StatusCode::BAD_REQUEST, Some(origin)));
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
                        return Err((StatusCode::BAD_REQUEST, Some(origin)));
                    };

                    let secs = secs.map(Duration::from_secs).unwrap_or(POLL_QUANTUM);

                    let init: Vec<_> =
                        self.db.get_system_metrics_since(since, secs.as_secs_f64()).await.collect().await;
                    log::info!("retrieved {} system metrics", init.len());

                    // Spawn background task that periodically updates the SSE stream
                    let last = init.last().map(|Flow { end, .. }| end).copied().unwrap_or_else(Utc::now);
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
                            let Some(last) = metrics.last().map(|Flow { end, .. }| end).copied() else {
                                log::warn!("no system metrics returned since last checkpoint {checkpoint}");
                                continue;
                            };
                            checkpoint = last;

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

                    let headers = res.headers_mut();
                    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
                    headers.append(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                    headers.append(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
                    Ok(res)
                }
                path => {
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None));
                    };
                    log::error!("unexpected request to GET {path}");
                    Err((StatusCode::NOT_FOUND, Some(origin)))
                }
            },
            Method::POST => match uri.path() {
                "/api/shutdown" => {
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None));
                    };

                    let Some(sid) = extract_session_id(&headers) else {
                        log::error!("absent session");
                        return Err((StatusCode::UNAUTHORIZED, Some(origin)));
                    };

                    let fmt = sid.simple();
                    let Some((mac, _)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {fmt}");
                        return Err((StatusCode::UNAUTHORIZED, Some(origin)));
                    };

                    let (creation, state) = self.db.request_close(mac).await;
                    log::info!("session {fmt} requested shutdown of unit {mac} [{state:?}]");

                    let json = to_sse_close(&creation).unwrap();
                    if let Ok(receivers) = self.tx.send((mac, json)) {
                        log::trace!("unit {mac} [{state:?}] notified {receivers} listeners");
                    } else {
                        log::trace!("no active listeners for unit {mac} [{state:?}]");
                    }

                    let mut res = Response::new(Either::Left(Default::default()));
                    let headers = res.headers_mut();
                    headers.append(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                    headers.append(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
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
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None));
                    };

                    let Some(sid) = extract_session_id(&headers) else {
                        log::error!("absent session");
                        return Err((StatusCode::UNAUTHORIZED, Some(origin)));
                    };

                    let fmt = sid.simple();
                    let Some((mac, _)) = self.db.get_unit_from_session(sid).await else {
                        log::error!("invalid session {fmt}");
                        return Err((StatusCode::UNAUTHORIZED, Some(origin)));
                    };

                    let (creation, state) = self.db.request_open(mac).await;
                    log::info!("session {fmt} requested reset of unit {mac} [{state:?}]");

                    let json = to_sse_open(&creation).unwrap();
                    if let Ok(receivers) = self.tx.send((mac, json)) {
                        log::trace!("unit {mac} [{state:?}] notified {receivers} listeners");
                    } else {
                        log::trace!("no active listeners for unit {mac} [{state:?}]");
                    }

                    let mut res = Response::new(Either::Left(Default::default()));
                    let headers = res.headers_mut();
                    headers.append(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                    headers.append(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
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
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None))
                    };

                    if bytes.len() < 6 {
                        log::error!("provided a MAC address that is too short");
                        return Err((StatusCode::BAD_REQUEST, Some(origin)));
                    }

                    let mut mac = MacAddress([0; 6]);
                    mac.0.copy_from_slice(&bytes[..6]);

                    let Some(uuid) = self.db.create_session(mac).await else {
                        log::error!("cannot create session because unit {mac} does not exist yet");
                        return Err((StatusCode::NOT_FOUND, Some(origin)));
                    };

                    let fmt = uuid.simple();
                    log::info!("created new session {fmt} for unit {mac}");

                    let cookie = format!("sid={fmt}; Path=/; HttpOnly; SameSite=None; Secure");
                    let cookie = HeaderValue::from_str(&cookie).unwrap();

                    let mut res = Response::new(Either::Left(Default::default()));
                    let headers = res.headers_mut();
                    headers.append(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                    headers.append(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
                    headers.insert(SET_COOKIE, cookie);
                    *res.status_mut() = StatusCode::CREATED;
                    Ok(res)
                }
                "/report/register" => {
                    let Ok(mac) = decode::<MacAddress>(&bytes) else {
                        log::error!("malformed MAC registration");
                        return Err((StatusCode::BAD_REQUEST, None))
                    };

                    let state = self.db.register_unit(mac).await;
                    log::info!("unit {mac} [{state:?}] registered");

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
                        return Err((StatusCode::BAD_REQUEST, None));
                    };

                    let Ping { addr, flow: data, leak } = flow;
                    let (creation, state) = self.db.report_ping(flow).await;
                    if leak {
                        log::warn!("unit {addr} [{state:?}] reported {data} ticks with a leak at {creation}");
                    } else {
                        log::info!("unit {addr} [{state:?}] reported {data} ticks at {creation}");
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
                        return Err((StatusCode::BAD_REQUEST, None))
                    };

                    let creation = self.db.report_bypass(addr).await;
                    log::warn!("unit {addr} reported a manual bypass at {creation}");

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
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None));
                    };
                    log::error!("unexpected request to POST {path}");
                    Err((StatusCode::NOT_FOUND, Some(origin)))
                }
            },
            Method::DELETE => match uri.path() {
                "/auth/session" => {
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None));
                    };

                    let Some(sid) = extract_session_id(&headers) else {
                        log::error!("absent session");
                        return Err((StatusCode::UNAUTHORIZED, Some(origin)));
                    };

                    let fmt = sid.simple();
                    let (body, status) = if let Some(mac) = self.db.delete_session(sid).await {
                        log::info!("logged out session {fmt}");
                        (Bytes::copy_from_slice(&mac.0), StatusCode::OK)
                    } else {
                        log::error!("cannot log out non-existent session {fmt}");
                        (Bytes::new(), StatusCode::NOT_FOUND)
                    };

                    let mut res = Response::new(Either::Left(Full::new(body)));
                    let headers = res.headers_mut();
                    let cookie = HeaderValue::from_static("sid=0; Max-Age=0; Path=/ HttpOnly; SameSite=None; Secure");
                    headers.append(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                    headers.append(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
                    headers.append(SET_COOKIE, cookie);
                    *res.status_mut() = status;
                    Ok(res)
                }
                path => {
                    let Some(origin) = headers.remove(ORIGIN) else {
                        log::error!("absent origin");
                        return Err((StatusCode::BAD_REQUEST, None));
                    };
                    log::error!("unexpected request to POST {path}");
                    Err((StatusCode::NOT_FOUND, Some(origin)))
                }
            },
            Method::OPTIONS => {
                let Some(origin) = headers.remove(ORIGIN) else {
                    log::error!("absent origin");
                    return Err((StatusCode::BAD_REQUEST, None));
                };

                let path = uri.path();
                let allow = match path {
                    "/api/session" => "GET, POST, DELETE",
                    "/api/reset" | "/api/shutdown" => "POST",
                    "/" | "/api/metrics/user" | "/api/metrics/system" => "GET",
                    _ => {
                        log::error!("unknown preflight path {path}");
                        return Err((StatusCode::BAD_REQUEST, Some(origin)));
                    }
                };

                log::info!("preflight check for {path}");
                let mut res = Response::new(Either::Left(Default::default()));
                let res_headers = res.headers_mut();
                res_headers.append(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                res_headers.append(ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static(allow));
                res_headers.append(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
                if let Some(req_headers) = headers.remove(ACCESS_CONTROL_REQUEST_HEADERS) {
                    res_headers.append(ACCESS_CONTROL_ALLOW_HEADERS, req_headers);
                }

                *res.status_mut() = StatusCode::NO_CONTENT;
                Ok(res)
            }
            method => {
                let Some(origin) = headers.remove(ORIGIN) else {
                    log::error!("absent origin");
                    return Err((StatusCode::BAD_REQUEST, None));
                };
                log::error!("unexpected {method} method received");
                Err((StatusCode::METHOD_NOT_ALLOWED, Some(origin)))
            }
        }
    }

    pub fn handle(
        self,
        req: Request<Incoming>,
    ) -> impl Future<Output = Response<Either<Full<Bytes>, StreamBody<impl Stream<Item = Result<Frame<Bytes>, Infallible>>>>>>
    {
        self.try_handle(req)
            .unwrap_or_else(|(code, origin)| {
                let mut res = Response::new(Either::Left(Default::default()));
                if let Some(origin) = origin {
                    log::info!("{origin:?}");
                    let headers = res.headers_mut();
                    headers.append(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                    headers.append(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
                }
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
