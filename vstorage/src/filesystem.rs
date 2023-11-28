// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Implements reading/writing entries from a local filesystem [`vdir`].
//!
//! - The `href` for an items is its filename relative to its parent directory.
//! - The `href` for a collection is its absolute path. This may change in future.
//!
//! [`vdir`]: https://vdirsyncer.pimutils.org/en/stable/vdir.html
#![allow(clippy::module_name_repetitions)]

use async_trait::async_trait;
use std::ffi::OsStr;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs::Metadata, os::unix::prelude::MetadataExt};
use tokio::fs::{
    create_dir, metadata, read_dir, read_to_string, remove_dir, remove_file, File, OpenOptions,
};
use tokio::io::AsyncWriteExt;

use crate::base::{
    AddressBookProperty, CalendarProperty, Collection, Definition, FetchedItem, Item, ItemRef,
    Storage,
};
use crate::{CollectionId, Error, ErrorKind, Etag, Result};

// TODO: atomic writes

/// A filesystem directory containing zero or more directories.
///
/// Each child directory is treated as [`Collection`]. Nested subdirectories are not supported.
///
/// # Hrefs
///
/// Internally, all `href`s are paths relative to the base directory.
pub struct FilesystemStorage<I: Item> {
    definition: FilesystemDefinition<I>,
}

#[async_trait]
impl<I: Item> Storage<I> for FilesystemStorage<I>
where
    I::CollectionProperty: PropertyWithFilename,
{
    async fn check(&self) -> Result<()> {
        let meta = metadata(&self.definition.path)
            .await
            .map_err(|e| Error::new(ErrorKind::DoesNotExist, e))?;

        if meta.is_dir() {
            Ok(())
        } else {
            Err(Error::from(ErrorKind::NotAStorage))
        }
    }

    async fn discover_collections(&self) -> Result<Vec<Collection>> {
        let mut entries = read_dir(&self.definition.path).await?;

        let mut collections = Vec::<Collection>::new();
        while let Some(entry) = entries.next_entry().await? {
            if !metadata(entry.path()).await?.is_dir() {
                continue;
            }
            let href = entry
                .file_name()
                .to_str()
                .ok_or_else(|| Error::new(ErrorKind::InvalidData, "collection name is not utf8"))?
                .to_owned();

            collections.push(Collection::new(href));
        }

        Ok(collections)
    }

    async fn create_collection(&self, href: &str) -> Result<Collection> {
        let path = self.join_collection_href(href)?;
        create_dir(&path).await?;

        self.open_collection(href)
    }

    async fn create_collection_with_id(&self, id: &CollectionId) -> Result<Collection> {
        let path = self.join_collection_href(id.as_ref())?;
        create_dir(&path).await?;

        self.open_collection(id.as_ref())
    }

    async fn destroy_collection(&self, href: &str) -> Result<()> {
        let path = self.join_collection_href(href)?;
        remove_dir(path).await.map_err(Error::from)
    }

    fn open_collection(&self, href: &str) -> Result<Collection> {
        Ok(Collection::new(href.to_string()))
    }

    async fn list_items(&self, collection: &Collection) -> Result<Vec<ItemRef>> {
        let mut read_dir = read_dir(self.collection_path(collection)).await?;

        let mut items = Vec::new();
        let extension = OsStr::new(self.definition.extension.as_str());
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
        let path = self.definition.path.join(href);

        let item = I::from(read_to_string(&path).await?);
        let etag = etag_for_path(&path).await?;

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

    async fn get_all_items(&self, collection: &Collection) -> Result<Vec<FetchedItem<I>>> {
        let mut read_dir = read_dir(self.collection_path(collection)).await?;

        let mut items = Vec::new();
        let extension = OsStr::new(self.definition.extension.as_str());
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == extension) {
                continue;
            }

            items.push(FetchedItem {
                href: self.href_for_path(&path)?,
                item: I::from(read_to_string(&path).await?),
                etag: etag_for_path(&path).await?,
            });
        }

        Ok(items)
    }

    async fn set_collection_property(
        &self,
        collection: &Collection,
        meta: I::CollectionProperty,
        value: &str,
    ) -> Result<()> {
        let filename = meta.filename();

        let path = self.collection_path(collection).join(filename);
        let mut file = File::create(path).await?;

        file.write_all(value.as_bytes()).await?;
        Ok(())
    }

    async fn get_collection_property(
        &self,
        collection: &Collection,
        meta: I::CollectionProperty,
    ) -> Result<Option<String>> {
        let filename = meta.filename();

        let path = self.collection_path(collection).join(filename);
        let value = match read_to_string(path).await {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::from(e)),
        };

        Ok(Some(value))
    }

    async fn add_item(&self, collection: &Collection, item: &I) -> Result<ItemRef> {
        // TODO: We only need to remove a few "illegal" characters, so this is a bit too strict.
        let basename = item
            .ident()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>();
        let href = format!("{}.{}", basename, self.definition.extension);

        let filename = self.collection_path(collection).join(&href);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&filename)
            .await?;
        file.write_all(item.as_str().as_bytes()).await?;

        let item_ref = ItemRef {
            href,
            etag: etag_for_path(&filename).await?,
        };
        Ok(item_ref)
    }

    async fn update_item(&self, href: &str, etag: &Etag, item: &I) -> Result<Etag> {
        let actual_etag = etag_for_path(&href).await?;
        if *etag != actual_etag {
            return Err(Error::new(ErrorKind::InvalidData, "wrong etag"));
        }

        // FIXME: this is racey and the etag can change after checking.
        // TODO: atomic writes.
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(false)
            .open(&href)
            .await?;
        file.write_all(item.as_str().as_bytes()).await?;

        let etag = etag_for_path(&href).await?;
        Ok(etag)
    }

    async fn delete_item(&self, href: &str, etag: &Etag) -> Result<()> {
        let actual_etag = etag_for_path(&href).await?;
        if *etag != actual_etag {
            return Err(Error::new(ErrorKind::InvalidData, "wrong etag"));
        }

        // FIXME: this is racey and the etag can change after checking.
        remove_file(href).await?;

        Ok(())
    }

    /// The id of a filesystem collection is the name of the directory.
    fn collection_id(&self, collection: &Collection) -> Result<CollectionId> {
        collection
            .href()
            .rsplit('/')
            .next()
            .expect("rsplit always returns at least one item")
            .parse()
            .map_err(|e| Error::new(ErrorKind::InvalidInput, e))
    }
}

