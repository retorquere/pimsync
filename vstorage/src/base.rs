// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Traits and common implementations shared by different storages.
//!
//! When writing code that should deal with different storage implementations, these traits should
//! be used as input / outputs, rather than concrete per-store types.
//!
//! See [`Storage`] as an entry point to this module.

use std::num::NonZeroUsize;

use async_trait::async_trait;
use tokio::sync::mpsc::Receiver;

use crate::{
    disco::Discovery, util::replace_uid, watch::Event, CollectionId, Error, ErrorKind, Etag, Href,
    Result,
};

/// A storage is the highest level abstraction where items can be stored. It can be a remote CalDav
/// account, a local filesystem, etc.
///
/// Each storage may contain one or more **collections** (e.g.: calendars or address books).
///
/// The specific type of item that a storage can hold is defined by the `I` generic parameter.
/// E.g.: a CalDav storage can hold icalendar items. Only items with the same kind of item can be
/// synchronised with each other (e.g.: it it nos possible to synchronise `Storage<VcardItem>` with
/// `Storage<IcsItem>`
///
/// # Note for implementors
///
/// The auto-generated documentation for this trait is rather hard to read due to the usage of
/// [`#[async_trait]`](mod@async_trait) macro. You might want to consider clicking on the
/// `source` link and reading the documentation from the raw code for this trait.
#[async_trait]
pub trait Storage<I: Item>: Sync + Send {
    // TODO: Some calendar instances only allow a single item type (e.g.: events but not todos).

    /// Checks that the storage works. This includes validating credentials, and reachability.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage is not reachable and usable.
    async fn check(&self) -> Result<()>;

    /// Finds existing collections for this storage.
    async fn discover_collections(&self) -> Result<Discovery>;

    /// Creates a new collection with a specified `href`.
    async fn create_collection(&self, href: &str) -> Result<Collection>;

    /// Deletes an existing collection.
    ///
    /// A collection must be empty for deletion to succeed.
    async fn destroy_collection(&self, href: &str) -> Result<()>;

    /// Returns the value of a property for a given collection.
    async fn get_collection_property(
        &self,
        collection: &str,
        property: I::CollectionProperty,
    ) -> Result<Option<String>>;

    /// Sets the value of a property for a given collection.
    async fn set_collection_property(
        &self,
        collection: &str,
        property: I::CollectionProperty,
        value: &str,
    ) -> Result<()>;

    /// Enumerates items in a given collection.
    async fn list_items(&self, collection_href: &str) -> Result<Vec<ItemRef>>;

    /// Fetches a single item from given collection.
    ///
    /// Storages never cache data locally. For reading items in bulk, prefer
    /// [`Storage::get_many_items`].
    async fn get_item(&self, href: &str) -> Result<(I, Etag)>;

    /// Fetches multiple items.
    ///
    /// Similar to [`Storage::get_item`], but optimised to minimise the amount of IO required.
    /// Duplicate `href`s are ignored.
    ///
    /// All requested items MUST belong to the same collection.
    ///
    /// # Note for implementers
    ///
    /// The default implementation is usually not optimal, and implementations of this trait should
    /// override it.
    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem<I>>> {
        let mut items = Vec::with_capacity(hrefs.len());
        for href in hrefs {
            let item = self.get_item(href).await?;
            items.push(FetchedItem {
                href: (*href).to_owned(),
                item: item.0,
                etag: item.1,
            });
        }
        Ok(items)
    }

    /// Fetch all items from a given collection.
    ///
    /// # Note for implementors
    ///
    /// The default implementation is usually not optimal, and implementations of this trait should
    /// override it.
    async fn get_all_items(&self, collection: &str) -> Result<Vec<FetchedItem<I>>> {
        let item_refs = self.list_items(collection).await?;
        let mut items = Vec::with_capacity(item_refs.len());
        for item_ref in item_refs {
            let item = self.get_item(&item_ref.href).await?;
            items.push(FetchedItem {
                href: item_ref.href,
                item: item.0,
                etag: item.1,
            });
        }
        Ok(items)
    }

