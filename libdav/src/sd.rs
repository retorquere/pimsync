// Copyright 2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Service discovery helpers.
//!
//! # See also
//!
//! - [`crate::CalDavClient::new_via_bootstrap`]
//! - [`crate::CardDavClient::new_via_bootstrap`]
//! - <https://www.rfc-editor.org/rfc/rfc6764>

use std::{io, string::FromUtf8Error};

use domain::{
    base::{
        name::LongChainError, wire::ParseError, Dname, Question, RelativeDname, Rtype,
        ToRelativeDname,
    },
    rdata::Txt,
    resolv::{lookup::srv::SrvError, StubResolver},
};
use http::{uri::Scheme, Uri};
use hyper::client::connect::Connect;

use crate::{common::check_support, dav::WebDavClient, CheckSupportError, InvalidUrl};

/// An error automatically bootstrapping a new client.
#[derive(thiserror::Error, Debug)]
pub enum BootstrapError {
    #[error("the input URL is not valid: {0}")]
    InvalidUrl(#[from] InvalidUrl),

    #[error("error resolving DNS SRV records: {0}")]
    DnsError(#[from] SrvError),

    #[error("SRV records returned domain/port pair that could not be parsed: {0}")]
    UnusableSrv(http::Error),

    #[error("error resolving context path via TXT records: {0}")]
    TxtError(#[from] TxtError),

    /// The service is decidedly not available.
    ///
    /// See <https://www.rfc-editor.org/rfc/rfc2782>, page 4
    #[error("the service is decidedly not available")]
    NotAvailable,
}

/// Find a CalDav or CardDav context path via client bootstrap sequence.
///
/// Determines the server's real host and the context path of the resources for a server,
/// following the discovery mechanism described in [rfc6764].
///
/// [rfc6764]: https://www.rfc-editor.org/rfc/rfc6764
///
/// This resolves from "user friendly" URLs to the real URL where the CalDav or CardDav server is
/// advertised as running. For example, a user may understand their CalDav server as being
/// `https://example.com` but bootstrapping would reveal it to actually run under
/// `https://instance31.example.com/users/john@example.com/calendars/`.
///
/// # Errors
///
/// If any of the underlying DNS or HTTP requests fail, or if any of the responses fail to
/// parse.
///
/// Does not return an error if DNS records are missing, only if they contain invalid data.
pub async fn find_context_url<C>(
    client: &WebDavClient<C>,
    service: DiscoverableService,
) -> Result<Option<Uri>, BootstrapError>
where
    C: Connect + Clone + Sync + Send,
{
    let domain = client.base_url.host().ok_or(InvalidUrl::MissingHost)?;
    let port = client.base_url.port_u16().unwrap_or(service.default_port());

    let dname = Dname::bytes_from_str(domain).map_err(InvalidUrl::InvalidDomain)?;
    let host_candidates = resolve_srv_record(service, &dname, port)
        .await?
        .ok_or(BootstrapError::NotAvailable)?;

    let mut context_path = None;
    if let Some(path) = find_context_path_via_txt_records(service, &dname).await? {
        for candidate in &host_candidates {
            let test_uri = Uri::builder()
                .scheme(service.scheme())
                .authority(format!("{}:{}", candidate.0, candidate.1))
                .path_and_query(&path)
                .build()
                .map_err(BootstrapError::UnusableSrv)?;

            match check_support(client, &test_uri, service.access_field()).await {
                Ok(()) | Err(CheckSupportError::NotAdvertised) => {
                    // NotAdvertised implies that the server does not advertise support for this
                    // protocol. We ignore this because NextCloud reports a lack of support for
                    // CalDav and CardDav. See https://github.com/nextcloud/server/issues/37374
                    context_path = Some(test_uri);
                    break;
                }
                Err(_) => continue,
            };
        }
    }
    if context_path.is_none() {
        for candidate in host_candidates {
            if let Ok(Some(url)) = client
                .find_context_path(service, &candidate.0, candidate.1)
                .await
            {
                context_path = Some(url);
                break;
            }
        }
    };

    Ok(context_path)
}

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

    /// Default port to use if no port is explicitly provided.
    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            DiscoverableService::CalDavs | DiscoverableService::CardDavs => 443,
            DiscoverableService::CalDav | DiscoverableService::CardDav => 80,
        }
    }

    /// Value that must be present in the `DAV:` header when checking for support.
    ///
    /// # See also
    ///
    /// - <https://www.rfc-editor.org/rfc/rfc4791#section-5.1>
    /// - <https://www.rfc-editor.org/rfc/rfc6352#section-6.1>
    #[must_use]
    pub fn access_field(self) -> &'static str {
        match self {
            DiscoverableService::CalDavs | DiscoverableService::CardDavs => "calendar-access",
            DiscoverableService::CalDav | DiscoverableService::CardDav => "addressbook",
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
    #[error("I/O error performing DNS request: {0}")]
    Network(#[from] io::Error),

    #[error("the domain name is too long and cannot be queried: {0}")]
    DomainTooLong(#[from] LongChainError),

    #[error("error parsing DNS response: {0}")]
    ParseError(#[from] ParseError),

    // FIXME: in theory, there's no reason why this should happen.
    #[error("txt record does not contain a valid utf-8 string: {0}")]
    NotUtf8Error(#[from] FromUtf8Error),

    #[error("data in txt record does no have the right syntax")]
    BadTxt,
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