impl<I: Item> FilesystemStorage<I> {
    fn collection_path(&self, collection: &Collection) -> PathBuf {
        self.definition.path.join(collection.href())
    }

    // Joins an href to the storage's path.
    //
    // # Errors
    //
    // If the resulting path is not a child of the storage's directory.
    fn join_collection_href(&self, href: &str) -> Result<PathBuf> {
        // TODO: validate that no `.` nor `..` components are in the input.
        let path = self.definition.path.join(href);
        if path.parent() != Some(&self.definition.path) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "directory is not child of storage directory",
            ));
        };

        Ok(path)
    }

    /// Returns the href for a path.
    ///
    /// # Panics
    ///
    /// If `path` is not a grandchild of the storage's path.
    fn href_for_path(&self, path: &Path) -> Result<String> {
        path.strip_prefix(&self.definition.path)
            // This never takes external input. If this panics, we have a bug.
            .expect("path of item must include storage path as prefix")
            .to_str()
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "Filename is not valid UTF-8"))
            .map(str::to_string)
    }
}

/// Definition for a storage instance.
#[derive(serde::Deserialize, Debug)]
pub struct FilesystemDefinition<I: Item> {
    /// The path to a directory containing a storage.
    ///
    /// Each top-level subdirectory will be treated as a separate collection, and individual files
    /// inside these are each treated as an `Item`.
    pub path: PathBuf,
    /// Filename extension for items in a storage. Files with matching extension are treated a
    /// items for a collection, and all other files are ignored.
    pub extension: String,
    i: PhantomData<I>,
}

