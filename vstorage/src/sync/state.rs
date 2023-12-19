// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Models the state of a storage to track which side has mutated across runs.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    base::{FetchedItem, Item, Storage},
    disco::{DiscoveredCollection, Discovery},
    CollectionId, Etag, Href, Result,
};

use super::plan::ResolvedCollection;

/// The state of a pair at a specific point in time.
///
/// Generally, this should be treated as opaque data and not modified by consumers of this library.
/// It should, however, be serialised and saved into persistent storages between synchronisation
/// operations.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[allow(clippy::module_name_repetitions)] // This name would be ambiguous otherwise.
pub struct PairState {
    pub(super) a: StorageState,
    pub(super) b: StorageState,
}

/// The state of a storage at a specific point in time.
///
/// See [`PairState`].
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[allow(clippy::module_name_repetitions)] // This name would be ambiguous otherwise.
pub(super) struct StorageState {
    collections: Vec<CollectionState>,
}

impl StorageState {
    pub(super) async fn current_for_storage<I: crate::base::Item>(
        previous_state: Option<&StorageState>,
        storage: &Arc<dyn Storage<I>>,
        // The hrefs that we care about:
        collection_hrefs: &Vec<&str>,
        discovery: &Discovery,
    ) -> Result<StorageState> {
        let mut collections = Vec::with_capacity(collection_hrefs.len());

        for href in collection_hrefs {
            let Some(collection) = discovery.find_collection_by_href(href) else {
                // If a collection does not exist the there is no state for it.
                continue;
            };

            let previous = previous_state
                .as_ref()
                .and_then(|s| s.find_collection_state(href));
            let state = CollectionState::generate_current(previous, storage, collection).await;
            collections.push(state?);
        }

        Ok(StorageState { collections })
    }

    /// Returns the state of the collection with the given href.
    ///
    /// Returns `None` if the collection does not exist in this state (which is
    /// distinct from the collection existing and being empty).
    #[must_use]
    #[inline]
    pub(super) fn find_collection_state(&self, href: &str) -> Option<&CollectionState> {
        self.collections.iter().find(|c| c.href == href)
    }

    #[must_use]
    #[inline]
    pub(super) fn find_collection_state_mut(
        &mut self,
        rc: &ResolvedCollection,
    ) -> Option<&mut CollectionState> {
        match rc {
            ResolvedCollection::Id { id } => self.collections.iter_mut().find(|c| c.id == *id),
            ResolvedCollection::Href { href } => {
                self.collections.iter_mut().find(|c| c.href == *href)
            }
        }
    }

    pub(super) fn add_collection(&mut self, id: CollectionId, href: String) {
        self.collections.push({
            CollectionState {
                id,
                href,
                items: Vec::new(),
            }
        });
    }

    pub(super) fn remove_collection(&mut self, href: &str) {
        self.collections.retain(|c| c.href != href);
    }
}

/// The state of a single collection on a single storage at a specific point in time.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(super) struct CollectionState {
    // TODO: keep etag (to delete when empty).
    pub(super) href: Href,
    pub(super) id: CollectionId,
    pub(super) items: Vec<ItemState>,
}

impl CollectionState {
    async fn generate_current<I: Item>(
        previous_state: Option<&CollectionState>,
        storage: &Arc<dyn Storage<I>>,
        collection: &DiscoveredCollection,
    ) -> crate::Result<Self> {
        let mut state = CollectionState {
            id: collection.id().clone(),
            href: collection.href().to_string(),
            items: Vec::new(),
        };
        let mut to_prefetch = Vec::new();

        // TODO: I could special case if previous_state is None and just get_all

        for item_ref in storage.list_items(&collection.to_collection()).await? {
            if let Some(ps) = previous_state {
                if let Some(p) = ps.get_item_by_href(&item_ref.href) {
                    if p.etag == item_ref.etag {
                        state.items.push(ItemState {
                            href: item_ref.href,
                            etag: item_ref.etag,
                            uid: p.uid.clone(),
                            hash: p.hash.clone(),
                        });
                        continue;
                    }
                }
            }

            to_prefetch.push(item_ref.href);
        }
        let to_prefetch = to_prefetch.iter().map(String::as_str).collect::<Vec<_>>();
        let prefetched = storage.get_many_items(&to_prefetch).await?.into_iter().map(
            |FetchedItem { href, item, etag }| ItemState {
                href,
                uid: item.ident(),
                etag,
                hash: item.hash(),
            },
        );
        state.items.extend(prefetched);

        Ok(state)
    }

    #[inline]
    pub(super) fn get_item_by_href(&self, href: &str) -> Option<&ItemState> {
        self.items.iter().find(|i| i.href == *href)
    }

    #[inline]
    pub(super) fn get_item_by_uid(&self, uid: &str) -> Option<&ItemState> {
        self.items.iter().find(|i| i.uid == *uid)
    }

    #[inline]
    pub(super) fn get_item_by_uid_mut(&mut self, uid: &str) -> Option<&mut ItemState> {
        self.items.iter_mut().find(|i| i.uid == *uid)
    }
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
pub(super) struct ItemState {
    pub(super) href: Href,
    pub(super) uid: String,
    pub(super) etag: Etag, // TODO: optional?
    pub(super) hash: String,
}
