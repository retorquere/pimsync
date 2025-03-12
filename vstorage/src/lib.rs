// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2
#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]
#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

//! Interact with and synchronise with storages with different underlying implementations.
//!
//! Storages contain collections which can themselves contain `icalendar` components, `vcard`
//! entries, or other similar content types where items have an internal unique ids.
//!
//! This crates contains the underlying logic for [pimsync][pimsync]. pimsync is a command line
//! tool to synchronise storages with calendars and contacts. This crate implements the actual
//! logic for comparing and synchronising storages, and can be used to write alternative interfaces
//! for the same synchronisation implementation.
//!
//! [pimsync]: https://pimsync.whynothugo.nl/
//!
//! # Storage types
//!
//! A [`Storage`] contains a set of collections, where each collection can contain many items, but
//! not other collections. This restriction matches the semantics of CalDAV/CardDAV and also object
//! stores like S3.
//!
//! This crate currently includes the following implementations:
//!
//! - [`CalDavStorage`]: a CalDAV server, where each collection is an individual calendar, and
//!   each item is an individual event or todo in a calendar.
//! - [`CardDavStorage`]: a CardDAV server, where each collection is an individual address book, and
//!   each item is an individual contact card.
//! - [`ReadOnlyStorage`]: wraps around another `Storage` instance, returning an error of kind
//!   [`ErrorKind::ReadOnly`] for any write operation.
//! - [`VdirStorage`] a local directory, where each collection is a directory and each
//!   item is a file.
//! - [`WebCal`]: An icalendar file loaded via HTTP(s). This storage is implicitly read-only.
//!
//! The `Storage` type and the logic for synchronisation of storages is agnostic to the content
//! type inside collections, and can synchronise collections with any type of content. When
//! synchronising two storages, items with the same UID on both sides are synchronised with each
//! other. Interpreting content of items in order to extract these UIDs is done via the generic `I`
//! parameter, which implements the necessary operations for a specific content type of a given
//! storage instance.
//!
//! [`Storage`]: crate::base::Storage
//! [`CalDavStorage`]: crate::caldav::CalDavStorage
//! [`CardDavStorage`]: crate::carddav::CardDavStorage
//! [`ReadOnlyStorage`]: crate::readonly::ReadOnlyStorage
//! [`VdirStorage`]: crate::vdir::VdirStorage
//! [`WebCal`]: crate::webcal::WebCalStorage
//!
//! ## Collections, Hrefs and Collections Ids
//!
//! As mentioned above, collections cannot be nested (note for IMAP: having an `INBOX` collection
//! and an `INBOX/Feeds` collection is perfectly valid).
//!
//! A collection has an `href` and usually has an `id`.
//!
//! The `href` attribute is the path to an item inside a storage instance. Its value is storage
//! dependant, meaning that when a collection is synchronised to another storage, it may have a
//! different `href` on each side.
//!
//! The `id` for a collection is not storage-specific. When synchronising two storages, the default
//! approach is to synchronise items across collections with the same `id`. The `id` of a
//! collection is entirely dependant on its `href`, and should never change.
//!
//! The [`Href`] alias is used to refer to `href`s to avoid ambiguity. [`Href`] instances should be
//! treated as an opaque value and not given any special meaning outside of this crate.
//!
//! See also: [`CollectionId`].
//!
//! ## Items
//!
//! See [`Item`](crate::base::Item).
//!
//! ## Properties
//!
//! Storages expose properties for collections. Property types vary depending on a Storage's items,
//! although items themselves cannot have properties.
//!
//! Calendars have a `Colour`, `Description`, `DisplayName` and `Order`
//!
//! Address Books have `DisplayName` and `Description`.
//!
//! ## Entity tags
//!
//! An `Etag` is a value that changes whenever an item has changed in a collection. It is inspired
//! on the HTTP header with the same name (used extensively in WebDAV). See [`Etag`].

use std::{backtrace::Backtrace, str::FromStr, sync::Arc};

pub mod addressbook;
mod atomic;
pub mod base;
pub mod caldav;
pub mod calendar;
pub mod carddav;
mod dav;
pub mod disco;
pub mod readonly;
mod simple_component;
pub mod sync;
mod util;
pub mod vdir;
pub mod watch;
pub mod webcal;

type Result<T, E = crate::Error> = std::result::Result<T, E>;

/// Variants used to categorise [`Error`] instances.
#[derive(Debug, PartialEq)]
pub enum ErrorKind {
    DoesNotExist,
    NotACollection,
    NotAStorage,
    AccessDenied,
    Io,
    InvalidData,
    InvalidInput,
    ReadOnly,
    CollectionNotEmpty,
    PreconditionFailed,
    /// The requested operation is not possible on this specific instance.
    Unavailable,
    /// This storage implementation does not support a required feature.
    Unsupported,
    // #[deprecated]
    Uncategorised,
}

