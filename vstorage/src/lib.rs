// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2
#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]

//! This crate implements a common API for reading and writing items on different underlying
//! storage implementations. Storage implementations can contain `icalendar` components, `vcard`
//! entries, or content types where items are either immutable or have unique ids.
//!
//! # Storage
//!
//! A [`Storage`] contains a set of collections, where each collection can contain many items, but
//! not other collections. This restriction matches the semantics of caldav/carddav and also object
//! stores like S3. Some examples are:
//!
//! - A [`CalDavStorage`] is a caldav server, where each collection is an individual calendar, and
//! each item is an individual event or todo in a calendar.
//! - A [`FilesystemStorage`] is a local directory, where each collection is a directory and each
//! item is a file.
//! - A potential `ImapStorage` instance is a single IMAP account, where each collection is a
//! mailbox and each item is an individual email message.
//!
//! This crate is agnostic to the content type inside collections, and can synchronise collections
//! with any type of content. However, some basic understanding of these is necessary; calendar
//! components have a UID, and when synchronising two storages, components with the same UID on
//! each side are to be treated as the same.
//!
//! Interpreting content to extract these UIDs is done via the generic `I` parameter, which
//! implements the necessary operations for a specific content type of a given storage instance.
//!
//! [`Storage`]: crate::base::Storage
//! [`CalDavStorage`]: crate::caldav::CalDavStorage
//! [`FilesystemStorage`]: crate::filesystem::FilesystemStorage
//!
//! ## Collections, Hrefs and Collections Ids
//!
//! As mentioned above, collections cannot be nested (although having an `INBOX` collection and an
//! `INBOX/Feeds` collection is perfectly valid).
//!
//! A collection has an `href` and an `id`. The `href` attribute is storage dependant, meaning that
//! when a collection is synchronised to another storage, it may have a different `href`. The `id`
//! for a collection is not storage-specific. When synchronising two storages, the default approach
//! is to synchronise items across collections with the same `id`. The `id` of a collection is
//! entirely dependant on its `href`, and should never change.
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
//! ## Entity tags
//!
//! An `Etag` is a value that changes whenever an item has changed in a collection. It is inspired
//! on the HTTP header with the same name (used extensively in WebDav). See [`Etag`].

use std::str::FromStr;

pub mod base;
mod boxed;
pub mod caldav;
pub mod carddav;
mod dav;
pub mod disco;
pub mod filesystem;
pub mod readonly;
mod simple_component;
pub mod sync;
mod util;
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
            ErrorKind::Unsupported => "the operation is not supported",
            ErrorKind::Uncategorised => "uncategorised error",
        }
    }
    // TODO: generate rustdoc for each variant using this method?
}

/// A common error type used by all Storage implementations.
///
/// See also [`ErrorKind`].
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    fn new<E>(kind: ErrorKind, source: E) -> Error
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Error {
            kind,
            source: Some(source.into()),
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Error { kind, source: None }
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

impl std::error::Error for Error {}

/// An identifier for a specific version of a resource.
///
/// Each time that a resource is read, it will return its current `Etag`. The `Etag` is a unique
/// identifier for the current version. An `Etag` value is specific to a specific storage
/// implementation and instance. E.g.: they are opaque values that have no meaning across storages.
///
/// This is strongly inspired on the [HTTP header of the same name][MDN].
///
/// It is assumed that all `Etag` values are valid UTF-8 strings. As of HTTP 1.1, all header values
/// are restricted to visible characters in the ASCII range, so this is not a problem for CalDav or
/// CardDav storages. Other storages with no native `Etag` concept should attempt to use the most
/// suitable approximation.
///
/// [MDN]: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/ETag
#[derive(Debug, PartialEq, Clone)]
pub struct Etag(String);

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

/// The path to the item inside the collection.
///
/// For example, for carddav collections this is the path of the entry inside the collection. For
/// Filesystem, this the file's relative path, etc. `Href`s MUST be valid UTF-8 sequences.
/// Implementations MUST define their `Href` in a way that it is possible to infer:
///
/// - Whether an Href belongs to a collection or an item.
/// - For an item, to which collection it belongs.
///
/// Whether an `href` is relative to a collection or absolute is storage dependant. As such, this
/// should be treated as an opaque string by consumers of this library.
pub type Href = String;

/// An identifier for a collection.
///
/// Collection identifiers are a short string that uniquely identify a collection inside a storage.
/// They are based on the `href` of a collection, which never changes. Likewise, the `CollectionId`
/// for a `CollectionId` never changes. The `CollectionId` is intended as a more human-friendly
/// substitute for collection `href`s.
///
/// The following limitations exist, given that such values would produce ambiguous results with
/// the implementation of [`FilesystemStorage`], [`CalDavStorage`], and [`CardDavStorage`]:
///
/// - A `CollectionId` cannot contain a `/` (slash)
/// - A `CollectionId` cannot be exactly `..` (double period).
/// - A `CollectionId` cannot be exactly `.` (a single period).
///
/// [`FilesystemStorage`]: crate::filesystem::FilesystemStorage
/// [`CalDavStorage`]: crate::caldav::CalDavStorage
/// [`CardDavStorage`]: crate::carddav::CardDavStorage
///
/// # Creating instances
///
/// Instances of `CollectionId` always contain previously validated data.
///
/// See: [`CollectionId::try_from`] and [`CollectionId::from_str`].
#[derive(PartialEq, Debug, Clone, Eq, Hash)]
pub struct CollectionId {
    // INVARIANT: matches rules in documentation above.
    inner: String,
}

impl CollectionId {
    #[inline]
    fn validate(value: &str) -> std::result::Result<(), CollectionIdError> {
        if value.chars().any(|c| c == '/') {
            return Err(CollectionIdError::Slash);
        }
        if value == ".." {
            return Err(CollectionIdError::DoublePeriod);
        }
        if value == "." {
            return Err(CollectionIdError::SinglePeriod);
        }

        Ok(())
    }
}

impl AsRef<str> for CollectionId {
    fn as_ref(&self) -> &str {
        self.inner.as_ref()
    }
}

impl From<CollectionId> for String {
    fn from(value: CollectionId) -> String {
        value.inner
    }
}

impl std::fmt::Display for CollectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

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

    /// Creates a new `CollectionId` with the input data.
    ///
    /// When converting a `String`, use [`CollectionId::try_from`] instead to avoid re-allocating
    /// the string data.
    ///
    /// # Example
    ///
    /// ```
    /// # use vstorage::CollectionId;
    /// let collection_id: CollectionId = "personal".parse().unwrap();
    /// ```
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // TODO: validation should not iterate string thrice.
        Self::validate(s)?;

        Ok(CollectionId {
            inner: s.to_string(),
        })
    }
}

impl TryFrom<String> for CollectionId {
    type Error = CollectionIdError;

    /// Converts a `String` instance into a `CollectionId`.
    ///
    /// Note that manually allocating a `String` before calling this method is an anti-pattern; the
    /// cost of the re-allocation is paid even if the validation fails. For converting [`&str`],
    /// see [`CollectionId::from_str`].
    fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
        Self::validate(&value)?;
        Ok(CollectionId { inner: value })
    }
}
