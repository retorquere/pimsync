// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Implements reading/writing entries from a local [`vdir`].
//!
//! - The `href` for an items is its filename relative to its parent directory.
//! - The `href` for a collection is its absolute path. This may change in future.
//!
//! [`vdir`]: https://vdirsyncer.pimutils.org/en/stable/vdir.html
use async_trait::async_trait;
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use std::ffi::OsStr;
use std::marker::PhantomData;
use std::os::unix::prelude::MetadataExt;
use std::path::Path;
use tokio::fs::{create_dir, metadata, read_dir, read_to_string, remove_dir, remove_file};
use tokio::io::AsyncWriteExt;

use crate::atomic::AtomicFile;
use crate::base::{
    AddressBookProperty, CalendarProperty, Collection, FetchedItem, Item, ItemRef, ListedProperty,
    Storage,
};
use crate::disco::{DiscoveredCollection, Discovery};
use crate::{CollectionId, Error, ErrorKind, Etag, Href, Result};

/// A `vdir` filesystem directory containing zero or more directories.
///
/// Each child directory is treated as [`Collection`]. Nested subdirectories are not supported.
///
/// # Hrefs
///
/// Internally, all `href`s are paths relative to the base directory.
// TODO: add link to spec here.
pub struct VdirStorage<I: Item> {
    /// The path to a directory containing a storage.
    ///
    /// Each top-level subdirectory will be treated as a separate collection, and individual files
    /// inside these are each treated as an `Item`.
    pub path: Utf8PathBuf,
    /// Filename extension for items in a storage. Files with matching extension are treated a
    /// items for a collection, and all other files are ignored.
    pub extension: String,
    i: PhantomData<I>,
}