impl<I: Item> FilesystemDefinition<I> {
    #[must_use]
    pub fn new(path: PathBuf, extension: String) -> Self {
        Self {
            path,
            extension,
            i: PhantomData,
        }
    }

    /// Build a new `Storage` instance.
    #[must_use]
    pub fn build(self) -> FilesystemStorage<I> {
        FilesystemStorage { definition: self }
    }
}

#[async_trait]
impl<I: Item + 'static> Definition<I> for FilesystemDefinition<I>
where
    I::CollectionProperty: PropertyWithFilename,
{
    async fn into_storage(self) -> Result<Arc<dyn Storage<I>>> {
        Ok(Arc::from(self.build()))
    }
}

async fn etag_for_path(path: impl AsRef<Path>) -> Result<Etag> {
    let metadata = metadata(path).await?;
    Ok(etag_for_metadata(&metadata))
}

fn etag_for_metadata(metadata: &Metadata) -> Etag {
    format!("{};{}", metadata.mtime(), metadata.ino()).into()
}

/// Helper to synchronise collection properties into filesystem.
///
/// This trait should only be required when implementing a new [`Item`] type that should work with
/// the existing [`FilesystemStorage`] implementation.
///
/// In order for the `Item`'s properties to synchronise to the filesystem, it should implement this
/// trait.
pub trait PropertyWithFilename {
    fn filename(&self) -> &'static str;
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
}

impl PropertyWithFilename for AddressBookProperty {
    fn filename(&self) -> &'static str {
        match self {
            AddressBookProperty::DisplayName => "displayname",
            AddressBookProperty::Description => "description",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{create_dir_all, write};

    use super::FilesystemDefinition;
    use crate::{
        base::{Collection, Definition, IcsItem, Storage},
        ErrorKind,
    };
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_missing_displayname() {
        let dir = tempdir().unwrap();
        let definition =
            FilesystemDefinition::<IcsItem>::new(dir.path().to_path_buf(), "ics".to_string());

        let storage = definition.into_storage().await.unwrap();
        let collection = storage.create_collection("test").await.unwrap();
        let displayname = storage
            .get_collection_property(&collection, crate::base::CalendarProperty::DisplayName)
            .await
            .unwrap();

        assert!(displayname.is_none())
    }

    #[tokio::test]
    async fn test_path_handling() {
        let dir = tempdir().unwrap();
        let definition =
            FilesystemDefinition::<IcsItem>::new(dir.path().to_path_buf(), "ics".to_string());
        let storage = definition.build();

        let collection_path = dir.path().join("one");
        create_dir_all(&collection_path).unwrap();

        let without_prodid = vec![
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

        write(collection_path.join("item.ics"), without_prodid).unwrap();
        let collection = Collection::new("one".to_string());

        let listed_items = storage.list_items(&collection).await.unwrap();
        assert_eq!(listed_items.len(), 1);
        assert_eq!(listed_items[0].href, "one/item.ics");

        let all_items = storage.get_all_items(&collection).await.unwrap();
        assert_eq!(all_items.len(), 1);
        assert_eq!(all_items[0].href, "one/item.ics");

        let _item = storage.get_item("one/item.ics").await.unwrap();
        // Nothing to assert here.

        let many_items = storage.get_many_items(&["one/item.ics"]).await.unwrap();
        assert_eq!(many_items.len(), 1);
        assert_eq!(many_items[0].href, "one/item.ics");

        let missing_collection = Collection::new("two".to_string());
        let err = match storage.list_items(&missing_collection).await {
            Ok(items) => panic!("expected error, got {} result.", items.len()),
            Err(e) => e,
        };
        assert_eq!(err.kind, ErrorKind::DoesNotExist);

        // TODO: more tests on the missing collection
    }

    // #[test]
    // fn test_write_read_meta_name() {
    //     todo!();
    // }

    // TODO: test writing and then checking the file
    // TODO: test writing a file and then getting
}
