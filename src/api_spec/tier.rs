#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Implemented,
    BestEffort,
    AgentRead,
    Unsupported,
    OutsideIdentity,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::BestEffort => "best_effort",
            Self::AgentRead => "agent_fallback_eligible",
            Self::Unsupported => "unsupported",
            Self::OutsideIdentity => "outside_product_identity",
        }
    }
}
