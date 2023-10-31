// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Helpers used for advanced TLS configuration.
use std::{fs::File, io::BufReader, num::ParseIntError, path::Path, sync::Arc};

use anyhow::{bail, Context};
use rustls::{
    client::{ServerCertVerified, ServerCertVerifier, WebPkiVerifier},
    Certificate, CertificateError, PrivateKey, RootCertStore,
};
use sha2::{Digest, Sha256};

/// Verifies that the fingerprint of a certificate matches.
pub(crate) struct FingerprintVerifier {
    fingerprint: Vec<u8>,
}

impl FingerprintVerifier {
    // Create a new verifier from a hexadecimal fingerprint representation.
    pub(crate) fn new(hex_fingerprint: &str) -> anyhow::Result<Self> {
        let fingerprint = (0..hex_fingerprint.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex_fingerprint[i..=i + 1], 16))
            .collect::<Result<Vec<u8>, ParseIntError>>()?;

        Ok(FingerprintVerifier { fingerprint })
    }
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &Certificate,
        _intermediates: &[Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        let fingerprint = Sha256::digest(&end_entity.0).to_vec();

        if self.fingerprint == fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                Arc::from(FingerprintError),
            )))
        }
    }
}

#[derive(Debug)]
struct FingerprintError;

impl std::fmt::Display for FingerprintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("certificate fingerprint does not match expectation")
    }
}

impl std::error::Error for FingerprintError {}

/// Verifies the fingerprint and CA for a certificate.
pub(crate) struct FingerprintAndWebPkiVerifier(FingerprintVerifier, WebPkiVerifier);

impl FingerprintAndWebPkiVerifier {
    pub(crate) fn new(
        hex_fingerprint: &str,
        roots: impl Into<Arc<RootCertStore>>,
    ) -> anyhow::Result<Self> {
        Ok(Self(
            FingerprintVerifier::new(hex_fingerprint)?,
            WebPkiVerifier::new(roots, None),
        ))
    }
}

impl ServerCertVerifier for FingerprintAndWebPkiVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &Certificate,
        intermediates: &[Certificate],
        server_name: &rustls::ServerName,
        scts: &mut dyn Iterator<Item = &[u8]>,
        ocsp_response: &[u8],
        now: std::time::SystemTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.0.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            scts,
            ocsp_response,
            now,
        )?;
        self.1.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            scts,
            ocsp_response,
            now,
        )
    }
}

/// Load certificates from a PEM-encoded file.
pub(crate) fn certs_from_pemfile(path: &Path) -> anyhow::Result<Vec<Certificate>> {
    let mut reader = BufReader::new(File::open(path)?);
    rustls_pemfile::certs(&mut reader)?
        .into_iter()
        .map(|v| Ok(Certificate(v)))
        .collect()
}

/// Load a keyfile from a PEM-encoded file.
pub(crate) fn key_from_pemfile(path: &Path) -> anyhow::Result<PrivateKey> {
    let mut reader = BufReader::new(File::open(path)?);

    loop {
        match rustls_pemfile::read_one(&mut reader)? {
            Some(
                rustls_pemfile::Item::RSAKey(key)
                | rustls_pemfile::Item::PKCS8Key(key)
                | rustls_pemfile::Item::ECKey(key),
            ) => return Ok(PrivateKey(key)),
            None => break,
            _ => {}
        }
    }

    bail!("no keys found in {}", path.to_string_lossy());
}

/// Load certificates and a key file from a pem-encoded file.
pub(crate) fn cert_and_key_from_pemfile(
    path: &Path,
) -> anyhow::Result<(Vec<Certificate>, PrivateKey)> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut certs = Vec::new();
    let mut raw_key = None;

    loop {
        match rustls_pemfile::read_one(&mut reader)? {
            Some(
                rustls_pemfile::Item::RSAKey(k)
                | rustls_pemfile::Item::PKCS8Key(k)
                | rustls_pemfile::Item::ECKey(k),
            ) => {
                if raw_key.replace(k).is_some() {
                    bail!("multiple keys found in {}", path.to_string_lossy());
                }
            }
            None => break,
            Some(rustls_pemfile::Item::X509Certificate(cert)) => certs.push(Certificate(cert)),
            _ => {}
        }
    }

    let key = raw_key
        .map(|k| PrivateKey(k))
        .with_context(|| format!("no key found in {}", path.to_string_lossy()))?;
    Ok((certs, key))
}