    /// Saves a new item into a given collection
    async fn add_item(&self, collection: &str, item: &I) -> Result<ItemRef>;

    /// Updates the contents of an existing item.
    async fn update_item(&self, href: &str, etag: &Etag, item: &I) -> Result<Option<Etag>>;

    /// Deletes an existing item.
    async fn delete_item(&self, href: &str, etag: &Etag) -> Result<()>;

    /// Return the id for a collection with the given `href`.
    ///
    /// The id for a given Collection must never change. Usually this is based off the last
    /// component of the href, but may be different for storages where this does not make sense.
    ///
    /// # Errors
    ///
    /// This functions returns an `Err` variant if the provided `collection` is invalid.
    fn collection_id(&self, collection_href: &str) -> Result<CollectionId>;

    /// Return the `href` for a collection that would have the given `id`.
    ///
    /// Creating a collection under `href` SHOULD result in the collection being available via
    /// discovery with the provided `id`.
    ///
    /// # Errors
    ///
    /// Returns an error if no collection can exist such that it is available via discovery AND its
    /// `CollectionId` matches the input.
    fn href_for_collection_id(&self, id: &CollectionId) -> Result<Href>;

    /// Monitor the storage for changes.
    ///
    /// Returns the [`Receiver`] of a channel which receives [`Event`] instances when changes are
    /// detected.
    ///
    /// # Errors
    ///
    /// The default implementation returns [`ErrorKind::Unsupported`].
    async fn monitor(&self, bufsize: NonZeroUsize) -> Result<Receiver<Event>> {
        let _ = bufsize;
        return Err(Error::new(
            ErrorKind::Unsupported,
            "Storage implementation does not currently support monitoring.",
        ));
    }
}

/// A collection may, for example, be an address book or a calendar.
///
/// The type of items contained is restricted by the underlying implementation. Collections contain
/// zero or more items (e.g.: an address book contains events). Each item is addressed by its own
/// [`Href`].
///
/// This type wraps around the `href` for a collection on a given storage. Using the same
/// `Collection` instance across different storages is disallowed.
#[derive(Debug)]
pub struct Collection {
    href: Href,
}

impl Collection {
    /// The path to this collection inside the storage.
    ///
    /// Href should not change over time, so should be associated with an immutable property of the
    /// collection (e.g.: a relative URL path, or a directory's filename).
    ///
    /// The exact meaning of this value is storage-specific, but should be remain consistent with a
    /// storage.
    #[must_use]
    pub fn href(&self) -> &Href {
        &self.href
    }

    #[must_use]
    pub fn into_href(self) -> String {
        self.href
    }

    pub(crate) fn new(href: String) -> Collection {
        Collection { href }
    }
}

/// A reference to an [`Item`] inside a collection.
#[derive(PartialEq, Debug, Clone)]
pub struct ItemRef {
    pub href: Href,
    pub etag: Etag,
}

/// A type of item that is contained in a [`Storage`].
///
/// A `Storage` can contain items of a concrete type described by implementations of this trait.
/// This trait defines how to extract the basic information that is required to synchronise
/// storages. Additional parsing is out of scope here and should be done by inspecting the raw data
/// inside an item via [`Item::as_str`].
pub trait Item: Sync + Send + std::fmt::Debug
where
    Self: From<String>,
{
    /// Property types supported by storages.
    ///
    /// Generally, this type should be an `enum` with each known property represented as a
    /// different variant.
    ///
    /// These were known as "metadata" in the previous vdirsyncer implementation.
    ///
    /// See also [`Storage::get_collection_property`] and [`Storage::get_collection_property`].
    type CollectionProperty: Sync + Send;

    /// Parse the item and return a unique identifier for it.
    ///
    /// The `uid` does not change when the item is modified. The `uid` remains the same when the
    /// item is copied across storages and storage types.
    #[must_use]
    fn uid(&self) -> Option<String>;

    /// Return the hash of this item, usually normalised.
    ///
    /// Implementations SHOULD normalise content before hashing to ensure that two semantically
    /// equivalent items return the same hash.
    ///
    /// This value is used as a fallback when a storage backend doesn't provide [`Etag`] values, or
    /// when an item's [`Item::uid`] returns `None`.
    #[must_use]
    fn hash(&self) -> String;

    /// A unique identifier for this item. Is either the UID (if any), or the hash of its contents.
    #[must_use]
    fn ident(&self) -> String;

    /// Returns a new copy of this Item with the supplied UID.
    #[must_use]
    fn with_uid(&self, new_uid: &str) -> Self;

    #[must_use]
    /// Returns the raw contents of this item.
    fn as_str(&self) -> &str;
}

