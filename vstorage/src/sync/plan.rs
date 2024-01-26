// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Plan for a synchronisation.

use std::collections::HashSet;

use log::{debug, trace};

use crate::base::{ItemRef, Storage};
use crate::disco::{DiscoveredCollection, Discovery};
use crate::sync::state::StorageState;
use crate::{base::Item, sync::declare::StoragePair};
use crate::{CollectionId, Error, ErrorKind, Etag, Href};

use super::declare::{CollectionDescription, DeclaredMapping};
use super::state::{CollectionState, ItemState, PairState};
use super::PlanError;

/// A series of actions that would synchronise a pair of storages.
pub struct Plan<'pair, I: Item> {
    pub(super) pair: &'pair StoragePair<I>,
    pub(super) collection_plans: Vec<CollectionPlan>,
    current_state: PairState,
}

/// Show only details of the plan itself; ignore other data.
///
/// This is partially necessary because storages might not implement `Debug`.
impl<'pair, I: Item> std::fmt::Debug for Plan<'pair, I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.collection_plans, f)
    }
}

impl<'pair, I: Item> Plan<'pair, I> {
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
        pair: &'pair StoragePair<I>,
        previous_state: Option<&PairState>,
    ) -> Result<Plan<'pair, I>, PlanError> {
        // TODO: disco needs to returns its own error type?
        // TODO: only discover collections if any are specified by Id or All
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

        let mappings = create_mappings_for_pair(pair, &disco_a, &disco_b)
            .map_err(PlanError::BadCollectionMappings)?;

        {
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
        }

        let hrefs_a = mappings
            .iter()
            .filter_map(ResolvedMapping::href_a)
            .collect();
        let hrefs_b = mappings
            .iter()
            .filter_map(ResolvedMapping::href_b)
            .collect();

        let (prev_a, prev_b) = match previous_state {
            Some(prev) => (Some(&prev.a), Some(&prev.b)),
            None => (None, None),
        };

        let a =
            StorageState::current_for_storage(prev_a, pair.storage_a.as_ref(), &hrefs_a, &disco_a)
                .await
                .map_err(PlanError::StateA)?;
        let b =
            StorageState::current_for_storage(prev_b, pair.storage_b.as_ref(), &hrefs_b, &disco_b)
                .await
                .map_err(PlanError::StateA)?;

        let collection_plans = mappings
            .iter()
            .filter_map(|m| create_plan_for_mapping(m, &a, &b, prev_a, prev_b))
            .collect::<Vec<_>>();

        Ok(Plan {
            pair,
            collection_plans,
            current_state: PairState { a, b },
        })
    }

    /// Returns a reference to the underlying pair.
    #[must_use]
    pub fn pair(&self) -> &'pair StoragePair<I> {
        self.pair
    }

    /// The state of the pair, as resolved when creating this plan.
    #[must_use]
    pub fn current_state(&self) -> &PairState {
        &self.current_state
    }
}

