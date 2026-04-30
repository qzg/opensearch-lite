use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    agent::{errors::AgentError, tools::AgentToolCall},
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
    #[serde(default)]
    pub tool_calls: Vec<AgentToolCall>,
}

pub fn validate_wrapper(
    raw: &str,
    confidence_threshold: u8,
    response_limit_bytes: usize,
) -> Result<Response, AgentError> {
    let wrapper = parse_wrapper(raw, response_limit_bytes)?;
    validate_wrapper_value(wrapper, confidence_threshold, ValidationMode::ReadOnly)
}

pub fn parse_wrapper(
    raw: &str,
    response_limit_bytes: usize,
) -> Result<AgentResponseWrapper, AgentError> {
    if raw.len() > response_limit_bytes {
        return Err(AgentError::new(
            "agent response exceeded configured size limit",
            "Ask for a narrower read request or raise --agent-response-limit for this local run.",
        ));
    }
    serde_json::from_str(raw).map_err(|error| {
        AgentError::new(
            format!("agent response was not a valid wrapper JSON object: {error}"),
            "Retry with a simpler read request or disable fallback for this route.",
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationMode {
    ReadOnly,
    Write { commit_performed: bool },
}

pub fn validate_write_wrapper_before_tools(
    wrapper: &AgentResponseWrapper,
    confidence_threshold: u8,
) -> Result<(), AgentError> {
    validate_confidence_and_status(wrapper, confidence_threshold)?;
    if wrapper.read_only && (200..300).contains(&wrapper.status) {
        return Err(AgentError::new(
            "write fallback success response declared read_only=true",
            "Return an error response or use commit_mutations and set read_only=false for successful write fallback.",
        ));
    }
    if !(200..300).contains(&wrapper.status) && !wrapper.tool_calls.is_empty() {
        return Err(AgentError::new(
            "agent write response included tool calls on a non-success response",
            "Return an error response without tool calls, or return a successful response with a valid commit_mutations call.",
        ));
    }
    Ok(())
}

pub fn validate_wrapper_value(
    wrapper: AgentResponseWrapper,
    confidence_threshold: u8,
    mode: ValidationMode,
) -> Result<Response, AgentError> {
    match mode {
        ValidationMode::ReadOnly if !wrapper.read_only => {
            return Err(AgentError::new(
                "agent response declared write intent",
                "Use a deterministic implemented write API or adjust the request to be read-only.",
            ));
        }
        ValidationMode::Write { commit_performed } => {
            if wrapper.read_only && (200..300).contains(&wrapper.status) {
                return Err(AgentError::new(
                    "write fallback success response declared read_only=true",
                    "Return an error response or use commit_mutations and set read_only=false for successful write fallback.",
                ));
            }
            if !commit_performed && !wrapper.read_only && (200..300).contains(&wrapper.status) {
                return Err(AgentError::new(
                    "agent write response claimed side effects without a successful commit",
                    "Use the commit_mutations tool before returning a successful write response.",
                ));
            }
        }
        _ => {}
    }
    validate_confidence_and_status(&wrapper, confidence_threshold)?;
    let headers = wrapper.headers.clone();
    let status = wrapper.status;
    let body = wrapper.body;
    let tier_signal = match mode {
        ValidationMode::ReadOnly => "agent_fallback_eligible",
        ValidationMode::Write { .. } => "agent_write_fallback_eligible",
    };
    let api_signal = match mode {
        ValidationMode::ReadOnly => "agent.read",
        ValidationMode::Write { .. } => "agent.write",
    };
    let mut response = Response::json(status, body);
    for (name, value) in headers {
        if allowed_agent_header(&name) {
            response = response.header(name.to_ascii_lowercase(), value);
        }
    }
    Ok(response.compatibility_signal(api_signal, tier_signal))
}

fn validate_confidence_and_status(
    wrapper: &AgentResponseWrapper,
    confidence_threshold: u8,
) -> Result<(), AgentError> {
    if wrapper.confidence < confidence_threshold {
        return Err(AgentError::new(
            wrapper
                .failure_reason
                .clone()
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
    Ok(())
}

fn allowed_agent_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(name.as_str(), "warning")
}

pub fn failure_response(error: AgentError) -> Response {
    open_search_error(
        501,
        "opensearch_lite_agent_fallback_exception",
        error.reason,
        Some(&error.hint),
    )
}
