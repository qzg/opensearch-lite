#![allow(clippy::field_reassign_with_default)]

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Uri};
use opensearch_lite::{
    http::request::Request, http::router, responses::Response, server::AppState, Config,
};
use serde_json::Value;

pub async fn call(state: &AppState, method: Method, path: &str, body: Value) -> Response {
    let body = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(serde_json::to_vec(&body).unwrap())
    };
    let mut headers = HeaderMap::new();
    if !body.is_empty() {
        headers.insert("content-type", HeaderValue::from_static("application/json"));
    }
    let request = Request::from_parts(method, path.parse::<Uri>().unwrap(), headers, body);
    router::handle(state.clone(), request).await
}

#[allow(dead_code)]
pub async fn ndjson_call(
    state: &AppState,
    method: Method,
    path: &str,
    body: &'static str,
) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/x-ndjson"),
    );
    let request = Request::from_parts(
        method,
        path.parse::<Uri>().unwrap(),
        headers,
        Bytes::from(body),
    );
    router::handle(state.clone(), request).await
}

pub fn ephemeral_state() -> AppState {
    let mut config = Config::default();
    config.ephemeral = true;
    AppState::new(config).unwrap()
}
