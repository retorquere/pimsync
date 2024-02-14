// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Plan for a synchronisation.

use std::collections::HashSet;
use std::sync::Arc;

use log::{debug, warn};

use crate::base::{FetchedItem, ItemRef, Storage};
use crate::disco::{DiscoveredCollection, Discovery};
use crate::{base::Item, sync::declare::StoragePair};
use crate::{CollectionId, ErrorKind, Href};

use super::declare::{CollectionDescription, DeclaredMapping};
use super::status::{ItemState, Side, StatusDatabase, StatusError};
use super::PlanError;

/// A series of actions that would synchronise a pair of storages.
pub struct Plan<I: Item> {
    pub(super) storage_a: Arc<dyn Storage<I>>,
    pub(super) storage_b: Arc<dyn Storage<I>>,
    pub(super) collection_plans: Vec<CollectionPlan>,
}

/// Show only details of the plan itself; ignore other data.
///
/// This is partially necessary because storages might not implement `Debug`.
impl<I: Item> std::fmt::Debug for Plan<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.collection_plans, f)
    }
}

impl<I: Item> Plan<I> {
    /// Create a new plan for a given storage pair.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// - There is an error discovering remote collections.
    /// - A mapping is defined by collection id, but the id is invalid for the underlying storage.
    /// - There is an error reading the state of existing items.
    /// - The same collection is mapped more than once.
    pub async fn new(
        pair: &StoragePair<I>,
        status: Option<&StatusDatabase>,
    ) -> Result<Plan<I>, PlanError> {
        let mappings = create_mappings_for_pair(pair).await?;

        let mut seen_a = HashSet::<&ResolvedCollection>::new();
        let mut seen_b = HashSet::<&ResolvedCollection>::new();

        for mapping in &mappings {
            // TODO: cloning here is not ideal, but it's not a hot path either.
            if seen_a.contains(&mapping.a) {
                // TODO: only fail if B is different
                return Err(PlanError::DuplicateCollectionInA(mapping.a.clone()));
            }
            if seen_b.contains(&mapping.b) {
                // TODO: only fail if A is different
                return Err(PlanError::DuplicateCollectionInB(mapping.b.clone()));
            }

            seen_a.insert(&mapping.a);
            seen_b.insert(&mapping.a);
        }
        drop(seen_a);
        drop(seen_b);

        let mut collection_plans = Vec::new();
        for m in mappings {
            if let Some(plan) = CollectionPlan::new(pair, m, status).await? {
                collection_plans.push(plan);
            }
        }

        Ok(Plan {
            storage_a: pair.storage_a.clone(),
            storage_b: pair.storage_b.clone(),
            collection_plans,
        })
    }

    #[must_use]
    pub fn storage_a(&self) -> &dyn Storage<I> {
        self.storage_a.as_ref()
    }

    #[must_use]
    pub fn storage_b(&self) -> &dyn Storage<I> {
        self.storage_b.as_ref()
    }
}

/// Resolve all collection mappings for a given pair.
async fn create_mappings_for_pair<I: Item>(
    pair: &StoragePair<I>,
) -> Result<Vec<ResolvedMapping>, PlanError> {
    let mut mappings = Vec::<ResolvedMapping>::with_capacity(pair.mappings.len());

    let disco_a = pair
        .storage_a
        .discover_collections()
        .await
        .map_err(PlanError::DiscoveryFailedA)?;
    let disco_b = pair
        .storage_b
        .discover_collections()
        .await
        .map_err(PlanError::DiscoveryFailedB)?;

    for mapping in &pair.mappings {
        mappings.push(
            ResolvedMapping::from_declared_mapping(
                mapping,
                pair.storage_a.as_ref(),
                pair.storage_b.as_ref(),
                &disco_a,
                &disco_b,
            )
            .await
            .map_err(PlanError::BadCollectionMappings)?,
        );
    }

    if pair.all_from_a {
        mappings.reserve(disco_a.collection_count());
        for collection in disco_a.collections() {
            mappings.push(ResolvedMapping {
                alias: format!("id:{}", collection.id()),
                a: ResolvedCollection {
                    href: collection.href().to_string(),
                    id: Some(collection.id().clone()),
                    exists: true,
                },
                b: resolve_mapping_counterpart(collection, &disco_b, pair.storage_b())?,
            });
        }
    }
    if pair.all_from_b {
        mappings.reserve(disco_b.collection_count());
        for collection in disco_b.collections() {
            let mapping = ResolvedMapping {
                alias: format!("id:{}", collection.id()),
                a: resolve_mapping_counterpart(collection, &disco_a, pair.storage_a())?,
                b: ResolvedCollection {
                    href: collection.href().to_string(),
                    id: Some(collection.id().clone()),
                    exists: true,
                },
            };
            if mappings.iter().any(|m| *m == mapping) {
                debug!("Skipping mapping; already present.");
            } else {
                mappings.push(mapping);
            }
        }
    }
    Ok(mappings)
}

