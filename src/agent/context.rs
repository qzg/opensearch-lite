use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequestContext {
    pub method: String,
    pub path: String,
    pub query: Value,
    pub body: Value,
    pub api_name: String,
    pub route_tier: String,
    pub catalog: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub tools: Value,
}
