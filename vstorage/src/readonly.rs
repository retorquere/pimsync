// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Wrappers for using storages in read-only mode.
//!
//! These wrappers wrap around a normal [`Storage`] instance, but return [`ReadOnly`] for
//! any write operations.
//!
//! [`ReadOnly`]: ErrorKind::ReadOnly

use std::marker::PhantomData;
use std::time::Duration;

use async_trait::async_trait;

use crate::base::Collection;
use crate::base::FetchedItem;
use crate::base::FetchedProperty;
use crate::base::Item;
use crate::base::Storage;
use crate::disco::Discovery;
use crate::watch::StorageMonitor;
use crate::CollectionId;
use crate::Href;
use crate::{ErrorKind, Etag, Result};

/// A wrapper around a [`Storage`] that disallows any write operations.
///
/// # Example
///
/// ```
/// # use vstorage::vdir::VdirStorage;
/// # use crate::vstorage::calendar::IcsItem;
/// # use camino::Utf8PathBuf;
/// # use vstorage::readonly::ReadOnlyStorage;
/// let orig = VdirStorage::<IcsItem>::new(
///     Utf8PathBuf::from("/path/to/storage/"),
///     String::from("ics"),
/// );
///
/// let read_only = ReadOnlyStorage::from(orig);
/// ```
pub struct ReadOnlyStorage<S: Storage<I>, I: Item> {
    inner: S,
    phantom: PhantomData<I>,
}

#[async_trait]
impl<S: Storage<I>, I: Item> Storage<I> for ReadOnlyStorage<S, I> {
    async fn check(&self) -> Result<()> {
        self.inner.check().await
    }

    async fn discover_collections(&self) -> Result<Discovery> {
        self.inner.discover_collections().await
    }

    async fn create_collection(&self, _href: &str) -> Result<Collection> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn destroy_collection(&self, _href: &str) -> Result<()> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn list_items(&self, collection_href: &str) -> Result<Vec<crate::base::ItemRef>> {
        self.inner.list_items(collection_href).await
    }

    async fn get_item(&self, href: &str) -> Result<(I, Etag)> {
        self.inner.get_item(href).await
    }

    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem<I>>> {
        self.inner.get_many_items(hrefs).await
    }

    async fn get_all_items(&self, collection_href: &str) -> Result<Vec<FetchedItem<I>>> {
        self.inner.get_all_items(collection_href).await
    }

    async fn add_item(&self, _: &str, _: &I) -> Result<crate::base::ItemRef> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn update_item(&self, _: &str, _: &Etag, _: &I) -> Result<Etag> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn set_property(&self, _: &str, _: I::Property, _: &str) -> Result<()> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn unset_property(&self, _: &str, _: I::Property) -> Result<()> {
        Err(ErrorKind::ReadOnly.into())
    }

    async fn get_property(&self, href: &str, meta: I::Property) -> Result<Option<String>> {
        self.inner.get_property(href, meta).await
    }

    async fn delete_item(&self, _: &str, _: &Etag) -> Result<()> {
        Err(ErrorKind::ReadOnly.into())
    }

    fn href_for_collection_id(&self, id: &CollectionId) -> Result<Href> {
        self.inner.href_for_collection_id(id)
    }

    async fn list_properties(
        &self,
        collection_href: &str,
    ) -> Result<Vec<FetchedProperty<I::Property>>> {
        self.inner.list_properties(collection_href).await
    }

    async fn monitor(&self, interval: Duration) -> Result<Box<dyn StorageMonitor>> {
        self.inner.monitor(interval).await
    }
}

impl<S: Storage<I>, I: Item> From<S> for ReadOnlyStorage<S, I> {
    fn from(value: S) -> Self {
        Self {
            inner: value,
            phantom: PhantomData,
        }
    }
}
