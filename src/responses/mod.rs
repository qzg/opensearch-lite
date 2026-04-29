use std::collections::BTreeMap;

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Response as AxumResponse, StatusCode},
    response::IntoResponse,
};
use serde_json::{json, Value};

pub mod best_effort;
pub mod errors;
pub mod info;
pub mod logging;

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Value>,
}

impl Response {
    pub fn json(status: u16, body: Value) -> Self {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        Self {
            status,
            headers,
            body: Some(body),
        }
    }

    pub fn empty(status: u16) -> Self {
        Self {
            status,
            headers: BTreeMap::new(),
            body: None,
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn compatibility_signal(self, api_name: &str, tier: &str) -> Self {
        self.header("x-opensearch-lite-api", api_name)
            .header("x-opensearch-lite-tier", tier)
    }

    pub fn into_axum(self) -> AxumResponse<Body> {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = match self.body {
            Some(body) => Body::from(serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec())),
            None => Body::empty(),
        };
        let mut response = AxumResponse::builder().status(status);
        for (name, value) in self.headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(&value),
            ) {
                response = response.header(name, value);
            }
        }
        response.body(body).expect("response body is valid")
    }
}

impl IntoResponse for Response {
    fn into_response(self) -> AxumResponse<Body> {
        self.into_axum()
    }
}

pub fn open_search_error(
    status: u16,
    error_type: &str,
    reason: impl Into<String>,
    hint: Option<&str>,
) -> Response {
    let reason = reason.into();
    let mut error = json!({
        "root_cause": [
            {
                "type": error_type,
                "reason": reason
            }
        ],
        "type": error_type,
        "reason": reason
    });

    if let Some(hint) = hint {
        error["opensearch_lite_hint"] = Value::String(hint.to_string());
    }

    Response::json(
        status,
        json!({
            "error": error,
            "status": status
        }),
    )
}

pub fn acknowledged(acknowledged: bool) -> Response {
    Response::json(200, json!({ "acknowledged": acknowledged }))
}
