#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Implemented,
    BestEffort,
    Mocked,
    AgentRead,
    AgentWrite,
    Unsupported,
    OutsideIdentity,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::BestEffort => "best_effort",
            Self::Mocked => "mocked",
            Self::AgentRead => "agent_fallback_eligible",
            Self::AgentWrite => "agent_write_fallback_eligible",
            Self::Unsupported => "unsupported",
            Self::OutsideIdentity => "outside_product_identity",
        }
    }
}
