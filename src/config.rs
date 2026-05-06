use std::{
    env,
    ffi::OsString,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use crate::security::{SecurityConfig, TlsConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub bearer_token_env: Option<String>,
    pub bearer_token_file: Option<PathBuf>,
    pub timeout: Duration,
    pub context_limit_bytes: usize,
    pub response_limit_bytes: usize,
    pub allow_insecure_endpoint: bool,
    pub confidence_threshold: u8,
    pub write_enabled: bool,
    pub write_allowlist: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub listen: SocketAddr,
    pub advertised_version: String,
    pub data_dir: PathBuf,
    pub ephemeral: bool,
    pub memory_limit_bytes: usize,
    pub max_body_bytes: usize,
    pub max_bulk_actions: usize,
    pub max_result_window: usize,
    pub max_indexes: usize,
    pub max_documents: usize,
    pub snapshot_write_threshold: usize,
    pub snapshot_interval: Duration,
    pub connection_limit: usize,
    pub allow_nonlocal_listen: bool,
    pub strict_compatibility: bool,
    pub strict_allowlist: Vec<String>,
    pub agent: AgentConfig,
    pub security: SecurityConfig,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_args(env::args_os())
    }

    pub fn from_args<I, T>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let mut config = Self::default();
        let mut tls_cert_file: Option<PathBuf> = None;
        let mut tls_key_file: Option<PathBuf> = None;
        let mut tls_ca_file: Option<PathBuf> = None;
        let mut args = args.into_iter().map(Into::into);
        let _program = args.next();

        while let Some(arg) = args.next() {
            let arg = arg
                .into_string()
                .map_err(|_| "arguments must be valid UTF-8".to_string())?;

            if arg == "--help" || arg == "-h" {
                return Err(Self::usage());
            }

            let (flag, inline_value) = split_flag_value(&arg);
            match flag {
                "--ephemeral" => {
                    reject_inline_value(flag, inline_value)?;
                    config.ephemeral = true;
                    continue;
                }
                "--allow-nonlocal-listen" => {
                    reject_inline_value(flag, inline_value)?;
                    config.allow_nonlocal_listen = true;
                    continue;
                }
                "--strict-compatibility" => {
                    reject_inline_value(flag, inline_value)?;
                    config.strict_compatibility = true;
                    continue;
                }
                "--agent-allow-insecure-endpoint" => {
                    reject_inline_value(flag, inline_value)?;
                    config.agent.allow_insecure_endpoint = true;
                    continue;
                }
                "--agent-enable-write-fallback" => {
                    reject_inline_value(flag, inline_value)?;
                    config.agent.write_enabled = true;
                    continue;
                }
                "--allow-insecure-non-loopback" => {
                    reject_inline_value(flag, inline_value)?;
                    config.security.allow_insecure_non_loopback = true;
                    continue;
                }
                "--require-client-cert" => {
                    reject_inline_value(flag, inline_value)?;
                    config.security.require_client_cert = true;
                    continue;
                }
                "--validate-config" => {
                    reject_inline_value(flag, inline_value)?;
                    config.security.validate_only = true;
                    continue;
                }
                "--listen"
                | "--advertised-version"
                | "--data-dir"
                | "--memory-limit"
                | "--max-body-size"
                | "--max-bulk-actions"
                | "--max-result-window"
                | "--max-indexes"
                | "--max-documents"
                | "--snapshot-write-threshold"
                | "--snapshot-interval-secs"
                | "--connection-limit"
                | "--strict-allowlist"
                | "--agent-endpoint"
                | "--agent-model"
                | "--agent-token-env"
                | "--agent-token-file"
                | "--agent-timeout-ms"
                | "--agent-context-limit"
                | "--agent-response-limit"
                | "--agent-confidence-threshold"
                | "--agent-write-allowlist"
                | "--tls-cert-file"
                | "--tls-key-file"
                | "--tls-ca-file"
                | "--users-file"
                | "--client-cert-ca-file"
                | "--auth-failure-delay-ms" => {}
                _ => return Err(format!("unknown argument {flag:?}\n\n{}", Self::usage())),
            }

            let value = match inline_value {
                Some(value) => value.to_string(),
                None => args
                    .next()
                    .ok_or_else(|| format!("missing value for {flag}"))?
                    .into_string()
                    .map_err(|_| format!("{flag} value must be valid UTF-8"))?,
            };

            match flag {
                "--listen" => {
                    config.listen = value
                        .parse()
                        .map_err(|error| format!("invalid --listen value {value:?}: {error}"))?;
                }
                "--advertised-version" => config.advertised_version = value,
                "--data-dir" => config.data_dir = PathBuf::from(value),
                "--memory-limit" => config.memory_limit_bytes = parse_bytes(&value)?,
                "--max-body-size" => config.max_body_bytes = parse_bytes(&value)?,
                "--max-bulk-actions" => {
                    config.max_bulk_actions = parse_positive_usize(flag, &value)?
                }
                "--max-result-window" => {
                    config.max_result_window = parse_positive_usize(flag, &value)?
                }
                "--max-indexes" => config.max_indexes = parse_positive_usize(flag, &value)?,
                "--max-documents" => config.max_documents = parse_positive_usize(flag, &value)?,
                "--snapshot-write-threshold" => {
                    config.snapshot_write_threshold = parse_positive_usize(flag, &value)?
                }
                "--snapshot-interval-secs" => {
                    let secs = parse_positive_u64(flag, &value)?;
                    config.snapshot_interval = Duration::from_secs(secs);
                }
                "--connection-limit" => {
                    config.connection_limit = parse_positive_usize(flag, &value)?
                }
                "--strict-allowlist" => {
                    config.strict_allowlist = value
                        .split(',')
                        .filter(|entry| !entry.trim().is_empty())
                        .map(|entry| entry.trim().to_string())
                        .collect();
                }
                "--agent-endpoint" => config.agent.endpoint = Some(value),
                "--agent-model" => config.agent.model = Some(value),
                "--agent-token-env" => config.agent.bearer_token_env = Some(value),
                "--agent-token-file" => config.agent.bearer_token_file = Some(PathBuf::from(value)),
                "--agent-timeout-ms" => {
                    let ms = parse_positive_u64(flag, &value)?;
                    config.agent.timeout = Duration::from_millis(ms);
                }
                "--agent-context-limit" => config.agent.context_limit_bytes = parse_bytes(&value)?,
                "--agent-response-limit" => {
                    config.agent.response_limit_bytes = parse_bytes(&value)?
                }
                "--agent-confidence-threshold" => {
                    let threshold = parse_positive_usize(flag, &value)?;
                    if threshold > 100 {
                        return Err(
                            "--agent-confidence-threshold must be between 1 and 100".to_string()
                        );
                    }
                    config.agent.confidence_threshold = threshold as u8;
                }
                "--agent-write-allowlist" => {
                    config.agent.write_allowlist = value
                        .split(',')
                        .filter(|entry| !entry.trim().is_empty())
                        .map(|entry| entry.trim().to_string())
                        .collect();
                }
                "--tls-cert-file" => tls_cert_file = Some(PathBuf::from(value)),
                "--tls-key-file" => tls_key_file = Some(PathBuf::from(value)),
                "--tls-ca-file" => tls_ca_file = Some(PathBuf::from(value)),
                "--users-file" => config.security.users_file = Some(PathBuf::from(value)),
                "--client-cert-ca-file" => {
                    config.security.client_cert_ca_file = Some(PathBuf::from(value))
                }
                "--auth-failure-delay-ms" => {
                    let ms = parse_positive_u64(flag, &value)?;
                    config.security.auth_failure_delay = Duration::from_millis(ms);
                }
                _ => unreachable!("unknown flags are rejected before value parsing"),
            }
        }

        if tls_cert_file.is_some() || tls_key_file.is_some() || tls_ca_file.is_some() {
            let cert_file = tls_cert_file
                .ok_or_else(|| "--tls-key-file requires --tls-cert-file".to_string())?;
            let key_file = tls_key_file
                .ok_or_else(|| "--tls-cert-file requires --tls-key-file".to_string())?;
            config.security.tls = Some(TlsConfig {
                cert_file,
                key_file,
                server_ca_file: tls_ca_file,
            });
        }

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.memory_limit_bytes == 0 {
            return Err("--memory-limit must be positive".to_string());
        }
        if self.max_body_bytes == 0 {
            return Err("--max-body-size must be positive".to_string());
        }
        if self.max_body_bytes > self.memory_limit_bytes {
            return Err("--max-body-size cannot exceed --memory-limit".to_string());
        }
        if self.max_bulk_actions == 0
            || self.max_result_window == 0
            || self.max_indexes == 0
            || self.max_documents == 0
            || self.snapshot_write_threshold == 0
            || self.snapshot_interval.is_zero()
            || self.connection_limit == 0
        {
            return Err("count limits must be positive".to_string());
        }
        if !self.allow_nonlocal_listen && !self.listen.ip().is_loopback() {
            return Err(format!(
                "--listen {} is not loopback; pass --allow-nonlocal-listen and configure TLS/auth for workgroup use",
                self.listen
            ));
        }
        self.agent.validate()?;
        self.security
            .validate_posture(self.listen, self.allow_nonlocal_listen)?;
        Ok(())
    }

    pub fn usage() -> String {
        [
            "Usage: mainstack-search [OPTIONS]",
            "",
            "Options:",
            "  --listen <addr>                         TCP listen address [default: 127.0.0.1:9200]",
            "  --advertised-version <version>          Version reported to clients [default: 3.6.0]",
            "  --data-dir <path>                       Durable local data directory [default: ./data]",
            "  --ephemeral                             Keep state in memory only",
            "  --memory-limit <bytes>                  Soft memory target [default: 512MiB]",
            "  --max-body-size <bytes>                 Maximum HTTP request body [default: 16MiB]",
            "  --max-bulk-actions <count>              Maximum NDJSON bulk actions [default: 10000]",
            "  --max-result-window <count>             Maximum search from+size [default: 10000]",
            "  --max-indexes <count>                   Maximum local indexes [default: 1024]",
            "  --max-documents <count>                 Maximum stored documents [default: 1000000]",
            "  --snapshot-write-threshold <count>      Flush snapshot after this many dirty writes [default: 1000]",
            "  --snapshot-interval-secs <seconds>      Flush snapshot after this dirty interval [default: 600]",
            "  --connection-limit <count>              Maximum concurrent connections [default: 128]",
            "  --allow-nonlocal-listen                 Permit binding to non-loopback addresses",
            "  --allow-insecure-non-loopback           Permit non-loopback HTTP/no-auth development mode",
            "  --tls-cert-file <path>                  PEM certificate chain for HTTPS",
            "  --tls-key-file <path>                   PEM private key for HTTPS",
            "  --tls-ca-file <path>                    CA bundle clients should trust",
            "  --users-file <path>                     JSON users file with PHC password hashes",
            "  --require-client-cert                   Require a client certificate at TLS handshake",
            "  --client-cert-ca-file <path>            CA bundle for verifying client certificates",
            "  --auth-failure-delay-ms <ms>            Delay after invalid credentials [default: 25]",
            "  --validate-config                       Validate TLS/auth config and exit",
            "  --strict-compatibility                  Fail best-effort/fallback routes unless allowlisted",
            "  --strict-allowlist <api,api>            Route names allowed in strict compatibility mode",
            "  --agent-endpoint <url>                  OpenAI-compatible chat endpoint",
            "  --agent-model <model>                   Model name for fallback requests",
            "  --agent-token-env <name>                Environment variable containing bearer token",
            "  --agent-token-file <path>               File containing bearer token",
            "  --agent-timeout-ms <ms>                 Agent request timeout [default: 10000]",
            "  --agent-context-limit <bytes>           Agent context byte limit [default: 1MiB]",
            "  --agent-response-limit <bytes>          Agent response byte limit [default: 1MiB]",
            "  --agent-confidence-threshold <1-100>    Minimum fallback confidence [default: 75]",
            "  --agent-allow-insecure-endpoint         Permit non-loopback http:// agent endpoint",
            "  --agent-enable-write-fallback           Permit eligible write fallback routes to use agent tools",
            "  --agent-write-allowlist <api,api>       Write fallback API names allowed when write fallback is enabled",
            "  -h, --help                              Show this help",
        ]
        .join("\n")
    }
}

