// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Traits and common implementations shared by different storages.
//!
//! When writing code that should deal with different storage implementations, these traits should
//! be used as input / outputs, rather than concrete per-store types.
//!
//! See [`Storage`] as an entry point to this module.

use std::time::Duration;

use async_trait::async_trait;
use vparser::Parser;

use crate::{
    disco::Discovery,
    util::ItemHash,
    watch::{IntervalMonitor, StorageMonitor},
    CollectionId, Etag, Href, Result,
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

    /// List all properties of a collection.
    async fn list_properties(
        &self,
        collection_href: &str,
    ) -> Result<Vec<FetchedProperty<I::Property>>>;

    /// Returns the value of a property for a given collection.
    async fn get_property(&self, href: &str, property: I::Property) -> Result<Option<String>>;

    /// Sets the value of a property for a given collection.
    async fn set_property(&self, href: &str, property: I::Property, value: &str) -> Result<()>;

    /// Unsets a property for a given collection.
    async fn unset_property(&self, href: &str, property: I::Property) -> Result<()>;

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
    async fn update_item(&self, href: &str, etag: &Etag, item: &I) -> Result<Etag>;

    /// Deletes an existing item.
    async fn delete_item(&self, href: &str, etag: &Etag) -> Result<()>;

    /// Return the `href` for a collection that is expected to have `id`.
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
    /// Returns a future that resolves into a [`StorageMonitor`] instance, which can be polled for
    /// new events on the underlying storage.
    ///
    /// # Errors
    ///
    /// If an error occurs setting up the monitor. In cases where monitoring is not possible due to
    /// limitations in the underlying storage, the `interval` should be used instead.
    async fn monitor(&self, interval: Duration) -> Result<Box<dyn StorageMonitor>> {
        Ok(Box::new(IntervalMonitor::new(interval)) as Box<dyn StorageMonitor>)
    }
}

/// Path to a collection (an address book or a calendar) inside a storage.
///
/// Collections contain zero or more items (e.g.: an address book contains events). Each item is
/// addressed by its own [`Href`].
///
/// This type wraps around the `href` for a collection on a given storage. The same `Collection`
/// instance should not be shared across different storages.
#[derive(Debug)]
pub struct Collection {
    href: Href,
}

impl Collection {
    /// The path to this collection inside the storage.
    ///
    /// An href must not change over time, and should be associated with an immutable property of the
    /// collection, like a URL path component, or the path to a directory.
    ///
    /// The exact meaning of this value is storage-specific, but should be remain consistent within
    /// a storage.
    #[must_use]
    pub fn href(&self) -> &Href {
        &self.href
    }

    /// Return the inner [`Href`] instance.
    #[must_use]
    pub fn into_href(self) -> Href {
        self.href
    }

    pub(crate) fn new(href: String) -> Collection {
        Collection { href }
    }
}

/// Reference to a specific version of an [`Item`] inside a collection.
#[derive(PartialEq, Debug, Clone)]
pub struct ItemRef {
    /// Path to the item.
    pub href: Href,
    /// Etag for the item.
    pub etag: Etag,
}

/// Properties for storage collections.
///
/// See [`Item::Property`].
pub trait Property:
    Sync + Send + Clone + Copy + std::fmt::Debug + std::hash::Hash + PartialEq + Eq + 'static
{
    /// Return a friendly name for this property.
    fn name(&self) -> &str;

    /// Return all known properties.
    fn known_properties() -> &'static [Self]
    where
        Self: Sized;

    /// Return the filename suitable for storing this property's data.
    ///
    /// This is used by the [`crate::vdir::VdirStorage`], and may be used by other future storages
    /// where the same semantics are appropriate.
    fn filename(&self) -> &str;
}

/// A type of item that is contained in a [`Storage`].
///
/// A `Storage` can contain items of a concrete type described by implementations of this trait.
/// This trait defines how to extract the basic information that is required to synchronise
/// storages. Additional parsing is out of scope here and should be done by inspecting the raw data
/// inside an item via [`Item::as_str`].
pub trait Item: Sync + Send + std::fmt::Debug + Clone
where
    Self: From<String>,
{
    /// Property types supported by storages.
    ///
    /// These were known as "metadata" in the original vdirsyncer implementation.
    ///
    /// See also [`Storage::get_property`] and [`Storage::set_property`].
    type Property: Property;

    /// Parse the item and return the value of its `UID` property, if defined..
    ///
    /// The `uid` does not change when the item is modified. The `uid` remains the same when the
    /// item is copied across storages and storage types.
    #[must_use]
    fn uid(&self) -> Option<String> {
        let mut lines = self.as_str().split_terminator("\r\n");
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

    /// Return the hash of this item, usually normalised.
    ///
    /// The content shall be normalised before hashing to ensure that two semantically equivalent
    /// items return the same hash.
    ///
    /// The output of the function shall remain the same across different versions, platforms and
    /// architectures.
    ///
    /// This value is used as a fallback when a storage backend doesn't provide [`Etag`] values, or
    /// when an item's [`Item::uid`] returns `None`.
    #[must_use]
    fn hash(&self) -> ItemHash {
        crate::util::hash(self.as_str())
    }

    /// A unique identifier for this item. Is either the UID (if any), or the hash of its contents.
    #[must_use]
    fn ident(&self) -> String {
        self.uid().unwrap_or_else(|| self.hash().to_string())
    }

    /// Returns a new copy of this Item with the supplied UID.
    #[must_use]
    fn with_uid(&self, new_uid: &str) -> Self {
        Self::from({
            let orig = self.as_str();
            let mut inside_component = false;
            let mut new = String::new();

            for line in Parser::new(orig) {
                if line.name() == "BEGIN"
                    && ["VEVENT", "VTODO", "VJOURNAL", "VCARD"].contains(&line.value().as_ref())
                {
                    inside_component = true;
                }
                if line.name() == "END"
                    && ["VEVENT", "VTODO", "VJOURNAL", "VCARD"].contains(&line.value().as_ref())
                {
                    inside_component = false;
                }
                if inside_component && line.name() == "UID" {
                    new.push_str("UID:");
                    new.push_str(new_uid);
                    new.push_str("\r\n");
                } else {
                    new.push_str(line.raw());
                    new.push_str("\r\n");
                }
            }

            new
        })
    }

    #[must_use]
    /// Returns the raw contents of this item.
    fn as_str(&self) -> &str;
}

/// Item fetched from a storage plus its metadata.
pub struct FetchedItem<I: Item> {
    /// See [`Href`]
    pub href: Href,
    /// The actual content of this item. See [`Item`].
    pub item: I,
    /// See [`Etag`]
    pub etag: Etag,
}

/// Property and its value fetched from a storage.
pub struct FetchedProperty<P: Property> {
    /// The kind of property.
    pub property: P,
    /// The value of the property.
    pub value: String,
}
