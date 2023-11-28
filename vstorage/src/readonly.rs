// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Wrappers for using storages in read-only mode.
//!
//! These wrappers wrap around a normal [`Storage`] instance, but return [`ReadOnly`] for
//! any write operations.
//!
//! [`ReadOnly`]: ErrorKind::ReadOnly

use std::sync::Arc;

use async_trait::async_trait;

use crate::base::Collection;
use crate::base::FetchedItem;
use crate::base::Item;
use crate::base::Storage;
use crate::CollectionId;
use crate::{ErrorKind, Etag, Result};

/// A wrapper around a [`Storage`] that disallows any write operations.
///
/// # Example
///
/// ```
/// # use vstorage::filesystem::FilesystemStorage;
/// # use crate::vstorage::base::Storage;
/// # use crate::vstorage::base::IcsItem;
/// # use vstorage::filesystem::FilesystemDefinition;
/// # use std::path::PathBuf;
/// # use vstorage::readonly::ReadOnlyStorage;
/// # use crate::vstorage::base::Definition;
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// let orig = FilesystemDefinition::<IcsItem>::new(
///     PathBuf::from("/path/to/storage/"),
///     String::from("ics"),
/// ).into_storage().await.unwrap();
///
/// let read_only = ReadOnlyStorage::from(orig);
/// # })
/// ```
pub struct ReadOnlyStorage<I: Item> {
    // FIXME: This ends up being an Arc containing another Arc. That sounds like an anti pattern.
    inner: Arc<dyn Storage<I>>,
}

#[async_trait]
impl<I: Item> Storage<I> for ReadOnlyStorage<I> {
    async fn check(&self) -> Result<()> {
        self.inner.check().await
    }

    async fn discover_collections(&self) -> Result<Vec<Collection>> {
        self.inner.discover_collections().await
    }

    async fn create_collection(&self, _href: &str) -> Result<Collection> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn create_collection_with_id(&self, _id: &CollectionId) -> Result<Collection> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn destroy_collection(&self, _href: &str) -> Result<()> {
        Err(ErrorKind::ReadOnly.into())
    }

    fn open_collection(&self, href: &str) -> Result<Collection> {
        self.inner.open_collection(href)
    }

    async fn list_items(&self, collection: &Collection) -> Result<Vec<crate::base::ItemRef>> {
        self.inner.list_items(collection).await
    }

    async fn get_item(&self, href: &str) -> Result<(I, Etag)> {
        self.inner.get_item(href).await
    }

    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem<I>>> {
        self.inner.get_many_items(hrefs).await
    }

    async fn get_all_items(&self, collection: &Collection) -> Result<Vec<FetchedItem<I>>> {
        self.inner.get_all_items(collection).await
    }

    async fn add_item(&self, _: &Collection, _: &I) -> Result<crate::base::ItemRef> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn update_item(&self, _: &str, _: &Etag, _: &I) -> Result<Etag> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn set_collection_property(
        &self,
        _: &Collection,
        _: I::CollectionProperty,
        _: &str,
    ) -> Result<()> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn get_collection_property(
        &self,
        collection: &Collection,
        meta: I::CollectionProperty,
    ) -> Result<Option<String>> {
        self.inner.get_collection_property(collection, meta).await
    }

    async fn delete_item(&self, _: &str, _: &Etag) -> Result<()> {
        Err(ErrorKind::ReadOnly.into())
    }

    fn collection_id(&self, collection: &Collection) -> Result<CollectionId> {
        self.inner.collection_id(collection)
    }
}

impl<I: Item> From<Arc<dyn Storage<I>>> for ReadOnlyStorage<I> {
    fn from(value: Arc<dyn Storage<I>>) -> Self {
        Self { inner: value }
    }
}
