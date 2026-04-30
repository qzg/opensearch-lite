use std::{net::SocketAddr, path::PathBuf, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsConfig {
    pub cert_file: PathBuf,
    pub key_file: PathBuf,
    pub server_ca_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityConfig {
    pub tls: Option<TlsConfig>,
    pub users_file: Option<PathBuf>,
    pub allow_insecure_non_loopback: bool,
    pub require_client_cert: bool,
    pub client_cert_ca_file: Option<PathBuf>,
    pub auth_failure_delay: Duration,
    pub validate_only: bool,
}

impl SecurityConfig {
    pub fn validate_posture(
        &self,
        listen: SocketAddr,
        allow_nonlocal_listen: bool,
    ) -> Result<(), String> {
        if self.require_client_cert && self.tls.is_none() {
            return Err("--require-client-cert requires TLS to be configured".to_string());
        }
        if self.require_client_cert && self.client_cert_ca_file.is_none() {
            return Err("--require-client-cert requires --client-cert-ca-file".to_string());
        }
        if self.client_cert_ca_file.is_some() && self.tls.is_none() {
            return Err("--client-cert-ca-file requires TLS to be configured".to_string());
        }

        if !allow_nonlocal_listen || listen.ip().is_loopback() {
            return Ok(());
        }

        if self.allow_insecure_non_loopback {
            return Ok(());
        }

        match (self.tls.is_some(), self.users_file.is_some()) {
            (true, true) => Ok(()),
            (true, false) => Err("--listen is non-loopback; configure --users-file with TLS or pass --allow-insecure-non-loopback for an explicit insecure development exception".to_string()),
            (false, true) => Err("--listen is non-loopback; configure TLS with --tls-cert-file and --tls-key-file or pass --allow-insecure-non-loopback for an explicit insecure development exception".to_string()),
            (false, false) => Err("--listen is non-loopback; configure TLS and --users-file, or pass --allow-insecure-non-loopback for an explicit insecure development exception".to_string()),
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            tls: None,
            users_file: None,
            allow_insecure_non_loopback: false,
            require_client_cert: false,
            client_cert_ca_file: None,
            auth_failure_delay: Duration::from_millis(25),
            validate_only: false,
        }
    }
}
