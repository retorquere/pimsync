// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Plan for a synchronisation.

use std::collections::HashSet;
use std::sync::Arc;

use log::debug;

use crate::base::{FetchedItem, ItemRef, Storage};
use crate::disco::{DiscoveredCollection, Discovery};
use crate::{base::Item, sync::declare::StoragePair};
use crate::{CollectionId, Error, ErrorKind, Etag, Href};

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
///
/// Performs no I/O; only operates on input data.
async fn create_mappings_for_pair<I: Item>(
    pair: &StoragePair<I>,
) -> Result<Vec<ResolvedMapping>, PlanError> {
    let mut mappings = Vec::<ResolvedMapping>::with_capacity(pair.mappings.len());

    // FIXME: discovery is not required if all collections are defined by href.
    //        (and discovery may not even be available in such cases)
    //
    // wrapping in a huge IF won't work due to inner signatures.
    // the best approach is likely to have something like a OnceCell for discovery data, so it is
    // fetched when first required.
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
            .map_err(PlanError::BadCollectionMappings)?,
        );
    }

    if pair.all_from_a {
        mappings.reserve(disco_a.collection_count());
        for collection in disco_a.collections() {
            mappings.push(ResolvedMapping {
                a: ResolvedCollection::Href {
                    href: collection.href().to_string(),
                },
                b: resolve_mapping_counterpart(collection, &disco_b),
            });
        }
    }
    if pair.all_from_b {
        mappings.reserve(disco_b.collection_count());
        for collection in disco_b.collections() {
            let mapping = ResolvedMapping {
                a: resolve_mapping_counterpart(collection, &disco_a),
                b: ResolvedCollection::Href {
                    href: collection.href().to_owned(),
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
    // TODO: An alias attribute?
    a: ResolvedCollection,
    b: ResolvedCollection,
}

impl ResolvedMapping {
    pub(super) fn collection_a(&self) -> &ResolvedCollection {
        &self.a
    }

    pub(super) fn collection_b(&self) -> &ResolvedCollection {
        &self.b
    }

    fn href(&self, side: Side) -> Option<&str> {
        match side {
            Side::A => &self.a,
            Side::B => &self.b,
        }
        .href()
    }

    /// Returns `Err` if the collection is missing on the `From` side.
    fn from_declared_mapping<I: Item>(
        declared: &DeclaredMapping,
        storage_a: &dyn Storage<I>,
        storage_b: &dyn Storage<I>,
        discovery_a: &Discovery,
        discovery_b: &Discovery,
    ) -> Result<Self, crate::Error> {
        match declared {
            DeclaredMapping::Direct { description } => Ok(ResolvedMapping {
                a: ResolvedCollection::from_declared_collection(description, discovery_a),
                b: ResolvedCollection::from_declared_collection(description, discovery_b),
            }),
            DeclaredMapping::FromA { description } => {
                resolve_from_x(description, discovery_a, storage_a, discovery_b)
                    // Note the order of arguments here.
                    .map(|(a, b)| ResolvedMapping { a, b })
            }
            DeclaredMapping::FromB { description } => {
                resolve_from_x(description, discovery_b, storage_b, discovery_a)
                    // Note the order of arguments here.
                    .map(|(b, a)| ResolvedMapping { a, b })
            }
            DeclaredMapping::Mapped { a, b, .. } => Ok(ResolvedMapping {
                a: ResolvedCollection::from_declared_collection(a, discovery_a),
                b: ResolvedCollection::from_declared_collection(b, discovery_b),
            }),
        }
    }
}

/// A collection as resolved based on existing data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedCollection {
    /// The collection does not exist; only its (expected) collection id is known.
    Id { id: CollectionId },
    /// The collection exists and its href is known.
    Href { href: String },
}

impl ResolvedCollection {
    /// Resolve the collection based on a storage and its collections.
    fn from_declared_collection(
        declared: &CollectionDescription,
        discovery: &Discovery,
    ) -> ResolvedCollection {
        match declared {
            CollectionDescription::Id { id } => match discovery.find_collection_by_id(id) {
                Some(collection) => ResolvedCollection::Href {
                    href: collection.href().to_string(),
                },
                None => ResolvedCollection::Id { id: id.clone() },
            },
            CollectionDescription::Href { href } => ResolvedCollection::Href {
                href: href.to_string(),
            },
        }
    }

    fn href(&self) -> Option<&str> {
        match self {
            ResolvedCollection::Id { .. } => None,
            ResolvedCollection::Href { href } => Some(href),
        }
    }
}

impl std::fmt::Display for ResolvedCollection {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolvedCollection::Id { id } => write!(fmt, "id: {id}"),
            ResolvedCollection::Href { href } => write!(fmt, "href: {href}"),
        }
    }
}

