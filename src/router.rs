use http_body_util::{combinators::BoxBody, BodyExt};
use hyper::Method;
use hyper::{Request, Response};

use http_body_util::{Empty, Full};
use hyper::body::Bytes;

pub async fn handle(
    req: Request<hyper::body::Incoming>,
) -> Result<Response<BoxBody<Bytes, hyper::Error>>, hyper::Error> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/leak") => {
            let x: Bytes = req.collect().await?.to_bytes();
            let y: Message = decode(&x).unwrap();

            let mac = y.head.mac;
            let timestamp = y.head.timestamp;
            let payload = y.data;

            log::info!("({}) mac: {}\t timestap: {}", "/leak", format!("{mac:#?}"), format!("{timestamp:#?}"));

            match payload {
                Payload::Conflict => Ok(Response::new(full("Success"))),
                _ => Ok(Response::new(full("Error"))),
            }
        }
        (&Method::POST, "/report") => {
            let x: Bytes = req.collect().await?.to_bytes();
            let y: Message = decode(&x).unwrap();

            let mac = y.head.mac;
            let timestamp = y.head.timestamp;
            let payload = y.data;

            log::info!("({}) mac: {}\t timestap: {}", "/report", format!("{mac:#?}"), format!("{timestamp:#?}"));

            match payload {
                Payload::Flow { ticks: _ } => Ok(Response::new(full("Success"))),
                _ => Ok(Response::new(full("Error"))),
            }
        }
        _ => Ok(Response::new(empty())),
    }
}

// We create some utility functions to make Empty and Full bodies
// fit our broadened Response body type.
fn empty() -> BoxBody<Bytes, hyper::Error> {
    Empty::<Bytes>::new().map_err(|never| match never {}).boxed()
}

fn full<T: Into<Bytes>>(chunk: T) -> BoxBody<Bytes, hyper::Error> {
    Full::new(chunk.into()).map_err(|never| match never {}).boxed()
}