impl ErrorKind {
    /// Create a new error of this kind.
    ///
    /// This is merely a convenience shortcut to [`Error::new`].
    fn error<E>(self, source: E) -> Error
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Error::new(self, source)
    }

    #[must_use]
    const fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::DoesNotExist => "resource does not exist",
            ErrorKind::NotACollection => "resource exists, but is not a collection",
            ErrorKind::NotAStorage => "resource exists, but is not a storage",
            ErrorKind::AccessDenied => "access to the resource was denied",
            ErrorKind::Io => "input/output error",
            ErrorKind::InvalidData => "operation returned data, but it is not valid",
            ErrorKind::InvalidInput => "input data is invalid",
            ErrorKind::ReadOnly => "the resource is read-only",
            ErrorKind::CollectionNotEmpty => "the collection is not empty",
            ErrorKind::PreconditionFailed => "a required condition was not met",
            ErrorKind::Unavailable => "the operation is not possible on this instance",
            ErrorKind::Unsupported => "the operation is not supported",
            ErrorKind::Uncategorised => "uncategorised error",
        }
    }
}

/// Common error type used by all Storage implementations.
///
/// See also [`ErrorKind`].
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
    backtrace: Backtrace,
}

impl Error {
    fn new<E>(kind: ErrorKind, source: E) -> Error
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Error {
            kind,
            source: Some(source.into()),
            backtrace: Backtrace::capture(),
        }
    }

    pub fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Error {
            kind,
            source: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        let kind = match value.kind() {
            std::io::ErrorKind::NotFound => ErrorKind::DoesNotExist,
            std::io::ErrorKind::PermissionDenied => ErrorKind::AccessDenied,
            std::io::ErrorKind::InvalidInput => ErrorKind::InvalidInput,
            std::io::ErrorKind::InvalidData => ErrorKind::InvalidData,
            _ => ErrorKind::Io,
        };
        Error {
            kind,
            source: Some(value.into()),
            backtrace: Backtrace::capture(),
        }
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.write_str(self.as_str())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.source {
            Some(ref s) => write!(fmt, "{}: {}", self.kind, s),
            None => self.kind.fmt(fmt),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.source {
            Some(e) => Some(e.as_ref()),
            None => None,
        }
    }
}

/// Identifier for a specific version of a resource.
///
/// Each time that a resource is read, it will return its current `Etag`. The `Etag` is a unique
/// identifier for the current version. An `Etag` value is specific to a specific storage
/// implementation and instance. E.g.: they are opaque values that have no meaning across storages.
///
/// This is strongly inspired on the [HTTP header of the same name][MDN].
///
/// It is assumed that all `Etag` values are valid UTF-8 strings. As of HTTP 1.1, all header values
/// are restricted to visible characters in the ASCII range, so this is not a problem for CalDAV or
/// CardDAV storages. Other storages with no native `Etag` concept should attempt to use the most
/// suitable approximation.
///
/// [MDN]: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/ETag
#[derive(Debug, PartialEq, Clone)]
pub struct Etag(String);

impl Etag {
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<T> From<T> for Etag
where
    String: From<T>,
{
    fn from(value: T) -> Self {
        Etag(value.into())
    }
}

impl AsRef<str> for Etag {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Etag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Path to the item inside the collection.
///
/// For example, for CardDAV collections this is the path of the entry inside the collection. For
/// [`vdir::VdirStorage`], this the file's relative path, etc. `Href`s MUST be valid UTF-8 sequences.
/// Implementations MUST define their `Href` in a way that it is possible to infer:
///
/// - Whether an Href belongs to a collection or an item.
/// - For an item, to which collection it belongs.
///
/// Whether an `href` is relative to a collection or absolute is storage dependant. As such, this
/// should be treated as an opaque string by consumers of this library.
pub type Href = String;

/// Identifier for a collection.
///
/// Collection identifiers are a short string that uniquely identify a collection inside a storage.
/// They are based on the `href` of a collection, which never changes. A `CollectionId` is
/// intended as a more human-friendly substitute for collection `href`s, and an attribute which is
/// more storage-agnostic.
///
/// The following limitations exist, given that such values would produce ambiguous results with
/// the implementation of [`VdirStorage`], [`CalDavStorage`], and [`CardDavStorage`]:
///
/// - A `CollectionId` cannot contain a `/` (slash)
/// - A `CollectionId` cannot be exactly `..` (double period).
/// - A `CollectionId` cannot be exactly `.` (a single period).
///
/// [`VdirStorage`]: crate::vdir::VdirStorage
/// [`CalDavStorage`]: crate::caldav::CalDavStorage
/// [`CardDavStorage`]: crate::carddav::CardDavStorage
///
/// Instances of `CollectionId` always contain previously validated data. Instances are internally
/// reference counted and cheap to [`clone`][`Clone::clone`].
///
/// # Example
///
/// ```
/// # use vstorage::CollectionId;
/// let collection_id: CollectionId = "personal".parse().unwrap();
/// ```
#[derive(PartialEq, Debug, Clone, Eq, Hash)]
// INVARIANT: matches rules in documentation above.
pub struct CollectionId(Arc<str>);

impl AsRef<str> for CollectionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CollectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Error type when creating a new [`CollectionId`].
#[derive(Debug, thiserror::Error)]
pub enum CollectionIdError {
    #[error("collection id must not contain a slash")]
    Slash,
    #[error("collection id must not be '..'")]
    DoublePeriod,
    #[error("collection id must not be '.'")]
    SinglePeriod,
}

impl FromStr for CollectionId {
    type Err = CollectionIdError;

    /// Allocates a new [`CollectionId`] with the input data.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            s if s.chars().any(|c| c == '/') => Err(CollectionIdError::Slash),
            ".." => Err(CollectionIdError::DoublePeriod),
            "." => Err(CollectionIdError::SinglePeriod),
            s => Ok(CollectionId(Arc::from(s))),
        }
    }
}
