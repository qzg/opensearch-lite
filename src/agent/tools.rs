use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    agent::errors::AgentError,
    storage::{mutation_log::Mutation, Store, StoreError},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolExecutionSummary {
    pub committed: bool,
    pub commit_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWriteScope {
    api_name: String,
    target_name: Option<String>,
    request_body: Option<Value>,
}

#[derive(Debug, Clone)]
pub enum ToolExecutionError {
    Agent(AgentError),
    Store(StoreError),
}

impl AgentWriteScope {
    pub fn legacy_template(name: impl Into<String>, request_body: Value) -> Self {
        Self {
            api_name: "indices.put_template".to_string(),
            target_name: Some(name.into()),
            request_body: Some(request_body),
        }
    }
}

impl From<AgentError> for ToolExecutionError {
    fn from(error: AgentError) -> Self {
        Self::Agent(error)
    }
}

impl From<StoreError> for ToolExecutionError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

pub fn tool_catalog(api_name: &str, write_enabled: bool) -> Value {
    let mut tools = vec![
        json!({
            "name": "inspect_catalog",
            "mode": "read",
            "description": "Inspect the bounded catalog and document summaries included in this fallback context."
        }),
        json!({
            "name": "construct_error",
            "mode": "read",
            "description": "Return an OpenSearch-shaped error when the request cannot be handled safely."
        }),
        json!({
            "name": "validate_query",
            "mode": "read",
            "description": "Validate a bounded OpenSearch query using the same local guardrails as search, count, explain, and by-query APIs."
        }),
        json!({
            "name": "analyze_text",
            "mode": "read",
            "description": "Tokenize text with OpenSearch Lite's development-scale standard, simple, whitespace, or keyword analyzer behavior."
        }),
        json!({
            "name": "explain_match",
            "mode": "read",
            "description": "Explain whether a stored document matches a supported local query shape."
        }),
    ];
    if write_enabled {
        tools.push(json!({
            "name": "commit_mutations",
            "mode": "write",
            "description": "Commit server-validated storage mutations. This is the only durable write boundary available to the fallback model. Mutations must match the current request target exactly.",
            "allowed_for_api": api_name,
            "arguments_schema": {
                "type": "object",
                "required": ["mutations"],
                "properties": {
                    "mutations": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "description": "A storage mutation_log::Mutation JSON object using its snake_case kind tag."
                        }
                    }
                }
            },
            "example_for_indices.put_template": {
                "mutations": [{
                    "kind": "put_registry_object",
                    "namespace": "legacy_template",
                    "name": "<template name from request path>",
                    "raw": "<exact JSON request body, unchanged>"
                }]
            }
        }));
    }
    Value::Array(tools)
}
pub fn apply_tool_calls(
    store: &Store,
    scope: &AgentWriteScope,
    calls: &[AgentToolCall],
) -> Result<ToolExecutionSummary, ToolExecutionError> {
    let mut all_mutations = Vec::new();
    let mut commit_call_count = 0usize;

    for call in calls {
        match call.name.as_str() {
            "commit_mutations" => {
                commit_call_count = commit_call_count.saturating_add(1);
                all_mutations.extend(parse_mutations(&call.arguments)?);
            }
            "inspect_catalog" | "construct_error" | "validate_query" | "analyze_text"
            | "explain_match" => {}
            other => {
                return Err(AgentError::new(
                    format!("unsupported agent tool [{other}]"),
                    "Use only the tools listed in the fallback context.",
                )
                .into());
            }
        }
    }

    validate_mutation_scope(scope, commit_call_count, &all_mutations)?;

    let mut summary = ToolExecutionSummary::default();
    if !all_mutations.is_empty() {
        summary.commit_count = all_mutations.len();
        store.commit_mutations(all_mutations)?;
        summary.committed = true;
    }
    Ok(summary)
}

fn parse_mutations(arguments: &Value) -> Result<Vec<Mutation>, AgentError> {
    let Some(raw_mutations) = arguments.get("mutations") else {
        return Err(AgentError::new(
            "commit_mutations was missing arguments.mutations",
            "Retry with a commit_mutations tool call containing a mutations array.",
        ));
    };
    let mutations =
        serde_json::from_value::<Vec<Mutation>>(raw_mutations.clone()).map_err(|error| {
            AgentError::new(
                format!("commit_mutations arguments were invalid: {error}"),
                "Use the documented storage mutation JSON shape.",
            )
        })?;
    if mutations.is_empty() {
        return Err(AgentError::new(
            "commit_mutations cannot commit an empty mutation list",
            "Return an error response if no write is required.",
        ));
    }
    Ok(mutations)
}

fn validate_mutation_scope(
    scope: &AgentWriteScope,
    commit_call_count: usize,
    mutations: &[Mutation],
) -> Result<(), AgentError> {
    if mutations.is_empty() {
        return Ok(());
    }
    if commit_call_count != 1 {
        return Err(AgentError::new(
            "write fallback requires exactly one commit_mutations call",
            "Retry with one commit_mutations call containing the complete atomic mutation list.",
        ));
    }
    if scope.api_name == "indices.put_template" {
        if mutations.len() != 1 {
            return Err(AgentError::new(
                "indices.put_template write fallback accepts exactly one registry mutation",
                "Commit only the legacy template named in the request path.",
            ));
        }
        let Some(expected_name) = scope.target_name.as_deref() else {
            return Err(AgentError::new(
                "indices.put_template write fallback was missing a request target",
                "Retry against /_template/{name}.",
            ));
        };
        let Some(expected_raw) = scope.request_body.as_ref() else {
            return Err(AgentError::new(
                "indices.put_template write fallback was missing the request body",
                "Retry with a JSON request body.",
            ));
        };
        if matches!(
            &mutations[0],
            Mutation::PutRegistryObject { namespace, name, raw }
                if namespace == "legacy_template" && name == expected_name && raw == expected_raw
        ) {
            return Ok(());
        }
    }
    Err(AgentError::new(
        format!(
            "mutation kind or target is not allowed for write fallback route [{}]",
            scope.api_name
        ),
        "Use only the mutation shape and target named in the fallback tool catalog.",
    ))
}
