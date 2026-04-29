#[derive(Debug, Clone)]
pub struct AgentError {
    pub reason: String,
    pub hint: String,
}

impl AgentError {
    pub fn new(reason: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            hint: hint.into(),
        }
    }
}
