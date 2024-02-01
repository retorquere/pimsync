// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Helpers used for advanced TLS configuration.
use std::{fs::File, io::BufReader, num::ParseIntError, path::Path, sync::Arc};

use anyhow::{bail, Context};
use rustls::{
    client::{
        danger::{ServerCertVerified, ServerCertVerifier},
        WebPkiServerVerifier,
    },
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
    CertificateError, OtherError, RootCertStore,
};
use sha2::{Digest, Sha256};

/// Verifies that the fingerprint of a certificate matches.
#[derive(Debug)]
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
        end_entity: &CertificateDer,
        _intermediates: &[CertificateDer],
        _server_name: &ServerName,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fingerprint = Sha256::digest(end_entity).to_vec();

        if self.fingerprint == fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::Other(
                OtherError(Arc::from(FingerprintError)),
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
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
#[derive(Debug)]
pub(crate) struct FingerprintAndWebPkiVerifier(FingerprintVerifier, Arc<WebPkiServerVerifier>);

impl FingerprintAndWebPkiVerifier {
    pub(crate) fn new(
        hex_fingerprint: &str,
        roots: impl Into<Arc<RootCertStore>>,
    ) -> anyhow::Result<Self> {
        Ok(Self(
            FingerprintVerifier::new(hex_fingerprint)?,
            WebPkiServerVerifier::builder(roots.into()).build()?,
        ))
    }
}

impl ServerCertVerifier for FingerprintAndWebPkiVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer,
        intermediates: &[CertificateDer],
        server_name: &ServerName,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.0
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)?;
        self.1
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Load certificates from a PEM-encoded file.
pub(crate) fn certs_from_pemfile(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let mut reader = BufReader::new(File::open(path)?);
    Ok(rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?)
}

/// Load a keyfile from a PEM-encoded file.
pub(crate) fn key_from_pemfile(path: &Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    let mut reader = BufReader::new(File::open(path)?);

    match rustls_pemfile::private_key(&mut reader)? {
        Some(private_key) => Ok(private_key),
        None => bail!("no key file found in {}", path.to_string_lossy()),
    }
}

/// Load certificates and a key file from a pem-encoded file.
pub(crate) fn cert_and_key_from_pemfile(
    path: &Path,
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut certs = Vec::new();
    let mut raw_key = None;

    loop {
        match rustls_pemfile::read_one(&mut reader)? {
            Some(rustls_pemfile::Item::Pkcs1Key(k)) => {
                if raw_key.replace(PrivateKeyDer::from(k)).is_some() {
                    bail!("multiple keys found in {}", path.to_string_lossy());
                }
            }
            Some(rustls_pemfile::Item::Pkcs8Key(k)) => {
                if raw_key.replace(PrivateKeyDer::from(k)).is_some() {
                    bail!("multiple keys found in {}", path.to_string_lossy());
                }
            }
            Some(rustls_pemfile::Item::Sec1Key(k)) => {
                if raw_key.replace(PrivateKeyDer::from(k)).is_some() {
                    bail!("multiple keys found in {}", path.to_string_lossy());
                }
            }
            Some(rustls_pemfile::Item::X509Certificate(cert)) => {
                certs.push(cert);
            }
            None => break,
            _ => {}
        }
    }

    let key = raw_key.with_context(|| format!("no key found in {}", path.to_string_lossy()))?;
    Ok((certs, key))
}
