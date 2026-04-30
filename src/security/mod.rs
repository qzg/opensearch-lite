pub mod authn;
pub mod authz;
pub mod config;
pub mod context;
pub mod diagnostics;
pub mod tls;
pub mod users;

use std::{io, sync::Arc};

use crate::config::Config;

pub use config::{SecurityConfig, TlsConfig};
pub use context::{Principal, Role, SecurityContext, SecurityMode};
pub use users::UserStore;

#[derive(Debug, Clone)]
pub struct SecurityState {
    mode: SecurityMode,
    users: Option<Arc<UserStore>>,
    auth_failure_delay: std::time::Duration,
}

impl SecurityState {
    pub fn from_config(config: &Config) -> io::Result<Self> {
        let users = match &config.security.users_file {
            Some(path) => Some(Arc::new(UserStore::from_file(path)?)),
            None => None,
        };
        let mode = if users.is_some() {
            SecurityMode::Secured
        } else if !config.listen.ip().is_loopback() && config.security.allow_insecure_non_loopback {
            SecurityMode::InsecureNonLoopback
        } else {
            SecurityMode::DisabledLoopback
        };

        Ok(Self {
            mode,
            users,
            auth_failure_delay: config.security.auth_failure_delay,
        })
    }

    pub fn mode(&self) -> SecurityMode {
        self.mode
    }

    pub fn users(&self) -> Option<&Arc<UserStore>> {
        self.users.as_ref()
    }

    pub fn auth_required(&self) -> bool {
        self.users.is_some()
    }

    pub fn auth_failure_delay(&self) -> std::time::Duration {
        self.auth_failure_delay
    }
}