/// Immutable wrapper around a `VCALENDAR` or `VCARD`.
///
/// Note that this is not a proper validating parser for icalendar or vcard; it's a very simple
/// one with the sole purpose of extracing a UID. Proper parsing of components is out of scope,
/// since supporting potentially invalid items is required.
#[derive(Debug)]
pub struct IcsItem {
    raw: String,
}

/// Properties supported for calendars.
///
/// This is strongly based on the properties supported by `CalDav`.
#[non_exhaustive]
pub enum CalendarProperty {
    /// A colour to be used when displaying this collection.
    ///
    /// Graphical interfaces may use this for the collection itself or its items.
    Colour,
    /// A user-friendly name for a collection.
    ///
    /// It is recommended to show this name in user interfaces.
    DisplayName,
    Description,
    Order,
}

impl Item for IcsItem {
    /// Calendar properties defined by `CalDav`.
    type CollectionProperty = CalendarProperty;

    /// Returns the contents of the `UID` property, if defined.
    #[must_use]
    fn uid(&self) -> Option<String> {
        uid(&self.raw)
    }

    /// Returns the hash of the normalised content.
    #[must_use]
    fn hash(&self) -> String {
        crate::util::hash(&self.raw)
    }

    /// A unique identifier for this item. Is either the UID (if any), or the hash of its contents.
    #[must_use]
    fn ident(&self) -> String {
        self.uid().unwrap_or_else(|| self.hash())
    }

    /// Returns a new copy of this Item with the supplied UID.
    #[must_use]
    fn with_uid(&self, new_uid: &str) -> Self {
        IcsItem::from(replace_uid(&self.raw, new_uid))
    }

    #[inline]
    #[must_use]
    /// Returns the raw contents of this item.
    fn as_str(&self) -> &str {
        &self.raw
    }
}

impl From<String> for IcsItem {
    /// Create a new `IcsItem`, normalising newlines into `\r\n`.
    fn from(value: String) -> Self {
        let mut lines = value
            .split_terminator('\n')
            .map(|line| line.trim_end_matches('\r'));
        let mut raw = lines
            .next()
            .expect("split line yields at least one item")
            .to_string();
        for line in lines {
            raw.push_str("\r\n");
            raw.push_str(line);
        }

        IcsItem { raw }
    }
}

