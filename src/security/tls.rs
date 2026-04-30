use std::{
    fs::File,
    io::{self, BufReader},
    sync::Arc,
};

use axum_server::tls_rustls::RustlsConfig;
use rustls::{server::WebPkiClientVerifier, RootCertStore, ServerConfig};
use rustls_pemfile::{certs, private_key};

use crate::security::config::SecurityConfig;

pub fn validate(config: &SecurityConfig) -> io::Result<()> {
    if let Some(tls) = &config.tls {
        let _ = load_certs(&tls.cert_file)?;
        let _ = load_key(&tls.key_file)?;
        if let Some(ca_file) = &tls.server_ca_file {
            let _ = load_certs(ca_file)?;
        }
    }
    if let Some(ca_file) = &config.client_cert_ca_file {
        let _ = load_certs(ca_file)?;
    }
    Ok(())
}

pub fn rustls_config(config: &SecurityConfig) -> io::Result<Option<RustlsConfig>> {
    let Some(tls) = &config.tls else {
        return Ok(None);
    };
    ensure_crypto_provider();

    let cert_chain = load_certs(&tls.cert_file)?;
    let key = load_key(&tls.key_file)?;

    let mut server_config = if config.require_client_cert {
        let ca_file = config.client_cert_ca_file.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--require-client-cert requires --client-cert-ca-file",
            )
        })?;
        let mut roots = RootCertStore::empty();
        for cert in load_certs(ca_file)? {
            roots.add(cert).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("failed to add client certificate CA: {error}"),
                )
            })?;
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid client certificate CA: {error}"),
                )
            })?;
        ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(cert_chain, key)
    } else {
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
    }
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, format!("{error}")))?;

    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Some(RustlsConfig::from_config(Arc::new(server_config))))
}

fn ensure_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

fn load_certs(
    path: &std::path::Path,
) -> io::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let file = File::open(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to read certificate file {}: {error}",
                path.display()
            ),
        )
    })?;
    let mut reader = BufReader::new(file);
    let certs = certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "failed to parse certificate file {}: {error}",
                    path.display()
                ),
            )
        })?;
    if certs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "certificate file {} contained no certificates",
                path.display()
            ),
        ));
    }
    Ok(certs)
}

fn load_key(path: &std::path::Path) -> io::Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let file = File::open(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to read private key file {}: {error}",
                path.display()
            ),
        )
    })?;
    let mut reader = BufReader::new(file);
    private_key(&mut reader)
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "failed to parse private key file {}: {error}",
                    path.display()
                ),
            )
        })?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "private key file {} contained no private key",
                    path.display()
                ),
            )
        })
}