#[async_trait]
impl<I: Item> Storage<I> for VdirStorage<I>
where
    I::Property: PropertyWithFilename,
{
    async fn check(&self) -> Result<()> {
        let meta = metadata(&self.path)
            .await
            .map_err(|e| Error::new(ErrorKind::DoesNotExist, e))?;

        if meta.is_dir() {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::NotAStorage,
                "path is not a directory",
            ))
        }
    }

    async fn discover_collections(&self) -> Result<Discovery> {
        let mut entries = read_dir(&self.path).await?;

        let mut collections = Vec::<_>::new();
        while let Some(entry) = entries.next_entry().await? {
            if !metadata(entry.path()).await?.is_dir() {
                continue;
            }
            let href = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::new(ErrorKind::InvalidData, "collection name is not utf8"))?;
            if href.starts_with('.') {
                continue;
            }
            let id = href
                .parse()
                .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
            collections.push(DiscoveredCollection::new(href, id));
        }

        Ok(collections.into())
    }

    async fn create_collection(&self, href: &str) -> Result<Collection> {
        let path = self.build_collection_path(href)?;
        create_dir(&path).await?;

        Ok(Collection::new(href.to_string()))
    }

    async fn destroy_collection(&self, href: &str) -> Result<()> {
        let path = self.build_collection_path(href)?;
        remove_dir(path).await.map_err(Error::from)
    }

    async fn list_items(&self, collection_href: &str) -> Result<Vec<ItemRef>> {
        let mut read_dir = read_dir(self.build_collection_path(collection_href)?).await?;

        let mut items = Vec::new();
        let extension = OsStr::new(self.extension.as_str());
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == extension) {
                continue;
            }
            let href = self.href_for_path(&path)?;
            let etag = etag_for_path(path).await?;

            items.push(ItemRef { href, etag });
        }

        Ok(items)
    }

    async fn get_item(&self, href: &str) -> Result<(I, Etag)> {
        let path = self.build_item_path(href)?;

        let item = I::from(read_to_string(&path).await?);
        let etag = etag_for_path(path).await?;

        Ok((item, etag))
    }

    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem<I>>> {
        // No specialisation for this type; it's fast enough for now.
        let mut items = Vec::with_capacity(hrefs.len());
        for href in hrefs {
            let (item, etag) = self.get_item(href).await?;
            items.push(FetchedItem {
                href: String::from(*href),
                item,
                etag,
            });
        }
        Ok(items)
    }

    async fn get_all_items(&self, collection_href: &str) -> Result<Vec<FetchedItem<I>>> {
        let mut read_dir = read_dir(self.build_collection_path(collection_href)?).await?;

        let mut items = Vec::new();
        let extension = OsStr::new(self.extension.as_str());
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == extension) {
                continue;
            }

            items.push(FetchedItem {
                href: self.href_for_path(&path)?,
                item: I::from(read_to_string(&path).await?),
                etag: etag_for_path(path).await?,
            });
        }

        Ok(items)
    }

    async fn set_property(&self, href: &str, meta: I::Property, value: &str) -> Result<()> {
        let filename = meta.filename();

        let path = self.build_collection_path(href)?.join(filename);
        let mut file = AtomicFile::new(path)?;

        file.write_all(value.as_bytes()).await?;
        file.commit()?;

        Ok(())
    }

    async fn unset_property(&self, href: &str, meta: I::Property) -> Result<()> {
        let filename = meta.filename();
        let path = self.build_collection_path(href)?.join(filename);
        remove_file(path).await?;
        Ok(())
    }

    async fn get_property(&self, href: &str, meta: I::Property) -> Result<Option<String>> {
        let filename = meta.filename();

        let path = self.build_collection_path(href)?.join(filename);
        match read_to_string(path).await {
            Ok(value) => Ok(Some(value)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::from(e)),
        }
    }

    async fn add_item(&self, collection_href: &str, item: &I) -> Result<ItemRef> {
        // TODO: We only need to remove a few "illegal" characters, so this is a bit too strict.
        let basename = item
            .ident()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>();

        let filename = format!("{}.{}", basename, self.extension);
        let relpath = Utf8PathBuf::from(collection_href).join(filename);

        let absolute_path = self.build_item_path(relpath.as_str())?;
        let mut file = AtomicFile::new(&absolute_path)?;
        file.write_all(item.as_str().as_bytes()).await?;
        file.commit_new()?;

        let item_ref = ItemRef {
            href: relpath.into_string(),
            etag: etag_for_path(absolute_path).await?,
        };
        Ok(item_ref)
    }

    async fn update_item(&self, href: &str, etag: &Etag, item: &I) -> Result<Etag> {
        let filename = self.build_item_path(href)?;

        let actual_etag = etag_for_path(&filename).await?;
        if *etag != actual_etag {
            return Err(Error::new(ErrorKind::InvalidData, "wrong etag"));
        }

        let mut file = AtomicFile::new(&filename)?;
        file.write_all(item.as_str().as_bytes()).await?;
        file.commit()?;

        // FIXME: this is racey and the etag can change after checking.
        //        I should use fstat here instead
        Ok(etag_for_path(filename).await?)
    }

    /// # Quirks
    ///
    /// Checking the etag is vulnerable to TOCTOU race conditions. Filesystem APIs do not provide
    /// facilities to work around this.
    async fn delete_item(&self, href: &str, etag: &Etag) -> Result<()> {
        let filename = self.build_item_path(href)?;

        let actual_etag = etag_for_path(&filename).await?;
        if *etag != actual_etag {
            return Err(Error::new(ErrorKind::InvalidData, "wrong etag"));
        }

        remove_file(filename).await?;

        Ok(())
    }

    /// The id of a filesystem collection is the name of the directory.
    fn collection_id(&self, collection_href: &str) -> Result<CollectionId> {
        collection_href
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .expect("rsplit always returns at least one item")
            .parse()
            .map_err(|e| Error::new(ErrorKind::InvalidInput, e))
    }

    fn href_for_collection_id(&self, id: &CollectionId) -> Result<Href> {
        Ok(id.to_string())
    }

    async fn list_properties(
        &self,
        collection_href: &str,
    ) -> Result<Vec<ListedProperty<I::Property>>> {
        let mut props = Vec::new();
        for property in I::Property::known_properties() {
            let prop_value = self.get_property(collection_href, property.clone()).await?;
            if let Some(value) = prop_value {
                props.push(ListedProperty {
                    property: property.clone(),
                    value,
                });
            };
        }
        Ok(props)
    }
}

impl<I: Item> VdirStorage<I> {
    #[must_use]
    pub fn new(path: Utf8PathBuf, extension: String) -> Self {
        Self {
            path,
            extension,
            i: PhantomData,
        }
    }

