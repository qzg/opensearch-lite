use std::io;

use crate::{config::Config, security};

pub fn validate(config: &Config) -> io::Result<()> {
    security::tls::validate(&config.security)?;
    let _ = security::tls::rustls_config(&config.security)?;
    let _ = security::SecurityState::from_config(config)?;
    Ok(())
}
