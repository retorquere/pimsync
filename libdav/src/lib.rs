#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]
// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! This library contains caldav and carddav clients.
//!
//! See [`CalDavClient`] and [`CardDavClient`] as a useful entry points.
//!
//! Both clients wrap a [`dav::WebDavClient`], and implement `Deref<Target = DavClient>`, so all
//! of `WebDavClient`'s associated functions for  are usable directly.
//!
//! # Service discovery
//!
//! DNS-based service discovery is implemented in [`sd::find_context_url`].
//!
//! The implementation does not validate DNSSEC signatures. Because of this, discovery must only be
//! used with a validating DNS resolver (as defined in [rfc4033][rfc4033]), or with domains served
//! from a local, trusted networks.
//!
//! [rfc4033]: https://www.rfc-editor.org/rfc/rfc4033
//!
//! # Hrefs
//!
//! All `href` strings returned by the server are unquoted by this library before being returned to
//! consumers. I.e.: you should assume that all `href`s have been url-decoded for you.
//!
//! All functions that take a parameter named `href` (or similar ones like `calendar_href`) expect
//! their input to NOT be URL-encoded. I.e.: you do not need to perform any quoting.

use crate::auth::Auth;
use dav::RequestError;
use dav::WebDavError;
use http::StatusCode;

pub mod auth;
mod caldav;
mod carddav;
mod common;
pub mod dav;
pub mod names;
pub mod sd;
pub mod xmlutils;

pub use caldav::service_for_url as caldav_service_for_url;
pub use caldav::CalDavClient;
pub use carddav::service_for_url as carddav_service_for_url;
pub use carddav::CardDavClient;

/// A WebDav property with a `namespace` and `name`.
///
/// This is currently an alias of [`roxmltree::ExpandedName`].
pub type PropertyName<'ns, 'name> = roxmltree::ExpandedName<'ns, 'name>;

/// A supplied Url was not valid.
#[derive(thiserror::Error, Debug)]
pub enum InvalidUrl {
    #[error("missing scheme")]
    MissingScheme,

    #[error("scheme is not valid for service type")]
    InvalidScheme,

    #[error("missing host")]
    MissingHost,

    #[error("the host is not a valid domain: {0}")]
    InvalidDomain(domain::base::name::FromStrError),
}

/// Error finding home set.
#[derive(thiserror::Error, Debug)]
#[error("error finding home set collection: {0}")]
pub struct FindHomeSetError(#[source] pub WebDavError);

/// See [`FetchedResource`]
#[derive(Debug, PartialEq, Eq)]
pub struct FetchedResourceContent {
    pub data: String,
    pub etag: String,
}

/// A parsed resource fetched from a server.
#[derive(Debug, PartialEq, Eq)]
pub struct FetchedResource {
    /// The absolute path to the resource in the server.
    pub href: String,
    /// The contents of the resource if available, or the status code if unavailable.
    pub content: Result<FetchedResourceContent, StatusCode>,
}

/// Returned when checking support for a feature encounters an error.
#[derive(thiserror::Error, Debug)]
pub enum CheckSupportError {
    #[error("the DAV header was missing from the response")]
    MissingHeader,

    #[error("the requested support is not advertised by the server")]
    NotAdvertised,

    #[error("the DAV header is not a valid string: {0}")]
    HeaderNotAscii(#[from] http::header::ToStrError),

    #[error("error performing http request: {0}: {0}")]
    Request(#[from] RequestError),

    #[error("invalid input URL: {0}")]
    InvalidInput(#[from] http::Error),

    #[error("http request returned {0}")]
    BadStatusCode(http::StatusCode),
}

impl From<StatusCode> for CheckSupportError {
    fn from(status: StatusCode) -> Self {
        CheckSupportError::BadStatusCode(status)
    }
}

/// Details of a single item that are returned when listing them.
///
/// This does not include actual item data, it only includes their metadata.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct ItemDetails {
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub resource_type: ResourceType,
}

#[derive(Default, Debug, PartialEq, Eq)]
// TODO: support unknown ones too...?
pub struct ResourceType {
    pub is_collection: bool,
    pub is_calendar: bool,
    pub is_address_book: bool,
}