    /// Joins an href to the storage's path.
    ///
    /// This method does safety checks to ensure that malicious input cannot write outside the
    /// storage's directory.
    ///
    /// # Errors
    ///
    /// - If the input is an invalid directory name
    /// - If the resulting path is not a child of the storage's directory.
    fn build_collection_path(&self, href: &str) -> Result<Utf8PathBuf> {
        let href = Utf8Path::new(href);
        let mut components = href.components();
        if !matches!(components.next(), Some(Utf8Component::Normal(_))) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "collection href must be a valid directory name",
            ));
        };
        if components.next().is_some() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "collection href must contain exactly one component",
            ));
        };

        Ok(self.path.join(href))
    }

    fn build_item_path(&self, href: &str) -> Result<Utf8PathBuf> {
        let href = Utf8Path::new(href);

        let mut components = href.components();
        if !matches!(components.next(), Some(Utf8Component::Normal(_))) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "first component of item href must be a regular filename",
            ));
        };
        if let Some(Utf8Component::Normal(name)) = components.next() {
            let name = Utf8Path::new(name);
            if !name.extension().is_some_and(|e| e == self.extension) {
                Err(Error::new(
                    ErrorKind::InvalidInput,
                    "item href does not have an extension matching this storage",
                ))?;
            }
        } else {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "second component of item href must be a regular filename",
            ));
        }
        if components.next().is_some() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "item href cannot contain more than two components",
            ));
        };

        Ok(self.path.join(href))
    }

    /// Returns the href for a path.
    ///
    /// # Panics
    ///
    /// If `path` is not a grandchild of the storage's path.
    fn href_for_path(&self, path: &Path) -> Result<String> {
        path.strip_prefix(&self.path)
            // This never takes external input. If this panics, we have a bug.
            .expect("path of item must include storage path as prefix")
            .to_str()
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "Filename is not valid UTF-8"))
            .map(str::to_string)
    }
}

async fn etag_for_path(path: impl AsRef<Path>) -> Result<Etag> {
    let metadata = &metadata(path).await?;
    Ok(format!("{};{}", metadata.mtime(), metadata.ino()).into())
}

/// Helper to synchronise collection properties into filesystem.
///
/// This trait should only be required when implementing a new [`Item`] type that should work with
/// the existing [`VdirStorage`] implementation.
///
/// In order for the `Item`'s properties to synchronise to the filesystem, it should implement this
/// trait.
pub trait PropertyWithFilename: 'static {
    fn filename(&self) -> &'static str;

    fn known_properties() -> &'static [Self]
    where
        Self: Sized;
}

impl PropertyWithFilename for CalendarProperty {
    fn filename(&self) -> &'static str {
        match self {
            CalendarProperty::DisplayName => "displayname",
            CalendarProperty::Colour => "color",
            CalendarProperty::Description => "description",
            CalendarProperty::Order => "order",
        }
    }

    fn known_properties() -> &'static [Self] {
        &[
            CalendarProperty::DisplayName,
            CalendarProperty::Colour,
            CalendarProperty::Description,
            CalendarProperty::Order,
        ]
    }
}