impl AgentConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.context_limit_bytes == 0 {
            return Err("--agent-context-limit must be positive".to_string());
        }
        if self.response_limit_bytes == 0 {
            return Err("--agent-response-limit must be positive".to_string());
        }
        if let Some(endpoint) = &self.endpoint {
            validate_agent_endpoint(endpoint, self.allow_insecure_endpoint)?;
        }
        Ok(())
    }

    pub fn enabled(&self) -> bool {
        self.endpoint.is_some()
    }

    pub fn write_enabled_for(&self, api_name: &str) -> bool {
        self.write_enabled
            && self.enabled()
            && self
                .write_allowlist
                .iter()
                .any(|allowed| allowed == api_name)
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            model: None,
            bearer_token_env: None,
            bearer_token_file: None,
            timeout: Duration::from_secs(10),
            context_limit_bytes: 1024 * 1024,
            response_limit_bytes: 1024 * 1024,
            allow_insecure_endpoint: false,
            confidence_threshold: 75,
            write_enabled: false,
            write_allowlist: Vec::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:9200"
                .parse()
                .expect("default listen address is valid"),
            advertised_version: "3.6.0".to_string(),
            data_dir: PathBuf::from("./data"),
            ephemeral: false,
            memory_limit_bytes: 512 * 1024 * 1024,
            max_body_bytes: 16 * 1024 * 1024,
            max_bulk_actions: 10_000,
            max_result_window: 10_000,
            max_indexes: 1024,
            max_documents: 1_000_000,
            snapshot_write_threshold: 1_000,
            snapshot_interval: Duration::from_secs(600),
            connection_limit: 128,
            allow_nonlocal_listen: false,
            strict_compatibility: false,
            strict_allowlist: Vec::new(),
            agent: AgentConfig::default(),
            security: SecurityConfig::default(),
        }
    }
}

