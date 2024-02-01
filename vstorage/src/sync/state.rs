// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Models to represent the current state of a `Storage`.

use crate::{
    base::{FetchedItem, Item, Storage},
    Href, Result,
};

use super::status::{ItemState, Side, StatusDatabase};

/// Used internally to represent the current state of a storage during the planning phase.
#[derive(Clone, Default, Debug)]
#[allow(clippy::module_name_repetitions)] // This name would be ambiguous otherwise.
pub(super) struct StorageState {
    collections: Vec<CollectionState>,
}

impl StorageState {
    pub(super) async fn current<I: crate::base::Item>(
        status: Option<&StatusDatabase>,
        storage: &dyn Storage<I>,
        // The hrefs that we care about:
        collection_hrefs: &Vec<&str>,
        side: Side,
    ) -> Result<StorageState, Box<dyn std::error::Error>> {
        let mut collections = Vec::with_capacity(collection_hrefs.len());

        for href in collection_hrefs {
            let state = CollectionState::generate_current(status, storage, href, side).await?;
            collections.push(state);
        }

        Ok(StorageState { collections })
    }

    /// Returns the state of the collection with the given href.
    ///
    /// Returns `None` if the collection does not exist. This is distinct from the collection
    /// existing and being empty.
    #[must_use]
    #[inline]
    pub(super) fn find_collection_state(&self, href: &str) -> Option<&CollectionState> {
        self.collections.iter().find(|c| c.href == href)
    }
}

/// The state of a single collection.
#[derive(Clone, Debug)]
pub(super) struct CollectionState {
    pub(super) href: Href,
    pub(super) items: Vec<ItemState>,
}

impl CollectionState {
    async fn generate_current<I: Item>(
        status: Option<&StatusDatabase>,
        storage: &dyn Storage<I>,
        collection_href: &str,
        side: Side,
    ) -> Result<CollectionState, Box<dyn std::error::Error>> {
        let mut state = CollectionState {
            href: collection_href.to_string(),
            items: Vec::new(),
        };

        let prefetched = if let Some(status) = status {
            let mut to_prefetch = Vec::new();

            for item_ref in storage.list_items(collection_href).await? {
                if let Some(prev_item) = status.get_item_by_href(side, &item_ref.href)? {
                    if prev_item.etag == item_ref.etag {
                        // The item has not changed, so its hash also remains the same.
                        // All data available; nothing to request.
                        state.items.push(ItemState {
                            href: item_ref.href,
                            etag: item_ref.etag,
                            uid: prev_item.uid.clone(),
                            hash: prev_item.hash.clone(),
                        });
                        continue;
                    } // else: item has changed
                } // else: item is new
                to_prefetch.push(item_ref.href);
            }

            let to_prefetch = to_prefetch.iter().map(String::as_str).collect::<Vec<_>>();
            storage.get_many_items(&to_prefetch).await?
        } else {
            storage.get_all_items(collection_href).await?
        };

        let prefetched = prefetched
            .into_iter()
            .map(|FetchedItem { href, item, etag }| ItemState {
                href,
                uid: item.ident(),
                etag,
                hash: item.hash(),
            });
        state.items.extend(prefetched);

        Ok(state)
    }

    #[inline]
    pub(super) fn get_item_by_uid(&self, uid: &str) -> Option<&ItemState> {
        self.items.iter().find(|i| i.uid == *uid)
    }
}