/// Finds a counterpart for a collection matching by id.
fn resolve_mapping_counterpart(
    source_collection: &DiscoveredCollection,
    target_discovery: &Discovery,
) -> ResolvedCollection {
    let id = source_collection.id();
    match target_discovery.find_collection_by_id(id) {
        Some(c) => ResolvedCollection::Href {
            href: c.href().to_string(),
        },
        None => ResolvedCollection::Id { id: id.clone() },
    }
}

/// Resolve a `FromX` mapping (e.g.: `FromA` or `FromB`).
///
/// The counterpart will be a collection with the same `CollectionId` on the other storage.
fn resolve_from_x<I: Item>(
    description: &CollectionDescription,
    discovery_x: &Discovery,
    storage_x: &dyn Storage<I>,
    discovery_y: &Discovery,
) -> Result<(ResolvedCollection, ResolvedCollection), crate::Error> {
    let (id, href) = match description {
        CollectionDescription::Id { id } => {
            let collection = discovery_x.find_collection_by_id(id).ok_or(Error::new(
                ErrorKind::DoesNotExist,
                format!("No collection with id: {id}"),
            ))?;
            (id.clone(), collection.href().to_string())
        }
        CollectionDescription::Href { href } => {
            let id = storage_x.collection_id(href)?;
            (id.clone(), href.to_string())
        }
    };

    let counterpart = match discovery_y.find_collection_by_id(&id) {
        Some(c) => ResolvedCollection::Href {
            href: c.href().to_string(),
        },
        None => ResolvedCollection::Id { id },
    };

    Ok((ResolvedCollection::Href { href }, counterpart))
}

#[derive(Debug)]
pub struct ItemAction {
    uid: String,
    action: Action,
}

impl ItemAction {
    pub(super) fn uid(&self) -> &str {
        &self.uid
    }
    pub(super) fn action(&self) -> &Action {
        &self.action
    }
}

/// A set of actions required to sync a collection between two storages.
#[derive(Debug)]
pub(super) struct CollectionPlan {
    mapping: ResolvedMapping,
    collection_action: Option<CollectionAction>,
    items: Vec<ItemAction>,
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
        // Collections declared by Id are already resolved to an Href at this point.
        // Those not resolved don't exist.
        let state_a = CollectionState::new(status, pair.storage_a(), &mapping, Side::A).await?;
        let state_b = CollectionState::new(status, pair.storage_b(), &mapping, Side::B).await?;

        let status_items = status.map_or(Ok(Vec::new()), StatusDatabase::all_uids)?;
        let status_items = status_items.iter();
        let items_a = state_a.as_ref().map(|s| &s.items).into_iter().flatten();
        let items_b = state_b.as_ref().map(|s| &s.items).into_iter().flatten();

        let all_items = items_a.chain(items_b).map(|i| &i.uid).chain(status_items);

        let item_actions = all_items
            .map(|uid| {
                let item_a = state_a.as_ref().and_then(|s| s.get_item_by_uid(uid));
                let item_b = state_b.as_ref().and_then(|s| s.get_item_by_uid(uid));

                let (prev_item_a, prev_item_b) = match status {
                    Some(s) => (
                        s.get_item_by_uid(Side::A, uid)?,
                        s.get_item_by_uid(Side::B, uid)?,
                    ),
                    None => (None, None),
                };

                let a_changed = Change::for_item(item_a, prev_item_a);
                let b_changed = Change::for_item(item_b, prev_item_b);

                if let Some(action) = Action::from_changes(a_changed, b_changed) {
                    Ok(Some(ItemAction {
                        uid: uid.to_string(),
                        action,
                    }))
                } else {
                    Ok(None)
                }
            })
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, PlanError>>()?;

