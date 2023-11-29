#![deny(clippy::pedantic)]
// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! This library contains caldav and carddav clients.
//!
//! See [`CalDavClient`] and [`CardDavClient`] as a useful entry points.
//!
//! Both clients implement `Deref<Target = DavClient>`, so all the associated
//! functions for [`dav::WebDavClient`] are usable directly.
//!
//! # Hrefs
//!
//! All `href` strings returned by the server are returned unquoted by this library before being
//! returned to consumers.
//!
//! All functions that take a parameter named `href` (or similar ones like `calendar_href`) expect
//! their input to NOT be URL-encoded.

use crate::auth::Auth;
use dav::DavError;
use dav::FindCurrentUserPrincipalError;
use dav::RequestError;
use dns::{SrvError, TxtError};
use http::StatusCode;

pub mod auth;
pub mod builder;
mod caldav;
mod carddav;
mod common;
pub mod dav;
pub mod dns;
pub mod names;
pub mod xmlutils;

pub use caldav::CalDavClient;
pub use carddav::CardDavClient;

/// A WebDav property with a `namespace` and `name`.
///
/// This is currently an alias of [`roxmltree::ExpandedName`].
pub type Property<'ns, 'name> = roxmltree::ExpandedName<'ns, 'name>;

/// An error automatically bootstrapping a new client.
#[derive(thiserror::Error, Debug)]
pub enum BootstrapError {
    #[error("the input URL is not valid")]
    InvalidUrl(&'static str),

    #[error("error resolving DNS SRV records")]
    DnsError(SrvError),

    #[error("SRV records returned domain/port pair that failed to parse")]
    UnusableSrv(http::Error),

    #[error("error resolving context path via TXT records")]
    TxtError(#[from] TxtError),

    #[error(transparent)]
    HomeSet(#[from] FindHomeSetError),

    #[error("error querying current user principal")]
    CurrentPrincipal(#[from] FindCurrentUserPrincipalError),

    #[error(transparent)]
    DavError(#[from] DavError),

    /// The service is decidedly not available.
    ///
    /// See <https://www.rfc-editor.org/rfc/rfc2782>, page 4
    #[error("the service is decidedly not available")]
    NotAvailable,
}

/// Error finding home set.
#[derive(thiserror::Error, Debug)]
#[error("error finding home set collection")]
pub struct FindHomeSetError(#[source] pub DavError);

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

    #[error("the DAV header is not a valid string")]
    HeaderNotAscii(#[from] http::header::ToStrError),

    #[error("error performing http request")]
    Request(#[from] RequestError),

    #[error("invalid input URL")]
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
    /// From: <https://www.rfc-editor.org/rfc/rfc6578>
    // TODO: move this field into `FoundCollection`; it is meaningless for non-collections.
    pub supports_sync: bool,
}

#[derive(Default, Debug, PartialEq, Eq)]
// TODO: support unknown ones too...?
pub struct ResourceType {
    pub is_collection: bool,
    pub is_calendar: bool,
    pub is_address_book: bool,
}
