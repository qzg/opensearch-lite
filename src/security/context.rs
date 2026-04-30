use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityMode {
    DisabledLoopback,
    InsecureNonLoopback,
    Secured,
}

impl SecurityMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DisabledLoopback => "disabled_loopback",
            Self::InsecureNonLoopback => "insecure_non_loopback",
            Self::Secured => "secured",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    ReadWrite,
    ReadOnly,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::ReadWrite => "read_write",
            Self::ReadOnly => "read_only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub username: String,
    pub roles: Vec<Role>,
}

impl Principal {
    pub fn has_role(&self, role: Role) -> bool {
        self.roles.contains(&role)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityContext {
    pub mode: SecurityMode,
    pub principal: Option<Principal>,
    pub client_cert_verified: bool,
}

impl SecurityContext {
    pub fn disabled_loopback() -> Self {
        Self {
            mode: SecurityMode::DisabledLoopback,
            principal: None,
            client_cert_verified: false,
        }
    }

    pub fn insecure_non_loopback() -> Self {
        Self {
            mode: SecurityMode::InsecureNonLoopback,
            principal: None,
            client_cert_verified: false,
        }
    }

    pub fn secured(principal: Principal, client_cert_verified: bool) -> Self {
        Self {
            mode: SecurityMode::Secured,
            principal: Some(principal),
            client_cert_verified,
        }
    }

    pub fn allows_all(&self) -> bool {
        matches!(
            self.mode,
            SecurityMode::DisabledLoopback | SecurityMode::InsecureNonLoopback
        )
    }

    pub fn is_admin(&self) -> bool {
        self.principal
            .as_ref()
            .map(|principal| principal.has_role(Role::Admin))
            .unwrap_or(false)
    }

    pub fn can_write(&self) -> bool {
        self.principal
            .as_ref()
            .map(|principal| principal.has_role(Role::Admin) || principal.has_role(Role::ReadWrite))
            .unwrap_or(false)
    }

    pub fn can_read(&self) -> bool {
        self.allows_all()
            || self
                .principal
                .as_ref()
                .map(|principal| {
                    principal.has_role(Role::Admin)
                        || principal.has_role(Role::ReadWrite)
                        || principal.has_role(Role::ReadOnly)
                })
                .unwrap_or(false)
    }
}