pub fn parse_bytes(value: &str) -> Result<usize, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("byte value cannot be empty".to_string());
    }

    let lower = trimmed.to_ascii_lowercase();
    let (number, multiplier) = if let Some(number) = lower.strip_suffix("kib") {
        (number, 1024usize)
    } else if let Some(number) = lower.strip_suffix("kb") {
        (number, 1024usize)
    } else if let Some(number) = lower.strip_suffix('k') {
        (number, 1024usize)
    } else if let Some(number) = lower.strip_suffix("mib") {
        (number, 1024usize * 1024)
    } else if let Some(number) = lower.strip_suffix("mb") {
        (number, 1024usize * 1024)
    } else if let Some(number) = lower.strip_suffix('m') {
        (number, 1024usize * 1024)
    } else if let Some(number) = lower.strip_suffix("gib") {
        (number, 1024usize * 1024 * 1024)
    } else if let Some(number) = lower.strip_suffix("gb") {
        (number, 1024usize * 1024 * 1024)
    } else if let Some(number) = lower.strip_suffix('g') {
        (number, 1024usize * 1024 * 1024)
    } else {
        (lower.as_str(), 1usize)
    };

    let parsed = number
        .parse::<usize>()
        .map_err(|error| format!("invalid byte value {value:?}: {error}"))?;

    parsed
        .checked_mul(multiplier)
        .ok_or_else(|| format!("byte value {value:?} is too large"))
}

