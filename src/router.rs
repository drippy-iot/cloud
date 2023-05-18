use core::future::Future;
use futures_util::TryFutureExt;
use http_body_util::{BodyExt, Empty};
use hyper::{body::Incoming, http::request::Parts, Method, Request, Response, StatusCode};
use model::{decode, Message, Payload};

async fn try_handle<D>(req: Request<Incoming>) -> Result<Response<Empty<D>>, StatusCode> {
    let (Parts { uri, method, .. }, incoming) = req.into_parts();

    let bytes = match incoming.collect().await {
        Ok(body) => body.to_bytes(),
        Err(err) => {
            log::error!("{err}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let Ok(Message { data, .. }) = decode(&bytes) else {
        log::error!("user provided bad input");
        return Err(StatusCode::BAD_REQUEST);
    };

    match method {
        Method::POST => match (uri.path(), data) {
            ("/leak", Payload::Conflict) => {
                log::warn!("leak detected");
                Ok(Response::new(Empty::new()))
            }
            ("/report", Payload::Flow { ticks }) => {
                log::info!("reported {ticks} ticks for this interval");
                Ok(Response::new(Empty::new()))
            }
            (path, data) => {
                log::error!("unexpected {data:?} to POST {path}");
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