#[cfg(test)]
mod test {
    use std::{str::FromStr, sync::Arc};

    use tempfile::Builder;

    use crate::{
        base::IcsItem,
        filesystem::FilesystemStorage,
        sync::{
            declare::{DeclaredMapping, StoragePair},
            plan::Plan,
        },
        CollectionId,
    };

    #[tokio::test]
    async fn test_discovery_of_duplicate_mappings() {
        let dir_a = Builder::new().prefix("vstorage").tempdir().unwrap();
        let dir_b = Builder::new().prefix("vstorage").tempdir().unwrap();

        let storage_a = Arc::new(FilesystemStorage::<IcsItem>::new(
            dir_a.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));
        let storage_b = Arc::from(FilesystemStorage::<IcsItem>::new(
            dir_b.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));

        {
            // This sync would be a no-op, but it's not "wrong".
            let mut pair = StoragePair::builder(storage_a.clone(), storage_b.clone()).build();
            assert!(Plan::new(&mut pair, None).await.is_ok());
        }
        {
            // This sync is okay.
            let collection = CollectionId::from_str("test").unwrap();
            let mut pair = StoragePair::builder(storage_a.clone(), storage_b.clone())
                .with_mapping(DeclaredMapping::direct(collection))
                .build();
            assert!(Plan::new(&mut pair, None).await.is_ok());
        }
        {
            // This sync has duplicate items.
            let collection = CollectionId::from_str("test").unwrap();
            let mut pair = StoragePair::builder(storage_a, storage_b)
                .with_mapping(DeclaredMapping::direct(collection.clone()))
                .with_mapping(DeclaredMapping::direct(collection))
                .build();
            assert!(Plan::new(&mut pair, None).await.is_err());
        }
    }
}

/// A mapping resolved based on the storage's current state.
///
/// A `ResolvedCollection::Id` variant implies that a collection does not exist on that side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMapping {
    alias: String,
    a: ResolvedCollection,
    b: ResolvedCollection,
}

impl ResolvedMapping {
    pub(super) fn collection(&self, side: Side) -> &ResolvedCollection {
        match side {
            Side::A => &self.a,
            Side::B => &self.b,
        }
    }

    async fn from_declared_mapping<I: Item>(
        declared: &DeclaredMapping,
        storage_a: &dyn Storage<I>,
        storage_b: &dyn Storage<I>,
        disco_a: &Discovery,
        disco_b: &Discovery,
    ) -> Result<Self, crate::Error> {
        let alias = declared.alias();
        match declared {
            DeclaredMapping::Direct { description } => Ok(ResolvedMapping {
                alias,
                a: ResolvedCollection::from_declared_collection(description, disco_a, storage_a)
                    .await?,
                b: ResolvedCollection::from_declared_collection(description, disco_b, storage_b)
                    .await?,
            }),
            DeclaredMapping::Mapped { a, b, .. } => Ok(ResolvedMapping {
                alias,
                a: ResolvedCollection::from_declared_collection(a, disco_a, storage_a).await?,
                b: ResolvedCollection::from_declared_collection(b, disco_b, storage_b).await?,
            }),
        }
    }
}

/// A collection as resolved based on existing data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedCollection {
    pub(super) id: Option<CollectionId>,
    pub(super) href: Href,
    exists: bool,
    // discoverable?
}

