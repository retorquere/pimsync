// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Traits and common implementations shared by different storages.
//!
//! When writing code that should deal with different storage implementations, these traits should
//! be used as input / outputs, rather than concrete per-store types.
//!
//! See [`Storage`] as an entry point to this module.

use std::{collections::VecDeque, str::FromStr, sync::Arc, time::Duration};

use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use vparser::Parser;

use crate::{
    disco::Discovery,
    watch::{IntervalMonitor, StorageMonitor},
    CollectionId, Etag, Href, Result,
};
// TODO: See (in vdirsyncer) IGNORE_PROPS for more props that might make sense to ignore.
pub const ICS_FIELDS_TO_IGNORE: &[&str] = &[
    // Servers often mutate this; resulting in noise when comparing.
    "PRODID",
    // When the information was last revised.
    // I don't think that servers SHOULD modify this, but they often do.
    // See: https://www.rfc-editor.org/rfc/rfc5545#section-3.8.7.2
    "DTSTAMP",
    // Ditto
    // See: https://www.rfc-editor.org/rfc/rfc5545#section-3.8.7.3
    "LAST-MODIFIED",
];

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
pub trait Storage<I: ItemKind>: Sync + Send {
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
    async fn get_item(&self, href: &str) -> Result<(Item, Etag)>;

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
    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem>> {
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
    async fn get_all_items(&self, collection: &str) -> Result<Vec<FetchedItem>> {
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
    async fn add_item(&self, collection: &str, item: &Item) -> Result<ItemRef>;

    /// Updates the contents of an existing item.
    async fn update_item(&self, href: &str, etag: &Etag, item: &Item) -> Result<Etag>;

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
/// See [`ItemKind::Property`].
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

pub trait ItemKind: Sync + Send + std::fmt::Debug + Clone {
    /// Property types supported by storages.
    ///
    /// These were known as "metadata" in the original vdirsyncer implementation.
    ///
    /// See also [`Storage::get_property`] and [`Storage::set_property`].
    type Property: Property;
}

/// A type of item that is contained in a [`Storage`].
///
/// A `Storage` can contain items of a concrete type described by implementations of this trait.
/// This trait defines how to extract the basic information that is required to synchronise
/// storages. Additional parsing is out of scope here and should be done by inspecting the raw data
/// inside an item via [`Item::as_str`].
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    raw: String,
}

impl Item {
    /// Parse the item and return the value of its `UID` property, if defined..
    ///
    /// The `uid` does not change when the item is modified. The `uid` remains the same when the
    /// item is copied across storages and storage types.
    #[must_use]
    pub fn uid(&self) -> Option<String> {
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

    /// Return the SHA256 hash of an icalendar or vcard.
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
    pub fn hash(&self) -> ItemHash {
        let mut hasher = Sha256::new();
        let parser = Parser::new(&self.raw);

        let mut in_tz = false;
        let mut tz_lines = VecDeque::new();

        for line in parser {
            if ICS_FIELDS_TO_IGNORE.contains(&line.name().as_ref()) {
                continue;
            }

            // TODO: strip/normalize timezones (tip: they are sometimes renamed)?
            // TODO: normalise order of lines inside each component?
            let raw = line.raw();
            if raw.is_empty() {
                continue;
            }

            // Swallow timezones, so we place them at the end.
            if line.name() == "BEGIN" && line.value() == "VTIMEZONE" {
                in_tz = true;
            }
            if in_tz {
                if line.name() == "END" && line.value() == "VTIMEZONE" {
                    in_tz = false;
                }
                tz_lines.push_back(line);
                continue;
            }

            // Place all timezones at the end to normalise discrepancies in ordering.
            if line.name() == "END" && line.value() == "VCALENDAR" {
                while let Some(l) = tz_lines.pop_front() {
                    hasher.update(l.unfolded().as_ref());
                    hasher.update("\r\n"); // Included even for the last line.
                }
            }

            // Use unfolded lines to ignore discrepancies in folding.
            hasher.update(line.unfolded().as_ref());
            hasher.update("\r\n"); // Included even for the last line.
        }

        // Only extremely malformed entries will match this branch,
        // well-formed icalendar files will have drained this queue already.
        while let Some(l) = tz_lines.pop_front() {
            hasher.update(l.unfolded().as_ref());
            hasher.update("\r\n"); // Included even for the last line.
        }

        ItemHash(Arc::from(<[u8; 32]>::from(hasher.finalize())))
    }

    /// A unique identifier for this item. Is either the UID (if any), or the hash of its contents.
    #[must_use]
    pub fn ident(&self) -> String {
        self.uid().unwrap_or_else(|| self.hash().to_string())
    }

    /// Returns a new copy of this Item with the supplied UID.
    #[must_use]
    pub fn with_uid(&self, new_uid: &str) -> Self {
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
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

/// The hash of an item. See [`Item::hash`].
#[derive(Default, PartialEq, Clone)]
pub struct ItemHash(Arc<[u8; 32]>);

// TODO: must confirm that this matches previous impl to ensure statusDb makes sense.
impl std::fmt::Display for ItemHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0.iter() {
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for ItemHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ItemHash(")?;
        for byte in self.0.iter() {
            write!(f, "{byte:02X}")?;
        }
        write!(f, ")")
    }
}

/// Error returned by [`ItemHash::from_str`].
#[derive(Debug, thiserror::Error)]
pub enum ItemHashError {
    #[error("Hash must be exactly 64 characters long")]
    InvalidLength,
    #[error("Invalid character in hash representation")]
    InvalidCharacter,
}

impl FromStr for ItemHash {
    type Err = ItemHashError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64 {
            return Err(ItemHashError::InvalidLength);
        }

        let mut bytes = [0u8; 32];
        for (i, chunk) in value.as_bytes().chunks(2).enumerate() {
            let hex = std::str::from_utf8(chunk).map_err(|_| ItemHashError::InvalidCharacter)?;
            bytes[i] = u8::from_str_radix(hex, 16).map_err(|_| ItemHashError::InvalidCharacter)?;
        }

        Ok(ItemHash(Arc::new(bytes)))
    }
}

impl From<String> for Item {
    /// Creates a new instance from valid iCalendar data.
    fn from(value: String) -> Self {
        Item { raw: value }
    }
}

/// Item fetched from a storage plus its metadata.
pub struct FetchedItem {
    /// See [`Href`]
    pub href: Href,
    /// The actual content of this item. See [`Item`].
    pub item: Item,
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

#[cfg(test)]
mod test {
    use crate::base::Item;

    #[test]
    fn compare_hashing_with_and_without_prodid() {
        let without_prodid: Item = [
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n")
        .into();
        let with_prodid: Item = [
            "PRODID:test-client",
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n")
        .into();

        assert_eq!(without_prodid.hash(), with_prodid.hash());
        assert_eq!(
            without_prodid.hash().to_string(),
            "E6DF19EB84E6DCE351EFB015D25C76D31A1FE09F2A8732BE6BC565A01EFA1A41"
        );
    }

    #[test]
    fn compare_hashing_with_different_folding() {
        let first: Item = [
            "DESCRIPTION:Voor meer informatie zie https://nluug.nl/evenementen/nluug/na",
            " jaarsconferentie-2023/",
        ]
        .join("\r\n")
        .into();
        let second: Item = [
            "DESCRIPTION:Voor meer informatie zie https:",
            " //nluug.nl/evenementen/nluug/najaarsconferentie-2023/",
        ]
        .join("\r\n")
        .into();

        assert_eq!(first.hash(), second.hash());
        assert_eq!(
            first.hash().to_string(),
            "9FCE34302FB7B6677542987089C91FDDF79F18F1D42862B03B1DEDF8E72F0CE2"
        );
    }

    #[test]
    fn hash_with_reordered_timezone() {
        let timezone_first:Item = [
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "CALSCALE:GREGORIAN",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Amsterdam",
            "X-LIC-LOCATION:Europe/Amsterdam",
            "BEGIN:DAYLIGHT",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "TZNAME:CEST",
            "DTSTART:19700329T020000",
            "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU",
            "END:DAYLIGHT",
            "BEGIN:STANDARD",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "TZNAME:CET",
            "DTSTART:19701025T030000",
            "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU",
            "END:STANDARD",
            "END:VTIMEZONE",
            "BEGIN:VEVENT",
            "UID:DF1E090791D8A93F3B530CFDA9CBFC0573CE3AB61C63A02AA33051B903F68A82",
            "SUMMARY:NLUUG najaarsconferentie 2023",
            "DESCRIPTION:Voor meer informatie zie https://nluug.nl/evenementen/nluug/najaarsconferentie-2023/",
            "DTSTART;TZID=Europe/Amsterdam:20231128T083000",
            "DTEND;TZID=Europe/Amsterdam:20231128T180000",
            "LOCATION:Winthontlaan 4-6, Utrecht, The Netherlands",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n").into();
        let timezone_last :Item = [
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "CALSCALE:GREGORIAN",
            "BEGIN:VEVENT",
            "UID:DF1E090791D8A93F3B530CFDA9CBFC0573CE3AB61C63A02AA33051B903F68A82",
            "SUMMARY:NLUUG najaarsconferentie 2023",
            "DESCRIPTION:Voor meer informatie zie https://nluug.nl/evenementen/nluug/najaarsconferentie-2023/",
            "DTSTART;TZID=Europe/Amsterdam:20231128T083000",
            "DTEND;TZID=Europe/Amsterdam:20231128T180000",
            "LOCATION:Winthontlaan 4-6, Utrecht, The Netherlands",
            "END:VEVENT",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Amsterdam",
            "X-LIC-LOCATION:Europe/Amsterdam",
            "BEGIN:DAYLIGHT",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "TZNAME:CEST",
            "DTSTART:19700329T020000",
            "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU",
            "END:DAYLIGHT",
            "BEGIN:STANDARD",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "TZNAME:CET",
            "DTSTART:19701025T030000",
            "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU",
            "END:STANDARD",
            "END:VTIMEZONE",
            "END:VCALENDAR",
        ]
        .join("\r\n").into();

        assert_eq!(timezone_first.hash(), timezone_last.hash());
    }

    #[test]
    fn test_single_line_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "END:VCARD"].join("\r\n");
        let item = Item::from(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));

        let raw = ["BEGIN:VCARD", "UID:hel", "lo", "END:VCARD"].join("\r\n");
        let item = Item::from(raw);
        assert_eq!(item.uid(), Some(String::from("hel")));
        assert_eq!(item.ident(), String::from("hel"));

        let raw = [
            "BEGIN:VCARD",
            "UID:hello",
            "REV:20210307T195614Z\tthere",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = Item::from(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));
    }

    #[test]
    fn test_multi_line_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "\tthere", "END:VCARD"].join("\r\n");
        let item = Item::from(raw);
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
        let item = Item::from(raw);
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
        let item = Item::from(raw);
        assert_eq!(item.uid(), None);
        assert_eq!(item.ident(), item.hash().to_string());
    }

    #[test]
    fn test_with_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "END:VCARD"].join("\r\n");
        let item = Item::from(raw);
        let item2 = item.with_uid("goodbye");
        assert_eq!(item2.uid(), Some(String::from("goodbye")));
        assert_eq!(item2.ident(), String::from("goodbye"));
    }

    #[test]
    fn test_with_uid_without_uid() {
        let raw = ["BEGIN:VCARD", "SUMMARY:hello", "END:VCARD"].join("\r\n");
        let item = Item::from(raw);
        let item2 = item.with_uid("goodbye");
        assert_eq!(item2.uid(), None);
    }
}