impl PropertyWithFilename for AddressBookProperty {
    fn filename(&self) -> &'static str {
        match self {
            AddressBookProperty::DisplayName => "displayname",
            AddressBookProperty::Description => "description",
        }
    }

    fn known_properties() -> &'static [Self] {
        &[
            AddressBookProperty::DisplayName,
            AddressBookProperty::Description,
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{create_dir_all, write},
        str::FromStr,
    };

    use crate::{
        base::{CalendarProperty, IcsItem, Storage},
        vdir::VdirStorage,
        CollectionId, ErrorKind,
    };
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_missing_displayname() {
        let dir = tempdir().unwrap();

        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );
        let collection = storage.create_collection("test").await.unwrap();
        let displayname = storage
            .get_property(
                collection.href(),
                crate::base::CalendarProperty::DisplayName,
            )
            .await
            .unwrap();

        assert!(displayname.is_none());
    }

    #[tokio::test]
    async fn test_path_handling() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let collection_name = "one";
        let collection_path = dir.path().join(collection_name);
        create_dir_all(&collection_path).unwrap();

        let without_prodid = [
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");

        write(collection_path.join("item.ics"), &without_prodid).unwrap();

        let listed_items = storage.list_items(collection_name).await.unwrap();
        assert_eq!(listed_items.len(), 1);
        assert_eq!(listed_items[0].href, "one/item.ics");

        let all_items = storage.get_all_items(collection_name).await.unwrap();
        assert_eq!(all_items.len(), 1);
        assert_eq!(all_items[0].href, "one/item.ics");

        let (_item, etag) = storage.get_item("one/item.ics").await.unwrap();
        // Nothing to assert here.

        let many_items = storage.get_many_items(&["one/item.ics"]).await.unwrap();
        assert_eq!(many_items.len(), 1);
        assert_eq!(many_items[0].href, "one/item.ics");

        storage.delete_item("one/item.ics", &etag).await.unwrap();

        let item = IcsItem::from(without_prodid);
        storage.add_item("one", &item).await.unwrap();

        let all_items = storage.get_all_items(collection_name).await.unwrap();
        assert_eq!(all_items.len(), 1);
    }

    #[tokio::test]
    async fn test_missing_paths() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let missing_collection = "two";
        let err = match storage.list_items(missing_collection).await {
            Ok(items) => panic!("expected error, got {} result.", items.len()),
            Err(e) => e,
        };
        assert_eq!(err.kind, ErrorKind::DoesNotExist);

        // TODO: more tests on the missing collection
    }

    #[tokio::test]
    async fn test_write_read_colour() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let collection_name = "one";
        storage.create_collection(collection_name).await.unwrap();

        storage
            .set_property(collection_name, CalendarProperty::Colour, "#000000")
            .await
            .unwrap();

        let colour = storage
            .get_property(collection_name, CalendarProperty::Colour)
            .await
            .unwrap();

        assert_eq!(colour, Some(String::from("#000000")));
    }

    #[tokio::test]
    async fn test_read_missing_description() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let collection_name = "one";
        storage.create_collection(collection_name).await.unwrap();

        let description = storage
            .get_property(collection_name, CalendarProperty::Description)
            .await
            .unwrap();

        assert_eq!(description, None);
    }

    // TODO: test writing and then checking the file
    // TODO: test writing a file and then getting
    //
    #[tokio::test]
    async fn test_href_for_collection_id() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let collection_id = CollectionId::from_str("one").unwrap();
        let href = storage.href_for_collection_id(&collection_id).unwrap();
        assert_eq!(href, "one");
    }

    #[tokio::test]
    async fn test_build_collection_path_is_safe() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        assert!(storage.build_collection_path("penguins").is_ok());
        assert!(storage.build_collection_path("penguins/").is_ok());
        assert!(storage.build_collection_path("蛙类").is_ok());

        assert!(storage.build_collection_path("/").is_err());
        assert!(storage.build_collection_path("/usr/share/").is_err());
        assert!(storage.build_collection_path("..").is_err());
        assert!(storage.build_collection_path(".").is_err());
        assert!(storage.build_collection_path("../d").is_err());
        assert!(storage.build_collection_path("s/../../").is_err());
    }

    #[tokio::test]
    async fn test_build_item_path_is_safe() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        assert!(storage.build_item_path("penguins/someitem.ics").is_ok());
        assert!(storage.build_item_path("蛙类/item.ics").is_ok());

        assert!(storage.build_item_path("penguins/someitem.jpeg").is_err());
        assert!(storage.build_item_path("蛙类/item.jpeg").is_err());
        assert!(storage.build_item_path("penguins/someitem").is_err());
        assert!(storage.build_item_path("蛙类/item").is_err());
        assert!(storage.build_item_path("penguins").is_err());
        assert!(storage.build_item_path("蛙类").is_err());
        assert!(storage.build_item_path("penguins/../someitem.ics").is_err());
        assert!(storage.build_item_path("../penguins/someitem.ics").is_err());
        assert!(storage.build_item_path("/").is_err());
        assert!(storage.build_item_path("/usr/share/").is_err());
        assert!(storage.build_item_path("..").is_err());
        assert!(storage.build_item_path(".").is_err());
        assert!(storage.build_item_path("s/../../").is_err());
    }
}
