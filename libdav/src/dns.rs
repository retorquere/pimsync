// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Helpers for DNS-based discovery.

use std::io;
use std::string::FromUtf8Error;

use domain::base::name::LongChainError;
use domain::base::wire::ParseError;
use domain::base::ToRelativeDname;
use domain::resolv::lookup::srv::SrvError;
use domain::{
    base::{Dname, Question, RelativeDname, Rtype},
    rdata::Txt,
    resolv::StubResolver,
};
use http::uri::Scheme;

/// Services for which automatic discovery is possible.
#[derive(Debug, Clone, Copy)]
pub enum DiscoverableService {
    /// Caldav over HTTPS.
    CalDavs,
    /// Caldav over plain-text HTTP.
    CalDav,
    /// Carddav over HTTPS.
    CardDavs,
    /// Carddav over plain-text HTTP.
    CardDav,
}

impl DiscoverableService {
    /// Relative domain suitable for querying this service type.
    #[must_use]
    #[allow(clippy::missing_panics_doc)]
    pub fn relative_domain(self) -> &'static RelativeDname<[u8]> {
        match self {
            DiscoverableService::CalDavs => RelativeDname::from_slice(b"\x08_caldavs\x04_tcp"),
            DiscoverableService::CalDav => RelativeDname::from_slice(b"\x07_caldav\x04_tcp"),
            DiscoverableService::CardDavs => RelativeDname::from_slice(b"\x09_carddavs\x04_tcp"),
            DiscoverableService::CardDav => RelativeDname::from_slice(b"\x08_carddav\x04_tcp"),
        }
        .expect("well known relative prefix is valid")
    }

    /// The scheme for this service type (e.g.: HTTP or HTTPS).
    #[must_use]
    pub fn scheme(self) -> Scheme {
        match self {
            DiscoverableService::CalDavs | DiscoverableService::CardDavs => Scheme::HTTPS,
            DiscoverableService::CalDav | DiscoverableService::CardDav => Scheme::HTTP,
        }
    }

    /// The well-known path for context-path discovery.
    #[must_use]
    pub fn well_known_path(self) -> &'static str {
        match self {
            DiscoverableService::CalDavs | DiscoverableService::CalDav => "/.well-known/caldav",
            DiscoverableService::CardDavs | DiscoverableService::CardDav => "/.well-known/carddav",
        }
    }

    /// Returns the default port to try and use.
    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            DiscoverableService::CalDavs | DiscoverableService::CardDavs => 443,
            DiscoverableService::CalDav | DiscoverableService::CardDav => 80,
        }
    }
}

/// Resolves SRV to locate the caldav server.
///
/// If the query is successful and the service is available, returns `Ok(Some(_))` with a `Vec` of
/// host/ports, in the order in which they should be tried.
///
/// If the query is successful but the service is decidedly not available, returns `Ok(None)`.
///
/// # Errors
///
/// If the underlying DNS request fails or the SRV record cannot be parsed.
///
/// # See also
///
/// - <https://www.rfc-editor.org/rfc/rfc2782>
/// - <https://www.rfc-editor.org/rfc/rfc6764>
pub async fn resolve_srv_record(
    service: DiscoverableService,
    domain: &Dname<impl AsRef<[u8]>>,
    port: u16,
) -> Result<Option<Vec<(String, u16)>>, SrvError> {
    Ok(StubResolver::new()
        .lookup_srv(service.relative_domain(), domain, port)
        .await?
        .map(|found| {
            found
                .into_srvs()
                .map(|entry| (entry.target().to_string(), entry.port()))
                .collect()
        }))
}

/// Error returned by [`find_context_path_via_txt_records`].
#[derive(thiserror::Error, Debug)]
pub enum TxtError {
    #[error("I/O error performing DNS request")]
    Network(#[from] io::Error),

    #[error("the domain name is too long and cannot be queried")]
    DomainTooLong(#[from] LongChainError),

    #[error("error parsing DNS response")]
    ParseError(#[from] ParseError),

    #[error("txt record does not contain a valid utf-8 string")]
    NotUtf8Error(#[from] FromUtf8Error),

    #[error("data in txt record does no have the right syntax")]
    BadTxt,
}

impl From<TxtError> for io::Error {
    fn from(value: TxtError) -> Self {
        match value {
            TxtError::Network(err) => err,
            TxtError::DomainTooLong(_) => io::Error::new(io::ErrorKind::InvalidInput, value),
            TxtError::ParseError(_) | TxtError::NotUtf8Error(_) | TxtError::BadTxt => {
                io::Error::new(io::ErrorKind::InvalidData, value)
            }
        }
    }
}

/// Resolves a context path via TXT records.
///
/// This returns a path where the default context path should be used for a given domain.
/// The domain provided should be in the format of `example.com` or `posteo.de`.
///
/// Returns an empty list of no relevant record was found.
///
/// # Errors
///
/// See [`TxtError`]
///
/// # See also
///
/// <https://www.rfc-editor.org/rfc/rfc6764>
pub async fn find_context_path_via_txt_records(
    service: DiscoverableService,
    domain: &Dname<impl AsRef<[u8]>>,
) -> Result<Option<String>, TxtError> {
    let resolver = StubResolver::new();
    let full_domain = service.relative_domain().chain(domain)?;
    let question = Question::new_in(full_domain, Rtype::Txt);

    let response = resolver.query(question).await?;
    let Some(record) = response.answer()?.next() else {
        return Ok(None);
    };
    let Some(parsed_record) = record?.into_record::<Txt<_>>()? else {
        return Ok(None);
    };

    let bytes = parsed_record.data().text::<Vec<u8>>();

    let path_result = String::from_utf8(bytes)?
        .strip_prefix("path=")
        .ok_or(TxtError::BadTxt)
        .map(String::from);
    Some(path_result).transpose()
}
