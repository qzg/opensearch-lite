use std::{env, fs};

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;

use crate::{
    agent::{
        config::AgentConfig,
        context::AgentRequestContext,
        errors::AgentError,
        prompt,
        validation::{parse_wrapper, validate_wrapper_value, AgentResponseWrapper, ValidationMode},
    },
    responses::Response,
};

#[derive(Clone)]
pub enum AgentClient {
    Disabled,
    Http(HttpAgentClient),
    Static(AgentResponseWrapper),
}

#[derive(Clone)]
pub struct HttpAgentClient {
    config: AgentConfig,
}

impl AgentClient {
    pub fn from_config(config: &AgentConfig) -> Self {
        if config.endpoint.is_some() {
            Self::Http(HttpAgentClient {
                config: config.clone(),
            })
        } else {
            Self::Disabled
        }
    }

    pub fn disabled() -> Self {
        Self::Disabled
    }

    pub fn static_response(wrapper: AgentResponseWrapper) -> Self {
        Self::Static(wrapper)
    }

    pub async fn complete(&self, context: AgentRequestContext) -> Result<Response, AgentError> {
        let wrapper = self.complete_raw(context).await?;
        let threshold = match self {
            Self::Disabled => 1,
            Self::Http(client) => client.config.confidence_threshold,
            Self::Static(_) => 1,
        };
        validate_wrapper_value(wrapper, threshold, ValidationMode::ReadOnly)
    }

    pub async fn complete_raw(
        &self,
        context: AgentRequestContext,
    ) -> Result<AgentResponseWrapper, AgentError> {
        match self {
            Self::Disabled => Err(AgentError::new(
                "agent fallback is disabled because no endpoint is configured",
                "Use an implemented API, simplify the query, or configure --agent-endpoint.",
            )),
            Self::Static(wrapper) => Ok(wrapper.clone()),
            Self::Http(client) => client.complete_raw(context).await,
        }
    }
}

impl HttpAgentClient {
    async fn complete_raw(
        &self,
        context: AgentRequestContext,
    ) -> Result<AgentResponseWrapper, AgentError> {
        let endpoint = self.config.endpoint.as_ref().ok_or_else(|| {
            AgentError::new("agent endpoint missing", "configure --agent-endpoint")
        })?;
        let context_bytes = serde_json::to_vec(&context).map_err(|error| {
            AgentError::new(
                format!("failed to encode fallback context: {error}"),
                "retry",
            )
        })?;
        if context_bytes.len() > self.config.context_limit_bytes {
            return Err(AgentError::new(
                "fallback context exceeded configured size limit",
                "Target a narrower index or query, or raise --agent-context-limit for this local run.",
            ));
        }

        let mut request = reqwest::Client::builder()
            .timeout(self.config.timeout)
            .build()
            .map_err(|error| {
                AgentError::new(format!("failed to build agent client: {error}"), "retry")
            })?
            .post(endpoint)
            .header(CONTENT_TYPE, "application/json");

        if let Some(token) = self.load_token()? {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }

        let body = serde_json::json!({
            "model": self.config.model.as_deref().unwrap_or("opensearch-lite-fallback"),
            "messages": prompt::messages(&context),
            "temperature": 0,
            "max_tokens": 1600,
            "response_format": { "type": "json_object" }
        });
        let response = request.json(&body).send().await.map_err(|error| {
            AgentError::new(
                format!("agent endpoint request failed: {error}"),
                "Check endpoint, auth, and network access.",
            )
        })?;

        let status = response.status();
        let value: Value = response.json().await.map_err(|error| {
            AgentError::new(
                format!("agent endpoint returned non-JSON response: {error}"),
                "Use an OpenAI-compatible chat endpoint.",
            )
        })?;
        if !status.is_success() {
            return Err(AgentError::new(
                format!("agent endpoint returned HTTP {status}"),
                "Check endpoint, auth, model, and request size.",
            ));
        }
        let content = value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AgentError::new(
                    "agent endpoint response did not include choices[0].message.content",
                    "Use an OpenAI-compatible chat completion response shape.",
                )
            })?;
        parse_wrapper(content, self.config.response_limit_bytes)
    }

    fn load_token(&self) -> Result<Option<String>, AgentError> {
        if let Some(env_name) = &self.config.bearer_token_env {
            return env::var(env_name)
                .map(|token| Some(token.trim().to_string()))
                .map_err(|_| {
                    AgentError::new(
                        "configured agent token environment variable was not set",
                        "Set the environment variable or remove --agent-token-env.",
                    )
                });
        }
        if let Some(path) = &self.config.bearer_token_file {
            return fs::read_to_string(path)
                .map(|token| Some(token.trim().to_string()))
                .map_err(|error| {
                    AgentError::new(
                        format!("failed to read configured agent token file: {error}"),
                        "Fix the secret file path or remove --agent-token-file.",
                    )
                });
        }
        Ok(None)
    }
}
