use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    agent::errors::AgentError,
    responses::{open_search_error, Response},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponseWrapper {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub body: Value,
    pub confidence: u8,
    #[serde(default)]
    pub failure_reason: Option<String>,
    pub read_only: bool,
}

pub fn validate_wrapper(
    raw: &str,
    confidence_threshold: u8,
    response_limit_bytes: usize,
) -> Result<Response, AgentError> {
    if raw.len() > response_limit_bytes {
        return Err(AgentError::new(
            "agent response exceeded configured size limit",
            "Ask for a narrower read request or raise --agent-response-limit for this local run.",
        ));
    }
    let wrapper: AgentResponseWrapper = serde_json::from_str(raw).map_err(|error| {
        AgentError::new(
            format!("agent response was not a valid wrapper JSON object: {error}"),
            "Retry with a simpler read request or disable fallback for this route.",
        )
    })?;
    if !wrapper.read_only {
        return Err(AgentError::new(
            "agent response declared write intent",
            "Use a deterministic implemented write API or adjust the request to be read-only.",
        ));
    }
    if wrapper.confidence < confidence_threshold {
        return Err(AgentError::new(
            wrapper
                .failure_reason
                .unwrap_or_else(|| "agent confidence was below threshold".to_string()),
            "Narrow the query, target a specific index, or use an implemented OpenSearch API.",
        ));
    }
    if !(200..600).contains(&wrapper.status) {
        return Err(AgentError::new(
            "agent wrapper status was outside HTTP status range",
            "Retry with a simpler read request.",
        ));
    }
    let mut response = Response::json(wrapper.status, wrapper.body)
        .compatibility_signal("agent.read", "agent_fallback_eligible");
    for (name, value) in wrapper.headers {
        if !name.eq_ignore_ascii_case("content-length") {
            response = response.header(name, value);
        }
    }
    Ok(response)
}

pub fn failure_response(error: AgentError) -> Response {
    open_search_error(
        501,
        "opensearch_lite_agent_fallback_exception",
        error.reason,
        Some(&error.hint),
    )
}
