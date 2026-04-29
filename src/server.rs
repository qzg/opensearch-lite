use std::{io, sync::Arc};

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, Method, Uri},
    routing::any,
    Router,
};
use tower::limit::ConcurrencyLimitLayer;

use crate::{
    agent::AgentClient,
    config::Config,
    http::{request::Request, router},
    responses::Response,
    storage::Store,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub agent: AgentClient,
}

impl AppState {
    pub fn new(config: Config) -> io::Result<Self> {
        let store = Store::open(&config)?;
        let agent = AgentClient::from_config(&config.agent);
        Ok(Self {
            config: Arc::new(config),
            store,
            agent,
        })
    }

    pub fn with_agent(config: Config, agent: AgentClient) -> io::Result<Self> {
        let store = Store::open(&config)?;
        Ok(Self {
            config: Arc::new(config),
            store,
            agent,
        })
    }
}

pub async fn run(config: Config) -> io::Result<()> {
    let state = AppState::new(config.clone())?;
    let app = app(state, &config);
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let bound_addr = listener.local_addr()?;
    eprintln!(
        "opensearch-lite listening on {bound_addr} (OpenSearch {} compatible, agent fallback {})",
        config.advertised_version,
        if config.agent.enabled() {
            "configured"
        } else {
            "disabled"
        }
    );
    axum::serve(listener, app).await
}

pub fn app(state: AppState, config: &Config) -> Router {
    Router::new()
        .fallback(any(handle))
        .with_state(state)
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(ConcurrencyLimitLayer::new(config.connection_limit))
}

async fn handle(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = Request::from_parts(method, uri, headers, body);
    router::handle(state, request).await
}
