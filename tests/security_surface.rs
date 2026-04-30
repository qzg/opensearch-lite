#![allow(clippy::field_reassign_with_default)]

use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Argon2,
};
use bytes::Bytes;
use http::{header, HeaderMap, HeaderValue, Method, Uri};
use opensearch_lite::{
    api_spec::{classify, AccessClass, Tier},
    http::request::Request,
    security::{authn, authz, Principal, Role, SecurityContext},
    server::AppState,
    Config,
};
use serde_json::json;

fn password_hash(password: &str) -> String {
    let salt = SaltString::encode_b64(b"opensearch-lite").unwrap();
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

fn users_file(role: Role) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let role = role.as_str();
    let body = json!({
        "users": [{
            "username": "kiyu",
            "password_hash": password_hash("secret"),
            "roles": [role]
        }]
    });
    std::fs::write(file.path(), serde_json::to_vec(&body).unwrap()).unwrap();
    file
}

fn secured_state(role: Role) -> (AppState, tempfile::NamedTempFile) {
    let users = users_file(role);
    let mut config = Config::default();
    config.ephemeral = true;
    config.security.users_file = Some(users.path().to_path_buf());
    (AppState::new(config).unwrap(), users)
}

fn basic(username: &str, password: &str) -> HeaderValue {
    let raw = format!("{username}:{password}");
    HeaderValue::from_str(&format!(
        "Basic {}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw)
    ))
    .unwrap()
}

#[test]
fn non_loopback_requires_tls_and_auth_unless_explicitly_insecure() {
    let error = Config::from_args([
        "opensearch-lite",
        "--listen",
        "0.0.0.0:9200",
        "--allow-nonlocal-listen",
    ])
    .unwrap_err();
    assert!(error.contains("configure TLS and --users-file"));

    let error = Config::from_args([
        "opensearch-lite",
        "--listen",
        "0.0.0.0:9200",
        "--allow-nonlocal-listen",
        "--tls-cert-file",
        "cert.pem",
        "--tls-key-file",
        "key.pem",
    ])
    .unwrap_err();
    assert!(error.contains("--users-file"));

    let error = Config::from_args([
        "opensearch-lite",
        "--listen",
        "0.0.0.0:9200",
        "--allow-nonlocal-listen",
        "--users-file",
        "users.json",
    ])
    .unwrap_err();
    assert!(error.contains("configure TLS"));

    let config = Config::from_args([
        "opensearch-lite",
        "--listen",
        "0.0.0.0:9200",
        "--allow-nonlocal-listen",
        "--allow-insecure-non-loopback",
    ])
    .unwrap();
    assert!(config.security.allow_insecure_non_loopback);

    let mut direct = Config::default();
    direct.listen = "0.0.0.0:9200".parse().unwrap();
    direct.allow_nonlocal_listen = true;
    let error = match AppState::new(direct) {
        Ok(_) => panic!("direct invalid config should fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("configure TLS and --users-file"));
}

#[tokio::test]
async fn basic_auth_uses_raw_authorization_headers_and_redacts_failures() {
    let (state, _users) = secured_state(Role::ReadOnly);

    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, basic("kiyu", "secret"));
    let context = authn::authenticate_headers(&state, &headers).await.unwrap();
    assert_eq!(context.principal.unwrap().username, "kiyu");

    headers.insert(header::AUTHORIZATION, basic("kiyu", "wrong"));
    let response = authn::authenticate_headers(&state, &headers)
        .await
        .unwrap_err();
    assert_eq!(response.status, 401);
    let body = response.body.unwrap().to_string();
    assert!(!body.contains("wrong"));
    assert!(!body.contains("secret"));

    let mut duplicate = HeaderMap::new();
    duplicate.append(header::AUTHORIZATION, basic("kiyu", "secret"));
    duplicate.append(header::AUTHORIZATION, basic("kiyu", "secret"));
    let response = authn::authenticate_headers(&state, &duplicate)
        .await
        .unwrap_err();
    assert_eq!(response.status, 401);
    assert!(response.body.unwrap().to_string().contains("multiple"));

    let mut non_utf8 = HeaderMap::new();
    non_utf8.insert(
        header::AUTHORIZATION,
        HeaderValue::from_bytes(b"Basic \xFF").unwrap(),
    );
    let response = authn::authenticate_headers(&state, &non_utf8)
        .await
        .unwrap_err();
    assert_eq!(response.status, 401);
}

#[tokio::test]
async fn missing_basic_auth_fails_before_request_body_handling() {
    let (state, _users) = secured_state(Role::ReadOnly);
    let headers = HeaderMap::new();

    let response = authn::authenticate_headers(&state, &headers)
        .await
        .unwrap_err();

    assert_eq!(response.status, 401);
    assert_eq!(
        response.headers.get("www-authenticate").map(String::as_str),
        Some("Basic realm=\"opensearch-lite\"")
    );
}

#[test]
fn read_only_authorization_allows_read_post_apis_and_blocks_mutations() {
    let context = SecurityContext::secured(
        Principal {
            username: "reader".to_string(),
            roles: vec![Role::ReadOnly],
        },
        false,
    );

    for (method, path) in [
        (Method::GET, "/"),
        (Method::GET, "/orders/_doc/1"),
        (Method::POST, "/orders/_search"),
        (Method::POST, "/orders/_count"),
        (Method::POST, "/_mget"),
        (Method::POST, "/_msearch"),
    ] {
        let request = request_with_context(method.clone(), path, context.clone());
        let route = classify(&method, path);
        authz::authorize(&request, &route).unwrap();
    }

    for (method, path) in [
        (Method::PUT, "/orders/_doc/1"),
        (Method::POST, "/_bulk"),
        (Method::POST, "/orders/_refresh"),
        (Method::PUT, "/orders/_mapping"),
        (Method::PUT, "/orders/_settings"),
        (Method::PUT, "/_index_template/orders"),
        (Method::POST, "/_aliases"),
    ] {
        let request = request_with_context(method.clone(), path, context.clone());
        let route = classify(&method, path);
        let response = authz::authorize(&request, &route).unwrap_err();
        assert_eq!(response.status, 403, "{method} {path}");
    }
}

#[test]
fn security_and_control_namespaces_are_not_fallback_eligible() {
    for path in [
        "/_plugins/_security/authinfo",
        "/_opendistro/_security/authinfo",
        "/_security/user",
        "/_snapshot/repo",
        "/_tasks",
    ] {
        let route = classify(&Method::GET, path);
        assert_ne!(route.tier, Tier::AgentRead, "{path}");
        assert_eq!(route.access, AccessClass::Admin, "{path}");
    }
}

fn request_with_context(method: Method, path: &str, context: SecurityContext) -> Request {
    Request::from_parts_with_security(
        method,
        path.parse::<Uri>().unwrap(),
        HeaderMap::new(),
        Bytes::new(),
        context,
    )
}
