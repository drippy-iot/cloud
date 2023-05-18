use core::future::Future;
use futures_util::TryFutureExt;
use http_body_util::{BodyExt, Empty};
use hyper::{body::Incoming, http::request::Parts, Method, Request, Response, StatusCode};
use model::{decode, Message, Payload};

async fn try_handle<D>(req: Request<Incoming>) -> Result<Response<Empty<D>>, StatusCode> {
    let (Parts { uri, method, .. }, incoming) = req.into_parts();
    let bytes = incoming.collect().await.unwrap().to_bytes();
    let Message { data, .. } = decode(&bytes).unwrap();
    match method {
        Method::POST => match (uri.path(), data) {
            ("/leak", Payload::Conflict) | ("/report", Payload::Flow { .. }) => Ok(Response::new(Empty::new())),
            _ => Err(StatusCode::NOT_FOUND),
        },
        _ => Err(StatusCode::METHOD_NOT_ALLOWED),
    }
}

pub fn handle<D>(req: Request<Incoming>) -> impl Future<Output = Response<Empty<D>>> {
    try_handle(req).unwrap_or_else(|code| {
        let mut res = Response::new(Empty::new());
        *res.status_mut() = code;
        res
    })
}
