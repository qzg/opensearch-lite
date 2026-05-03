use std::{io, net::TcpListener as StdTcpListener, sync::Arc};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, State},
    http::{HeaderMap, Method, Uri},
    middleware,
    routing::any,
    Router,
};
use tower::limit::ConcurrencyLimitLayer;

use crate::{
    agent::AgentClient,
    config::Config,
    http::{request::Request, router},
    resources,
    responses::Response,
    runtime::RuntimeState,
    security::{self, SecurityContext, SecurityState},
    snapshots::SnapshotService,
    storage::Store,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub agent: AgentClient,
    pub security: SecurityState,
    pub runtime: RuntimeState,
    pub snapshots: SnapshotService,
}

impl AppState {
    pub fn new(config: Config) -> io::Result<Self> {
        validate_state_security(&config)?;
        resources::validate(&config)?;
        let store = Store::open(&config)?;
        let agent = AgentClient::from_config(&config.agent);
        let security = SecurityState::from_config(&config)?;
        let snapshots = SnapshotService::from_config(&config);
        Ok(Self {
            config: Arc::new(config),
            store,
            agent,
            security,
            runtime: RuntimeState::default(),
            snapshots,
        })
    }

    pub fn with_agent(config: Config, agent: AgentClient) -> io::Result<Self> {
        validate_state_security(&config)?;
        resources::validate(&config)?;
        let store = Store::open(&config)?;
        let security = SecurityState::from_config(&config)?;
        let snapshots = SnapshotService::from_config(&config);
        Ok(Self {
            config: Arc::new(config),
            store,
            agent,
            security,
            runtime: RuntimeState::default(),
            snapshots,
        })
    }
}

pub async fn run(config: Config) -> io::Result<()> {
    validate_config(&config)?;
    let state = AppState::new(config.clone())?;
    let app = app(state, &config);
    if let Some(tls_config) = security::tls::rustls_config(&config.security)? {
        let listener = StdTcpListener::bind(config.listen)?;
        listener.set_nonblocking(true)?;
        let bound_addr = listener.local_addr()?;
        startup_log(&config, bound_addr);
        return axum_server::from_tcp_rustls(listener, tls_config)?
            .serve(app.into_make_service())
            .await;
    }

    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let bound_addr = listener.local_addr()?;
    startup_log(&config, bound_addr);
    axum::serve(listener, app).await
}

pub fn validate_config(config: &Config) -> io::Result<()> {
    validate_startup_config(config)?;
    let diagnostics = resources::validate(config)?;
    eprintln!(
        "opensearch-lite resource diagnostics: {}",
        diagnostics.summary()
    );
    security::diagnostics::validate(config)
}

fn validate_startup_config(config: &Config) -> io::Result<()> {
    config
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn validate_state_security(config: &Config) -> io::Result<()> {
    if !config.allow_nonlocal_listen && !config.listen.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "--listen {} is not loopback; pass --allow-nonlocal-listen and configure TLS/auth for workgroup use",
                config.listen
            ),
        ));
    }
    config
        .agent
        .validate()
        .and_then(|_| {
            config
                .security
                .validate_posture(config.listen, config.allow_nonlocal_listen)
        })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn startup_log(config: &Config, bound_addr: std::net::SocketAddr) {
    eprintln!(
        "opensearch-lite listening on {bound_addr} (OpenSearch {} compatible, security {}, agent fallback {})",
        config.advertised_version,
        if config.security.users_file.is_some() {
            "secured"
        } else if config.security.allow_insecure_non_loopback && !config.listen.ip().is_loopback() {
            "insecure-non-loopback"
        } else {
            "disabled-loopback"
        },
        if config.agent.enabled() {
            "configured"
        } else {
            "disabled"
        }
    );
}

pub fn app(state: AppState, config: &Config) -> Router {
    Router::new()
        .fallback(any(handle))
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(ConcurrencyLimitLayer::new(config.connection_limit))
        .layer(middleware::from_fn_with_state(
            state,
            security::authn::middleware,
        ))
}

async fn handle(
    State(state): State<AppState>,
    Extension(security): Extension<SecurityContext>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = Request::from_parts_with_security(method, uri, headers, body, security);
    router::handle(state, request).await
}
