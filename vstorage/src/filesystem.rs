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
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::{fs::Metadata, os::unix::prelude::MetadataExt};
use tokio::fs::{
    create_dir, metadata, read_dir, read_to_string, remove_dir, remove_file, File, OpenOptions,
};
use tokio::io::AsyncWriteExt;
use tokio_stream::wrappers::ReadDirStream;
use tokio_stream::StreamExt;

use crate::base::{
    AddressBookProperty, CalendarProperty, Collection, Definition, Item, ItemRef, Storage,
};
use crate::{CollectionId, Error, ErrorKind, Etag, Href, Result};

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

    async fn create_collection(&mut self, href: &str) -> Result<Collection> {
        let path = self.join_collection_href(href)?;
        create_dir(&path).await?;

        self.open_collection(href)
    }

    async fn create_collection_with_id(&mut self, id: &CollectionId) -> Result<Collection> {
        let path = self.join_collection_href(id.as_ref())?;
        create_dir(&path).await?;

        self.open_collection(id.as_ref())
    }

    async fn destroy_collection(&mut self, href: &str) -> Result<()> {
        let path = self.join_collection_href(href)?;
        remove_dir(path).await.map_err(Error::from)
    }

    fn open_collection(&self, href: &str) -> Result<Collection> {
        Ok(Collection::new(href.to_string()))
    }

    async fn list_items(&self, collection: &Collection) -> Result<Vec<ItemRef>> {
        let path = self.collection_path(collection);
        let mut read_dir = ReadDirStream::new(read_dir(path).await?);

        let mut items = Vec::new();
        while let Some(entry) = read_dir.next().await {
            let entry = entry?;
            let href = entry
                .file_name()
                .to_str()
                .ok_or_else(|| Error::new(ErrorKind::InvalidData, "Filename is not valid UTF-8"))?
                .into();
            let etag = etag_for_path(&entry.path()).await?;
            let item = ItemRef { href, etag };
            items.push(item);
        }

        Ok(items)
    }

    async fn get_item(&self, collection: &Collection, href: &str) -> Result<(I, Etag)> {
        let path = self.collection_path(collection).join(href);
        let meta = metadata(&path).await?;

        let item = I::from(read_to_string(&path).await?);
        let etag = etag_for_metadata(&meta);

        Ok((item, etag))
    }

    async fn get_many_items(
        &self,
        collection: &Collection,
        hrefs: &[&str],
    ) -> Result<Vec<(Href, I, Etag)>> {
        // No specialisation for this type; it's fast enough for now.
        let mut items = Vec::with_capacity(hrefs.len());
        for href in hrefs {
            let (item, etag) = self.get_item(collection, href).await?;
            items.push((String::from(*href), item, etag));
        }
        Ok(items)
    }

    async fn get_all_items(&self, collection: &Collection) -> Result<Vec<(Href, I, Etag)>> {
        let mut read_dir = read_dir(self.collection_path(collection)).await?;

        let mut items = Vec::new();
        while let Some(entry) = read_dir.next_entry().await? {
            let href: String = entry
                .file_name()
                .to_str()
                .ok_or_else(|| Error::new(ErrorKind::InvalidData, "Filename is not valid UTF-8"))?
                .into();
            let etag = etag_for_path(&entry.path()).await?;
            let item = I::from(read_to_string(&href).await?);
            items.push((href, item, etag));
        }

        Ok(items)
    }

    async fn set_collection_property(
        &mut self,
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

    async fn add_item(&mut self, collection: &Collection, item: &I) -> Result<ItemRef> {
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

    async fn update_item(
        &mut self,
        collection: &Collection,
        href: &str,
        etag: &Etag,
        item: &I,
    ) -> Result<Etag> {
        let filename = self.collection_path(collection).join(href);

        let actual_etag = etag_for_path(&filename).await?;
        if *etag != actual_etag {
            return Err(Error::new(ErrorKind::InvalidData, "wrong etag"));
        }

        // FIXME: this is racey and the etag can change after checking.
        // TODO: atomic writes.
        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(false)
            .open(&filename)
            .await?;
        file.write_all(item.as_str().as_bytes()).await?;

        let etag = etag_for_path(&filename).await?;
        Ok(etag)
    }

    async fn delete_item(
        &mut self,
        collection: &Collection,
        href: &str,
        etag: &Etag,
    ) -> Result<()> {
        let filename = self.collection_path(collection).join(href);

        let actual_etag = etag_for_path(&filename).await?;
        if *etag != actual_etag {
            return Err(Error::new(ErrorKind::InvalidData, "wrong etag"));
        }

        // FIXME: this is racey and the etag can change after checking.
        remove_file(filename).await?;

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

    pub fn build(self) -> FilesystemStorage<I> {
        FilesystemStorage { definition: self }
    }
}

#[async_trait]
impl<I: Item + 'static> Definition<I> for FilesystemDefinition<I>
where
    I::CollectionProperty: PropertyWithFilename,
{
    async fn build_boxed(self) -> Result<Box<dyn Storage<I>>> {
        Ok(Box::from(self.build()))
    }
}

async fn etag_for_path<P: AsRef<Path>>(path: P) -> Result<Etag> {
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
    use super::FilesystemDefinition;
    use crate::base::{Definition, IcsItem};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_missing_displayname() {
        let dir = tempdir().unwrap();
        let definition =
            FilesystemDefinition::<IcsItem>::new(dir.path().to_path_buf(), "ics".to_string());

        let mut storage = definition.build_boxed().await.unwrap();
        let collection = storage.create_collection("test").await.unwrap();
        let displayname = storage
            .get_collection_property(&collection, crate::base::CalendarProperty::DisplayName)
            .await
            .unwrap();

        assert!(displayname.is_none())
    }

    // #[test]
    // fn test_write_read_meta_name() {
    //     todo!();
    // }

    // TODO: test writing and then checking the file
    // TODO: test writing a file and then getting
}
