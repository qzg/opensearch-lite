#![allow(clippy::field_reassign_with_default)]

use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Argon2,
};
use http::StatusCode;
use opensearch_lite::{
    security::{self, Role, TlsConfig},
    server::{self, AppState},
    Config,
};
use rcgen::{generate_simple_self_signed, CertifiedKey};
use serde_json::json;

fn password_hash(password: &str) -> String {
    let salt = SaltString::encode_b64(b"opensearch-lite").unwrap();
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

fn tls_files(temp: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    tls_files_named(temp, "server")
}

fn tls_files_named(
    temp: &tempfile::TempDir,
    name: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_path = temp.path().join(format!("{name}-cert.pem"));
    let key_path = temp.path().join(format!("{name}-key.pem"));
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, signing_key.serialize_pem()).unwrap();
    (cert_path, key_path)
}

fn users_file(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let users_path = temp.path().join("users.json");
    let body = json!({
        "users": [{
            "username": "kiyu",
            "password_hash": password_hash("secret"),
            "roles": [Role::Admin.as_str()]
        }]
    });
    std::fs::write(&users_path, serde_json::to_vec(&body).unwrap()).unwrap();
    users_path
}

#[tokio::test]
async fn https_listener_accepts_basic_auth_with_trusted_cert() {
    let temp = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = tls_files(&temp);
    let users_path = users_file(&temp);

    let mut config = Config::default();
    config.ephemeral = true;
    config.security.tls = Some(TlsConfig {
        cert_file: cert_path.clone(),
        key_file: key_path,
        server_ca_file: Some(cert_path.clone()),
    });
    config.security.users_file = Some(users_path);

    let state = AppState::new(config.clone()).unwrap();
    let app = server::app(state, &config);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let tls_config = security::tls::rustls_config(&config.security)
        .unwrap()
        .unwrap();

    let server = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls_config)
            .unwrap()
            .serve(app.into_make_service())
            .await
            .unwrap();
    });

    let ca = reqwest::Certificate::from_pem(&std::fs::read(&cert_path).unwrap()).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(ca)
        .build()
        .unwrap();
    let response = client
        .get(format!("https://localhost:{}/", addr.port()))
        .basic_auth("kiyu", Some("secret"))
        .send()
        .await
        .unwrap();

    server.abort();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["version"]["distribution"], "opensearch");
}

#[test]
fn tls_validation_rejects_missing_files_without_secret_material() {
    let mut config = Config::default();
    config.security.tls = Some(TlsConfig {
        cert_file: "missing-cert.pem".into(),
        key_file: "missing-key.pem".into(),
        server_ca_file: None,
    });

    let error = security::tls::validate(&config.security).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("missing-cert.pem"));
    assert!(!message.contains("secret"));
}

#[test]
fn config_validation_rejects_mismatched_certificate_and_key() {
    let temp = tempfile::tempdir().unwrap();
    let (cert_path, _key_path) = tls_files_named(&temp, "first");
    let (_other_cert_path, other_key_path) = tls_files_named(&temp, "second");

    let mut config = Config::default();
    config.security.tls = Some(TlsConfig {
        cert_file: cert_path,
        key_file: other_key_path,
        server_ca_file: None,
    });

    let error = server::validate_config(&config).unwrap_err();
    assert!(!error.to_string().contains("opensearch-lite-smoke-password"));
}