/// Resolve all collection mappings for a given pair.
///
/// Performs no I/O; only operates on input data.
fn create_mappings_for_pair<I: Item>(
    pair: &StoragePair<I>,
    disco_a: &Discovery,
    disco_b: &Discovery,
) -> Result<Vec<ResolvedMapping>, crate::Error> {
    let mut mappings = Vec::<ResolvedMapping>::with_capacity(pair.mappings.len());
    for mapping in &pair.mappings {
        mappings.push(ResolvedMapping::from_declared_mapping(
            mapping.clone(),
            pair.storage_a.as_ref(),
            pair.storage_b.as_ref(),
            disco_a,
            disco_b,
        )?);
    }

    if pair.all_from_a {
        mappings.reserve(disco_a.collection_count());
        for collection in disco_a.collections() {
            mappings.push(ResolvedMapping {
                a: ResolvedCollection::Href {
                    href: collection.href().to_string(),
                },
                b: resolve_mapping_counterpart(collection, disco_b),
            });
        }
    }
    if pair.all_from_b {
        mappings.reserve(disco_b.collection_count());
        for collection in disco_b.collections() {
            let mapping = ResolvedMapping {
                a: resolve_mapping_counterpart(collection, disco_a),
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

/// Create plan for a collection mapping.
///
/// Performs no I/O; only operates on input data.
fn create_plan_for_mapping(
    mapping: &ResolvedMapping,
    current_a: &StorageState,
    current_b: &StorageState,
    previous_a: Option<&StorageState>,
    previous_b: Option<&StorageState>,
) -> Option<CollectionPlan> {
    let (cur_a, prev_a) = if let Some(href) = mapping.href_a() {
        (
            current_a.find_collection_state(href),
            previous_a.and_then(|s| s.find_collection_state(href)),
        )
    } else {
        (None, None)
    };
    let (cur_b, prev_b) = if let Some(href) = mapping.href_b() {
        (
            current_b.find_collection_state(href),
            previous_b.and_then(|s| s.find_collection_state(href)),
        )
    } else {
        (None, None)
    };

    CollectionPlan::new(mapping, prev_a, cur_a, prev_b, cur_b)
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

    pub(super) fn href_a(&self) -> Option<&str> {
        match &self.a {
            ResolvedCollection::Id { .. } => None,
            ResolvedCollection::Href { href } => Some(href),
        }
    }
    pub(super) fn href_b(&self) -> Option<&str> {
        match &self.b {
            ResolvedCollection::Id { .. } => None,
            ResolvedCollection::Href { href } => Some(href),
        }
    }

    /// Returns `Err` if the collection is missing on the `From` side.
    fn from_declared_mapping<I: Item>(
        declared: DeclaredMapping,
        storage_a: &dyn Storage<I>,
        storage_b: &dyn Storage<I>,
        discovery_a: &Discovery,
        discovery_b: &Discovery,
    ) -> Result<Self, crate::Error> {
        match declared {
            DeclaredMapping::Direct { description } => Ok(ResolvedMapping {
                a: ResolvedCollection::from_declared_collection(description.clone(), discovery_a),
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
        declared: CollectionDescription,
        discovery: &Discovery,
    ) -> ResolvedCollection {
        match declared {
            CollectionDescription::Id { id } => match discovery.find_collection_by_id(&id) {
                Some(collection) => ResolvedCollection::Href {
                    href: collection.href().to_string(),
                },
                None => ResolvedCollection::Id { id: id.clone() },
            },
            CollectionDescription::Href { href } => ResolvedCollection::Href { href },
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
    description: CollectionDescription,
    discovery_x: &Discovery,
    storage_x: &dyn Storage<I>,
    discovery_y: &Discovery,
) -> Result<(ResolvedCollection, ResolvedCollection), crate::Error> {
    let (id, href) = match description {
        CollectionDescription::Id { id } => {
            let collection = discovery_x.find_collection_by_id(&id).ok_or(Error::new(
                ErrorKind::DoesNotExist,
                format!("No collection with id: {id}"),
            ))?;
            (id, collection.href().to_string())
        }
        CollectionDescription::Href { href } => {
            let id = storage_x.collection_id(&href)?;
            (id, href.to_string())
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
    /// Each `previous_state` field shall be `None` if the collection did not previously exist.
    /// Each `current_state` field shall be `None` if the collection currently does not exist.
    ///
    /// Returns `None` if this plan would be a no-op.
    ///
    /// # Performance
    ///
    /// This methods is still quite inefficient. While the external API is not expected to change
    /// much, the internal implementation is not yet final.
    #[must_use]
    fn new<'a>(
        mapping: &ResolvedMapping,
        previous_state_a: Option<&'a CollectionState>,
        current_state_a: Option<&'a CollectionState>,
        previous_state_b: Option<&'a CollectionState>,
        current_state_b: Option<&'a CollectionState>,
    ) -> Option<CollectionPlan> {
        let all_items = current_state_a
            .map(|s| &s.items)
            .into_iter()
            .flatten()
            .chain(current_state_b.map(|s| &s.items).into_iter().flatten())
            .chain(previous_state_a.map(|s| &s.items).into_iter().flatten())
            .chain(previous_state_b.map(|s| &s.items).into_iter().flatten());

        let all_items = all_items.map(|i| &i.uid).collect::<HashSet<_>>();
        let item_actions = all_items
            .into_iter()
            .filter_map(|uid| {
                let item_a = current_state_a.and_then(|s| s.get_item_by_uid(uid));
                let item_b = current_state_b.and_then(|s| s.get_item_by_uid(uid));

                if item_a.is_some_and(|a| item_b.is_some_and(|b| a.hash == b.hash)) {
                    trace!("Item uid={} is unchanged; will take no action.", uid);
                    return None;
                }

                let prev_item_a = previous_state_a.and_then(|s| s.get_item_by_uid(uid));
                let prev_item_b = previous_state_b.and_then(|s| s.get_item_by_uid(uid));

                let a_changed = Change::for_item(item_a, prev_item_a);
                let b_changed = Change::for_item(item_b, prev_item_b);

                Action::from_changes(a_changed, b_changed).map(|action| ItemAction {
                    uid: uid.clone(),
                    action,
                })
            })
            .collect::<Vec<ItemAction>>();

        let collection_action = Action::for_collection(
            mapping,
            current_state_a,
            current_state_b,
            previous_state_b,
            previous_state_a,
        );

        if collection_action.is_none() && item_actions.is_empty() {
            None
        } else {
            Some(CollectionPlan {
                mapping: mapping.clone(),
                collection_action,
                items: item_actions,
            })
        }
    }

    pub(super) fn into_parts(self) -> (ResolvedMapping, Option<CollectionAction>, Vec<ItemAction>) {
        (self.mapping, self.collection_action, self.items)
    }
}

/// An action to executing when synchronising.
#[derive(PartialEq, Debug, Clone)]
pub enum Action {
    CreateInA { source: Href },
    CreateInB { source: Href },
    UpdateInA { source: Href, target: ItemRef },
    UpdateInB { source: Href, target: ItemRef },
    DeleteInA { href: Href, etag: Etag }, // TODO: use ItemRef here too
    DeleteInB { href: Href, etag: Etag },
    Conflict, // TODO: content might still match on both sides
}

/// An action to executing on a collection when synchronising.
#[derive(PartialEq, Debug, Clone)]
pub enum CollectionAction {
    CreateInA { collection: ResolvedCollection },
    CreateInB { collection: ResolvedCollection },
    DeleteInA { href: Href },
    DeleteInB { href: Href },
}

impl Action {
    /// Return the correct action given a pair of changes.
    ///
    /// `None` implies that no action needs to be taken.
    #[must_use]
    fn from_changes(left: Change, right: Change) -> Option<Action> {
        match (left, right) {
            (Change::Changed { .. }, Change::Changed { .. }) => Some(Action::Conflict),
            (Change::NoChange { href, etag }, Change::Deleted { .. }) => Some(Action::DeleteInA {
                href: href.clone(),
                etag: etag.clone(),
            }),
            (Change::Deleted { .. }, Change::NoChange { href, etag }) => Some(Action::DeleteInB {
                href: href.clone(),
                etag: etag.clone(),
            }),
            // Copy new into A.
            (Change::Deleted { .. } | Change::Absent, Change::Changed { href })
            | (Change::Absent, Change::NoChange { href, .. }) => Some(Action::CreateInA {
                source: href.clone(),
            }),
            // Copy and overwrite into A.
            (
                Change::NoChange {
                    etag,
                    href: target_href,
                },
                Change::Changed { href },
            ) => Some(Action::UpdateInA {
                source: href.clone(),
                target: ItemRef {
                    etag: etag.clone(),
                    href: target_href.clone(),
                },
            }),
            // Copy new into B.
            (Change::Changed { href }, Change::Deleted { .. } | Change::Absent)
            | (Change::NoChange { href, .. }, Change::Absent) => Some(Action::CreateInB {
                source: href.clone(),
            }),
            // Copy and overwrite into B.
            (
                Change::Changed { href },
                Change::NoChange {
                    etag,
                    href: target_href,
                },
            ) => Some(Action::UpdateInB {
                source: href.clone(),
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

    #[must_use]
    fn for_collection<'href>(
        mapping: &ResolvedMapping,
        current_a: Option<&'href CollectionState>,
        current_b: Option<&'href CollectionState>,
        previous_a: Option<&'href CollectionState>,
        previous_b: Option<&'href CollectionState>,
    ) -> Option<CollectionAction> {
        match (current_a, current_b, previous_a, previous_b) {
            (None, None, _, _) | (Some(_), Some(_), _, _) => None,
            // New or present in B, missing from A.
            (None, Some(_), _, None) | (None, Some(_), None, Some(_)) => {
                Some(CollectionAction::CreateInA {
                    collection: mapping.a.clone(),
                })
            }
            // Deleted from A.
            (None, Some(c), Some(_), Some(_)) => Some(CollectionAction::DeleteInB {
                href: c.href.clone(),
            }),
            // New or present in A, missing from B.
            (Some(_), None, None, _) | (Some(_), None, Some(_), None) => {
                Some(CollectionAction::CreateInB {
                    collection: mapping.b.clone(),
                })
            }
            // Deleted from B.
            (Some(c), None, Some(_), Some(_)) => Some(CollectionAction::DeleteInA {
                href: c.href.clone(),
            }),
        }
    }
}

/// A transition that has occurred to a pair of items or collections.
#[derive(Debug, Clone)]
pub(super) enum Change<'href> {
    /// Mutated or created.
    Changed { href: &'href Href },
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
    pub(super) fn for_item(
        current: Option<&'href ItemState>,
        previous: Option<&ItemState>,
    ) -> Change<'href> {
        match (current, previous) {
            (Some(c), Some(p)) => {
                if c.uid == p.uid && c.etag == p.etag && c.hash == p.hash {
                    Change::NoChange {
                        href: &c.href,
                        etag: &c.etag,
                    }
                } else {
                    Change::Changed { href: &c.href }
                }
            }
            (Some(c), None) => Change::Changed { href: &c.href },
            (None, Some(_)) => Change::Deleted,
            (None, None) => Change::Absent,
        }
    }
}
