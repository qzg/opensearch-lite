use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, Request as AxumRequest, Response as AxumResponse},
    middleware::Next,
    response::IntoResponse,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use tokio::time::sleep;

use crate::{
    responses::{open_search_error, Response},
    security::{context::SecurityContext, SecurityMode},
    server::AppState,
};

pub async fn middleware(
    State(state): State<AppState>,
    mut request: AxumRequest<Body>,
    next: Next,
) -> AxumResponse<Body> {
    match authenticate_headers(&state, request.headers()).await {
        Ok(context) => {
            request.extensions_mut().insert(context);
            next.run(request).await
        }
        Err(response) => response.into_response(),
    }
}

pub async fn authenticate_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<SecurityContext, Response> {
    if !state.security.auth_required() {
        return Ok(match state.security.mode() {
            SecurityMode::InsecureNonLoopback => SecurityContext::insecure_non_loopback(),
            _ => SecurityContext::disabled_loopback(),
        });
    }

    let credentials = match basic_credentials(headers) {
        Ok(credentials) => credentials,
        Err(response) => return Err(response),
    };

    let users = state
        .security
        .users()
        .expect("auth_required implies users are loaded");
    if let Some(principal) = users.verify(&credentials.username, &credentials.password) {
        return Ok(SecurityContext::secured(
            principal,
            state.config.security.require_client_cert,
        ));
    }

    let delay = state.security.auth_failure_delay();
    if !delay.is_zero() {
        sleep(delay).await;
    }
    Err(authentication_error(
        "invalid username or password",
        Some("Check the configured username/password or update the mounted users file."),
    ))
}

struct BasicCredentials {
    username: String,
    password: String,
}

fn basic_credentials(headers: &HeaderMap) -> Result<BasicCredentials, Response> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let Some(value) = values.next() else {
        return Err(authentication_error(
            "missing Authorization header",
            Some("Send HTTP Basic credentials for secured OpenSearch Lite."),
        ));
    };
    if values.next().is_some() {
        return Err(authentication_error(
            "multiple Authorization headers are not allowed",
            Some("Send exactly one HTTP Basic Authorization header."),
        ));
    }

    let value = value.to_str().map_err(|_| {
        authentication_error(
            "Authorization header must be valid UTF-8",
            Some("Send HTTP Basic credentials using an ASCII Authorization header."),
        )
    })?;
    let Some(encoded) = value.strip_prefix("Basic ") else {
        return Err(authentication_error(
            "Authorization header must use HTTP Basic",
            Some("Send credentials as Basic base64(username:password)."),
        ));
    };
    let decoded = STANDARD.decode(encoded).map_err(|_| {
        authentication_error(
            "Authorization header contains invalid Basic credentials",
            Some("Send credentials as Basic base64(username:password)."),
        )
    })?;
    let decoded = String::from_utf8(decoded).map_err(|_| {
        authentication_error(
            "Authorization header contains invalid Basic credentials",
            Some("Send credentials as Basic base64(username:password)."),
        )
    })?;
    let Some((username, password)) = decoded.split_once(':') else {
        return Err(authentication_error(
            "Authorization header contains invalid Basic credentials",
            Some("Send credentials as Basic base64(username:password)."),
        ));
    };
    if username.is_empty() {
        return Err(authentication_error(
            "username cannot be empty",
            Some("Send a non-empty username in the Basic Authorization header."),
        ));
    }
    Ok(BasicCredentials {
        username: username.to_string(),
        password: password.to_string(),
    })
}

pub fn authentication_error(reason: &str, hint: Option<&str>) -> Response {
    open_search_error(401, "security_exception", reason, hint)
        .header("www-authenticate", "Basic realm=\"opensearch-lite\"")
}