        let collection_action = CollectionAction::new(&mapping, status, state_a, state_b)?;

        if collection_action.is_none() && item_actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CollectionPlan {
                mapping,
                collection_action,
                items: item_actions,
            }))
        }
    }

    pub(super) fn into_parts(self) -> (ResolvedMapping, Option<CollectionAction>, Vec<ItemAction>) {
        (self.mapping, self.collection_action, self.items)
    }
}

/// An action to executing when synchronising.
#[derive(PartialEq, Debug, Clone)]
pub enum Action {
    // Item is identical on both sides but are missing from state.
    // This mostly happens during the first run.
    SaveToState { a: ItemState, b: ItemState },
    // TODO: details on target collection should be included here.
    CreateInA { source: Href },
    CreateInB { source: Href },
    UpdateInA { source: Href, target: ItemRef },
    UpdateInB { source: Href, target: ItemRef },
    DeleteInA { target: ItemRef },
    DeleteInB { target: ItemRef },
    Conflict,
}

impl Action {
    /// Return the correct action given a pair of changes.
    ///
    /// `None` implies that no action needs to be taken.
    #[must_use]
    fn from_changes(left: Change, right: Change) -> Option<Action> {
        match (left, right) {
            (Change::Changed { state: state_a }, Change::Changed { state: state_b }) => {
                if state_a.hash == state_b.hash {
                    Some(Action::SaveToState {
                        a: state_a.clone(),
                        b: state_b.clone(),
                    })
                } else {
                    Some(Action::Conflict)
                }
            }
            (Change::NoChange { href, etag }, Change::Deleted { .. }) => Some(Action::DeleteInA {
                target: ItemRef {
                    href: href.clone(),
                    etag: etag.clone(),
                },
            }),
            (Change::Deleted { .. }, Change::NoChange { href, etag }) => Some(Action::DeleteInB {
                target: ItemRef {
                    href: href.clone(),
                    etag: etag.clone(),
                },
            }),
            // Copy new into A.
            (Change::Deleted { .. } | Change::Absent, Change::Changed { state }) => {
                Some(Action::CreateInA {
                    source: state.href.clone(),
                })
            }
            (Change::Absent, Change::NoChange { href, .. }) => Some(Action::CreateInA {
                source: href.clone(),
            }),
            // Copy and overwrite into A.
            (
                Change::NoChange {
                    etag,
                    href: target_href,
                },
                Change::Changed { state },
            ) => Some(Action::UpdateInA {
                source: state.href.clone(),
                target: ItemRef {
                    etag: etag.clone(),
                    href: target_href.clone(),
                },
            }),
            // Copy new into B.
            (Change::Changed { state }, Change::Deleted { .. } | Change::Absent) => {
                Some(Action::CreateInB {
                    source: state.href.clone(),
                })
            }
            (Change::NoChange { href, .. }, Change::Absent) => Some(Action::CreateInB {
                source: href.clone(),
            }),
            // Copy and overwrite into B.
            (
                Change::Changed { state },
                Change::NoChange {
                    etag,
                    href: target_href,
                },
            ) => Some(Action::UpdateInB {
                source: state.href.clone(),
                target: ItemRef {
                    etag: etag.clone(),
                    href: target_href.clone(),
                },
            }),
            // No-op
            (Change::Deleted { .. } | Change::Absent, Change::Deleted { .. } | Change::Absent)
            | (Change::NoChange { .. }, Change::NoChange { .. }) => None,
        }
    }
}

/// An action to executing on a collection when synchronising.
#[derive(PartialEq, Debug, Clone)]
pub enum CollectionAction {
    CreateInA { collection: ResolvedCollection },
    CreateInB { collection: ResolvedCollection },
    DeleteInA { href: Href },
    DeleteInB { href: Href },
}