fn validate_agent_endpoint(endpoint: &str, allow_insecure: bool) -> Result<(), String> {
    let url = url::Url::parse(endpoint)
        .map_err(|_| "--agent-endpoint must include http:// or https://".to_string())?;
    match url.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = url
                .host_str()
                .ok_or_else(|| "--agent-endpoint must include a host".to_string())?;
            if is_loopback_host(host) || allow_insecure {
                Ok(())
            } else {
                Err("--agent-endpoint http:// is only allowed for loopback hosts unless --agent-allow-insecure-endpoint is set".to_string())
            }
        }
        _ => Err("--agent-endpoint must use http:// or https://".to_string()),
    }
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

fn split_flag_value(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((flag, value)) => (flag, Some(value)),
        None => (arg, None),
    }
}

fn reject_inline_value(flag: &str, value: Option<&str>) -> Result<(), String> {
    if value.is_some() {
        return Err(format!("{flag} does not accept a value"));
    }
    Ok(())
}

fn parse_positive_usize(flag: &str, value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|error| format!("invalid {flag} value: {error}"))?;
    if parsed == 0 {
        return Err(format!("{flag} must be positive"));
    }
    Ok(parsed)
}

fn parse_positive_u64(flag: &str, value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|error| format!("invalid {flag} value: {error}"))?;
    if parsed == 0 {
        return Err(format!("{flag} must be positive"));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn defaults_to_loopback_and_recent_version() {
        let config = Config::from_args(["mainstack-search"]).unwrap();
        assert_eq!(config.listen.to_string(), "127.0.0.1:9200");
        assert_eq!(config.advertised_version, "3.6.0");
    }

    #[test]
    fn rejects_non_loopback_without_opt_in() {
        let error = Config::from_args(["mainstack-search", "--listen", "0.0.0.0:9200"]).unwrap_err();
        assert!(error.contains("--allow-nonlocal-listen"));
    }

    #[test]
    fn rejects_agent_http_non_loopback_without_override() {
        let error = Config::from_args([
            "mainstack-search",
            "--agent-endpoint",
            "http://example.test/v1/chat/completions",
        ])
        .unwrap_err();
        assert!(error.contains("http:// is only allowed"));
    }

    #[test]
    fn accepts_loopback_agent_http() {
        let config = Config::from_args([
            "mainstack-search",
            "--agent-endpoint",
            "http://127.0.0.1:11434/v1/chat/completions",
        ])
        .unwrap();
        assert!(config.agent.enabled());
    }

    #[test]
    fn rejects_loopback_looking_dns_names_for_agent_http() {
        let error = Config::from_args([
            "mainstack-search",
            "--agent-endpoint",
            "http://127.0.0.1.example.test/v1/chat/completions",
        ])
        .unwrap_err();
        assert!(error.contains("http:// is only allowed"));
    }
}