fn uid(raw: &str) -> Option<String> {
    let mut lines = raw.split_terminator("\r\n");
    let mut uid = lines
        .find_map(|line| line.strip_prefix("UID:"))
        .map(String::from)?;

    // If the following lines start with a space or tab, they're a continuation of the UID.
    // See: https://www.rfc-editor.org/rfc/rfc5545#section-3.1
    lines
        .map_while(|line| line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')))
        .for_each(|part| uid.push_str(part));

    Some(uid)
}

#[cfg(test)]
mod tests {
    // Note: Some of these examples are NOT valid vcards.
    // vdirsyncer is expected to handle invalid input gracefully and sync it as-is,
    // so this is not really a problem.

    use crate::base::{IcsItem, Item, Storage};

    #[test]
    fn test_single_line_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));

        let raw = ["BEGIN:VCARD", "UID:hel", "lo", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hel")));
        assert_eq!(item.ident(), String::from("hel"));

        let raw = [
            "BEGIN:VCARD",
            "UID:hello",
            "REV:20210307T195614Z\tthere",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));
    }

    #[test]
    fn test_missing_carrige_return() {
        // Same as above, but missing \r.
        let raw = ["BEGIN:VCARD", "UID:hello", "END:VCARD"].join("\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));

        let raw = ["BEGIN:VCARD", "UID:hel", "lo", "END:VCARD"].join("\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hel")));
        assert_eq!(item.ident(), String::from("hel"));

        let raw = [
            "BEGIN:VCARD",
            "UID:hello",
            "REV:20210307T195614Z\tthere",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));
    }

    #[test]
    fn test_multi_line_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "\tthere", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hellothere")));
        assert_eq!(item.ident(), String::from("hellothere"));

        let raw = [
            "BEGIN:VCARD",
            "UID:hello",
            "\tthere",
            "REV:20210307T195614Z",
            "\tnope",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hellothere")));
        assert_eq!(item.ident(), String::from("hellothere"));
    }

    #[test]
    fn test_missing_uid() {
        let raw = [
            "BEGIN:VCARD",
            "UIDX:hello",
            "REV:20210307T195614Z\tthere",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), None);
        assert_eq!(item.ident(), item.hash());
    }

    #[test]
    fn test_storage_is_object_safe() {
        #[allow(dead_code)]
        fn dummy(_: Box<dyn Storage<IcsItem>>) {}
    }

    #[test]
    fn test_with_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        let item2 = item.with_uid("goodbye");
        assert_eq!(item2.uid(), Some(String::from("goodbye")));
        assert_eq!(item2.ident(), String::from("goodbye"));
    }

    #[test]
    fn test_with_uid_without_uid() {
        let raw = ["BEGIN:VCARD", "SUMMARY:hello", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        let item2 = item.with_uid("goodbye");
        assert_eq!(item2.uid(), None);
    }
}

/// Immutable wrapper around a `VCARD`.
///
/// Note that this is not a proper validating parser for vcard; it's a very simple one with the
/// sole purpose of extracing a UID. Proper parsing of components is out of scope, since we want to
/// enable operating on potentially invalid items too.
#[derive(Debug)]
pub struct VcardItem {
    // TODO: make this Vec<u8> instead?
    raw: String,
}

/// Properties supported for address books.
///
/// This is strongly based on the properties supported by `CardDav`.
#[non_exhaustive]
pub enum AddressBookProperty {
    DisplayName,
    Description,
    // TODO: can this have colour too?
}

impl Item for VcardItem {
    type CollectionProperty = AddressBookProperty;
    /// Returns a unique identifier for this item.
    ///
    /// The UID does not change when the item is modified. The UID must remain the same when the
    /// item is copied across storages and storage types.
    #[must_use]
    fn uid(&self) -> Option<String> {
        uid(&self.raw)
    }

    /// Returns the hash of the normalised content.
    #[must_use]
    fn hash(&self) -> String {
        crate::util::hash(&self.raw)
    }

    /// A unique identifier for this item. Is either the UID (if any), or the hash of its contents.
    #[must_use]
    fn ident(&self) -> String {
        self.uid().unwrap_or_else(|| self.hash())
    }

    /// Returns a new copy of this Item with the supplied UID.
    #[must_use]
    fn with_uid(&self, new_uid: &str) -> Self {
        VcardItem::from(replace_uid(&self.raw, new_uid))
    }

    #[inline]
    #[must_use]
    /// Returns the raw contents of this item.
    fn as_str(&self) -> &str {
        &self.raw
    }
}

impl From<String> for VcardItem {
    fn from(value: String) -> Self {
        VcardItem { raw: value }
    }
}

/// An item plus metadata returned when fetching it.
pub struct FetchedItem<I: Item> {
    /// See [`Href`]
    pub href: Href,
    /// The actual content of this item. See [`Item`].
    pub item: I,
    /// See [`Etag`]
    pub etag: Etag,
}