impl CollectionAction {
    fn new(
        mapping: &ResolvedMapping,
        status: Option<&StatusDatabase>,
        current_a: Option<CollectionState>,
        current_b: Option<CollectionState>,
    ) -> Result<Option<CollectionAction>, StatusError> {
        // Test whether the collection previously existed.
        let (previous_a, previous_b) = match status {
            Some(s) => (
                s.collection_exists(&mapping.a)?,
                s.collection_exists(&mapping.b)?,
            ),
            None => (false, false),
        };

        let collection_action = match (current_a, current_b, previous_a, previous_b) {
            // Deleted on both sides OR exists on both sides.
            (None, None, _, _) | (Some(_), Some(_), _, _) => None,
            // New or present in B AND missing from A.
            (None, Some(_), _, false) | (None, Some(_), false, true) => {
                Some(CollectionAction::CreateInA {
                    collection: mapping.a.clone(),
                })
            }
            // Deleted from A.
            (None, Some(c), true, true) => Some(CollectionAction::DeleteInB { href: c.href }),
            // New or present in A AND missing from B.
            (Some(_), None, false, _) | (Some(_), None, true, false) => {
                Some(CollectionAction::CreateInB {
                    collection: mapping.b.clone(),
                })
            }
            // Deleted from B.
            (Some(c), None, true, true) => Some(CollectionAction::DeleteInA { href: c.href }),
        };
        Ok(collection_action)
    }
}

/// A transition that has occurred to a pair of items or collections.
#[derive(Debug, Clone)]
enum Change<'href> {
    /// Mutated or created.
    Changed { state: &'href ItemState },
    /// Deleted.
    Deleted,
    /// The item exists and has not changed.
    NoChange {
        href: &'href Href,
        etag: &'href Etag,
    },
    /// The item does not exist and did not exist before.
    ///
    /// This might indicate that this item was previously excluded from synchronisation.
    Absent,
}

impl<'href> Change<'href> {
    #[must_use]
    fn for_item(current: Option<&'href ItemState>, previous: Option<ItemState>) -> Change<'href> {
        // TODO: what happens when an item changes href but all else remains the same?
        match (current, previous) {
            (Some(c), Some(p)) => {
                if c.uid == p.uid && c.etag == p.etag && c.hash == p.hash {
                    Change::NoChange {
                        href: &c.href,
                        etag: &c.etag,
                    }
                } else {
                    Change::Changed { state: c }
                }
            }
            (Some(state), None) => Change::Changed { state },
            (None, Some(_)) => Change::Deleted,
            (None, None) => Change::Absent,
        }
    }
}

/// The state of a single collection.
#[derive(Clone, Debug)]
struct CollectionState {
    href: Href,
    items: Vec<ItemState>,
}

impl CollectionState {
    /// Returns `None` if the collection does not have an `href`.
    async fn new<I: Item>(
        status: Option<&StatusDatabase>,
        storage: &dyn Storage<I>,
        mapping: &ResolvedMapping,
        side: Side,
    ) -> Result<Option<CollectionState>, PlanError> {
        let Some(collection_href) = mapping.href(side) else {
            return Ok(None);
        };
        let mut items = Vec::new();

        let prefetched = if let Some(status) = status {
            let mut to_prefetch = Vec::new();

            for item_ref in storage.list_items(collection_href).await? {
                if let Some(prev_item) = status.get_item_by_href(side, &item_ref.href)? {
                    if prev_item.etag == item_ref.etag {
                        // The item has not changed, so its hash also remains the same.
                        // All data available; nothing to request.
                        items.push(ItemState {
                            href: item_ref.href,
                            etag: item_ref.etag,
                            uid: prev_item.uid,
                            hash: prev_item.hash,
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
        items.extend(prefetched);

        Ok(Some(CollectionState {
            href: collection_href.to_string(),
            items,
        }))
    }

    #[inline]
    pub fn get_item_by_uid(&self, uid: &str) -> Option<&ItemState> {
        self.items.iter().find(|i| i.uid == *uid)
    }
}