impl ResolvedCollection {
    /// Resolve the collection based on a storage and its collections.
    async fn from_declared_collection<I: Item>(
        declared: &CollectionDescription,
        discovery: &Discovery,
        storage: &dyn Storage<I>,
    ) -> Result<ResolvedCollection, crate::Error> {
        match declared {
            CollectionDescription::Id { id } => {
                if let Some(collection) = discovery.find_collection_by_id(id) {
                    Ok(ResolvedCollection {
                        id: Some(id.clone()),
                        href: collection.href().to_string(),
                        exists: true,
                    })
                } else {
                    let href = storage.href_for_collection_id(id)?;
                    let exists = storage_exists(storage, &href).await?;

                    Ok(ResolvedCollection {
                        id: Some(id.clone()),
                        href,
                        exists, // TODO: won't this always be false?
                    })
                }
            }
            CollectionDescription::Href { href } => {
                let id = discovery
                    .collections()
                    .iter()
                    .find(|c| c.href() == *href)
                    .map(|c| c.id().clone());
                let exists = id.is_some() || storage_exists(storage, href).await?;
                Ok(ResolvedCollection {
                    href: href.clone(),
                    id,
                    exists,
                })
            }
        }
    }

    pub(super) fn href(&self) -> &str {
        &self.href
    }
}

async fn storage_exists<I: Item>(
    storage: &dyn Storage<I>,
    href: &str,
) -> Result<bool, crate::Error> {
    // FIXME: Listing items is a bit heavyweight.
    //        I need a separate method that just checks this (e.g.: HTTP HEAD).
    match storage.list_items(href).await {
        Ok(_) => Ok(true),
        Err(e) => {
            if e.kind == ErrorKind::DoesNotExist || e.kind == ErrorKind::AccessDenied {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

impl std::fmt::Display for ResolvedCollection {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            fmt,
            "id: {:?}, href: {}, exists: {}",
            self.id, self.href, self.exists
        )
    }
}

/// Finds a counterpart for a collection matching by id.
fn resolve_mapping_counterpart<I: Item>(
    source_collection: &DiscoveredCollection,
    target_discovery: &Discovery,
    target_storage: &dyn Storage<I>,
) -> Result<ResolvedCollection, PlanError> {
    let id = source_collection.id();
    match target_discovery.find_collection_by_id(id) {
        Some(c) => Ok(ResolvedCollection {
            href: c.href().to_string(),
            id: Some(id.clone()),
            exists: true,
        }),
        None => Ok(ResolvedCollection {
            id: Some(id.clone()),
            href: target_storage.href_for_collection_id(id)?,
            exists: false, //FIXME: are you sure?
        }),
    }
}

/// A set of actions required to sync a collection between two storages.
#[derive(Debug)]
pub(super) struct CollectionPlan {
    pub(super) collection_action: Option<CollectionAction>,
    pub(super) item_actions: Vec<ItemAction>,
    pub(super) href_a: Href,
    pub(super) href_b: Href,
    pub(super) id_a: Option<CollectionId>,
    pub(super) id_b: Option<CollectionId>,
}

impl CollectionPlan {
    /// Calculate actions to sync a collection between two storages.
    ///
    /// Returns `None` if this plan would be a no-op.
    async fn new<I: Item>(
        pair: &StoragePair<I>,
        mapping: ResolvedMapping,
        status: Option<&StatusDatabase>,
    ) -> Result<Option<CollectionPlan>, PlanError> {
        let items_a =
            item_for_collection(status, pair.storage_a(), &mapping.a.href, Side::A).await?;
        let items_b =
            item_for_collection(status, pair.storage_b(), &mapping.b.href, Side::B).await?;

        let status_uids = status.map_or(Ok(Vec::new()), |s| s.all_uids(&mapping))?;
        let status_uids = status_uids.iter();
        let uids_a = items_a.iter();
        let uids_b = items_b.iter();

        let all_uids = uids_a.chain(uids_b).map(|i| &i.uid).chain(status_uids);

        let item_actions = all_uids
            .map(|uid| {
                let item_a = items_a.iter().find(|i| i.uid == *uid);
                let item_b = items_b.iter().find(|i| i.uid == *uid);

                let (prev_a, prev_b) = match status {
                    Some(s) => (
                        get_item_by_uid(s, Side::A, &mapping, uid)?,
                        get_item_by_uid(s, Side::B, &mapping, uid)?,
                    ),
                    None => (None, None),
                };

                Ok(ItemAction::for_item(item_a, prev_a, item_b, prev_b, uid))
            })
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, PlanError>>()?;

        let collection_action =
            CollectionAction::new(&mapping, status, mapping.a.exists, mapping.b.exists)?;

        if collection_action.is_none() && item_actions.is_empty() {
            return Ok(None);
        }

        Ok(Some(CollectionPlan {
            collection_action,
            item_actions,
            id_a: mapping.a.id,
            href_a: mapping.a.href,
            id_b: mapping.b.id,
            href_b: mapping.b.href,
        }))
    }
}

fn get_item_by_uid(
    status: &StatusDatabase,
    side: Side,
    mapping: &ResolvedMapping,
    uid: &str,
) -> Result<Option<ItemState>, StatusError> {
    let collection_href = mapping.collection(side).href();
    status.get_item_by_uid(side, collection_href, uid)
}

/// An action to executing when synchronising.
#[derive(PartialEq, Debug, Clone)]
pub enum ItemAction {
    // Item is identical on both sides but are missing from state.
    // This mostly happens during the first run.
    SaveToState { a: ItemState, b: ItemState },
    // State is stale and item is gone on both sides.
    ClearState { uid: String },
    // TODO: details on target collection should be included here.
    CreateInA { source: ItemState },
    CreateInB { source: ItemState },
    UpdateInA { source: ItemState, target: ItemRef },
    UpdateInB { source: ItemState, target: ItemRef },
    DeleteInA { target: ItemState },
    DeleteInB { target: ItemState },
    Conflict { uid: String },
}

impl ItemAction {
    #[must_use]
    #[allow(clippy::match_same_arms)] // Merging branches hurts readability here.
    #[allow(clippy::too_many_lines)]
    fn for_item(
        current_a: Option<&ItemState>,
        previous_a: Option<ItemState>,
        current_b: Option<&ItemState>,
        previous_b: Option<ItemState>,
        uid: &str,
    ) -> Option<ItemAction> {
        match (current_a, previous_a, current_b, previous_b) {
            (None, _, None, _) => Some(ItemAction::ClearState {
                uid: uid.to_string(),
            }),
            (None, None, Some(b), _) => Some(ItemAction::CreateInA { source: b.clone() }),
            (None, Some(_), Some(b), None) => Some(ItemAction::CreateInA { source: b.clone() }),
            (None, Some(ap), Some(bc), Some(bp)) => {
                if ap.hash == bp.hash {
                    // Item used to be in sync
                    if bc.hash == bp.hash {
                        // B is unchanged
                        Some(ItemAction::DeleteInB { target: bc.clone() })
                    } else {
                        warn!("Item deleted in A but changed B: {}.", bc.uid);
                        Some(ItemAction::CreateInA { source: bc.clone() })
                    }
                } else {
                    // Item used to be in conflict
                    Some(ItemAction::CreateInA { source: bc.clone() })
                }
            }
            (Some(a), None, None, _) => Some(ItemAction::CreateInB { source: a.clone() }),
            (Some(a), Some(_), None, None) => Some(ItemAction::CreateInB { source: a.clone() }),
            (Some(ac), Some(ap), None, Some(bp)) => {
                if ap.hash == bp.hash {
                    // Item used to be in sync
                    if ac.hash == ap.hash {
                        // A is unchanged
                        Some(ItemAction::DeleteInA { target: ac.clone() })
                    } else {
                        warn!("Item deleted in B but changed A: {}.", ac.uid);
                        Some(ItemAction::CreateInB { source: ac.clone() })
                    }
                } else {
                    Some(ItemAction::CreateInB { source: ac.clone() })
                }
            }
            (Some(ac), Some(ap), Some(bc), Some(bp)) => {
                if ac.hash == bc.hash {
                    // Item are in sync
                    if ac.hash == ap.hash && ap.hash == bp.hash {
                        // Status is up to date
                        None
                    } else {
                        Some(ItemAction::SaveToState {
                            a: ac.clone(),
                            b: bc.clone(),
                        })
                    }
                } else if ac.hash == ap.hash {
                    // Side A has not changed
                    Some(ItemAction::UpdateInA {
                        source: bc.clone(),
                        target: ac.to_item_ref(),
                    })
                } else if bc.hash == bp.hash {
                    // Side A has not changed
                    Some(ItemAction::UpdateInB {
                        source: ac.clone(),
                        target: bc.to_item_ref(),
                    })
                } else {
                    // Both sides have changed
                    Some(ItemAction::Conflict {
                        uid: uid.to_string(),
                    })
                }
            }
            (Some(a), _, Some(b), _) => {
                // This is a fall through: (Some, Some, Some, Some) MUST be above this.
                // This covers the three cases where previous is None.
                if a.hash == b.hash {
                    Some(ItemAction::SaveToState {
                        a: a.clone(),
                        b: b.clone(),
                    })
                } else {
                    Some(ItemAction::Conflict {
                        uid: uid.to_string(),
                    })
                }
            }
        }
    }
}

/// An action to executing on a collection when synchronising.
#[derive(PartialEq, Debug, Clone)]
pub enum CollectionAction {
    SaveToStatus,
    CreateInA,
    CreateInB,
    CreateInBoth,
    DeleteInA,
    DeleteInB,
}

impl CollectionAction {
    fn new(
        mapping: &ResolvedMapping,
        status: Option<&StatusDatabase>,
        current_a: bool,
        current_b: bool,
    ) -> Result<Option<CollectionAction>, PlanError> {
        // Test whether the collection previously existed.
        let (previous_a, previous_b) = match status {
            Some(s) => (
                // TODO: replace `get_collection_href` with `collection_exists`
                s.get_collection_href(&mapping.a, Side::A)?.is_some(),
                s.get_collection_href(&mapping.b, Side::B)?.is_some(),
            ),
            None => (false, false),
        };

        // Note on collection deletion
        //
        // Collections should only be deleted if they were found via discovery and discovery is
        // still enabled. If it was explicitly removed from the configuration, it should remain.
        //
        // Right now we're operating on:
        // - explicitly configured collections
        // - discovered collections
        //
        // So if a collection reaches this point, it was found under one of those circumstances.
        // Collections that are
        let collection_action = match (current_a, current_b, previous_a, previous_b) {
            // Deleted or missing on both sides
            (false, false, _, _) => Some(CollectionAction::CreateInBoth),
            // New on both sides
            (true, true, false, false) => Some(CollectionAction::SaveToStatus),
            // Previously seen on one side. Bad input.
            (true, true, true, false) | (true, true, false, true) => {
                unreachable!("collection recorded as existing on only one side")
            }
            // No change.
            (true, true, true, true) => None,
            // New or present in B AND missing from A.
            (false, true, _, false) | (false, true, false, true) => {
                Some(CollectionAction::CreateInA)
            }
            // Deleted from A.
            (false, true, true, true) => Some(CollectionAction::DeleteInB),
            // New or present in A AND missing from B.
            (true, false, false, _) | (true, false, true, false) => {
                Some(CollectionAction::CreateInB)
            }
            // Deleted from B.
            (true, false, true, true) => Some(CollectionAction::DeleteInA),
        };
        Ok(collection_action)
    }
}

/// Returns the state of all items for a collection.
async fn item_for_collection<I: Item>(
    status: Option<&StatusDatabase>,
    storage: &dyn Storage<I>,
    collection_href: &str,
    side: Side,
) -> Result<Vec<ItemState>, PlanError> {
    debug!("Resolving state for collection: {}.", collection_href);
    let mut items = Vec::new();

    let prefetched = if let Some(status) = status {
        let mut to_prefetch = Vec::new();

        for item_ref in storage.list_items(collection_href).await? {
            if let Some(prev_item) = status.get_item_by_href(side, &item_ref.href)? {
                if prev_item.etag == item_ref.etag {
                    // Item has not changed; nothing to fetch.
                    items.push(prev_item);
                    continue;
                } // else: item has changed
            } // else: item is new
            to_prefetch.push(item_ref.href);
        }

        let to_prefetch = to_prefetch.iter().map(String::as_str).collect::<Vec<_>>();
        storage.get_many_items(&to_prefetch).await?
    } else {
        match storage.get_all_items(collection_href).await {
            Ok(items) => items,
            Err(err) if err.kind == ErrorKind::DoesNotExist => Vec::new(),
            Err(err) => return Err(PlanError::from(err)),
        }
    };

    let prefetched = prefetched
        .into_iter()
        .map(|FetchedItem { href, item, etag }| ItemState {
            href,
            uid: item.ident(),
            etag,
            hash: item.hash(),
        });
    items.extend(prefetched);

    Ok(items)
}
