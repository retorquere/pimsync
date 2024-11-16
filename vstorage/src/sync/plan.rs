// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Plan for a synchronisation.
//!
//! The main entry point of this module is [`Plan`]

use std::collections::HashSet;
use std::sync::Arc;

use log::{debug, warn};

use crate::base::{FetchedItem, ItemRef, Property, Storage};
use crate::disco::{DiscoveredCollection, Discovery};
use crate::{base::Item, sync::declare::StoragePair};
use crate::{CollectionId, ErrorKind, Href};

use super::declare::{CollectionDescription, DeclaredMapping, OnEmpty};
use super::status::{ItemState, MappingUid, Side, StatusDatabase, StatusError, StatusForItem};

/// Error that occurs when creating a [`Plan`].
#[derive(thiserror::Error, Debug)]
pub enum PlanError {
    /// Conflicting mapping haves been defined.
    ///
    /// Two (or more) collections on one side would be synchronised to the same collection on the
    /// other side. The `Side` and `Href` parameters refer to the collection that has multiple
    /// counterparts.
    #[error("Conflicting mappings on side {0} for href {1}.")]
    ConflictingMappings(Side, Href),

    /// Discovering collections on storage A failed.
    #[error("Discovery failed for storage A: {0}")]
    DiscoveryFailedA(#[source] crate::Error),

    /// Discovering collections on storage B failed.
    #[error("Discovery failed for storage B: {0}")]
    DiscoveryFailedB(#[source] crate::Error),

    /// Error occurred interacting with a storage.
    #[error("Error interacting with underlying storage: {0}")]
    Storage(#[from] crate::Error),

    /// Error occurred reading the status database.
    #[error("Error querying status database: {0}")]
    StatusDb(#[from] StatusError),
}

/// Actions that would synchronise a pair of storages.
///
/// Use [`Plan::new`] to create new instances.
///
/// Use [`Plan::collection_plans`]) to inspect the plan and render it into a human-friendly
/// representation, into a CSV, or into any other format that is necessary.
///
/// Use [`Plan::execute`] to execute this plan and apply changes to the provided [`Storage`]
/// instances.
pub struct Plan<I: Item> {
    pub(super) storage_a: Arc<dyn Storage<I>>,
    pub(super) storage_b: Arc<dyn Storage<I>>,
    pub collection_plans: Vec<CollectionPlan<I>>,
}

/// Show details of the plan itself.
///
/// This is partially necessary because storages don't yet implement `Debug` .
// TODO: they should
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

        let mut collection_plans = Vec::with_capacity(mappings.len());
        for m in mappings {
            collection_plans.push(CollectionPlan::new(pair, m, status).await?);
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
            .await?,
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
            // A mapping might already be present if we used `from_a`.
            if mappings.iter().any(|m| *m == mapping) {
                debug!("Skipping mapping; already present.");
            } else {
                mappings.push(mapping);
            }
        }
    }
    check_for_duplicate_mappings(&mappings)?;
    Ok(mappings)
}

fn check_for_duplicate_mappings(mappings: &[ResolvedMapping]) -> Result<(), PlanError> {
    let mut seen = Vec::<(&Href, &Href)>::new(); // Contains (href_a, href_b)

    for mapping in mappings {
        if let Some(conflict) = seen.iter().find_map(|s| {
            if s.0 == &mapping.a.href {
                Some(PlanError::ConflictingMappings(Side::A, s.0.to_string()))
            } else if s.1 == &mapping.b.href {
                Some(PlanError::ConflictingMappings(Side::B, s.1.to_string()))
            } else {
                None
            }
        }) {
            return Err(conflict);
        };
        seen.push((&mapping.a.href, &mapping.b.href));
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::{str::FromStr, sync::Arc};

    use tempfile::Builder;

    use crate::{
        base::{IcsItem, Storage},
        sync::{
            declare::{CollectionDescription, DeclaredMapping, StoragePair},
            plan::{create_mappings_for_pair, Plan, PlanError},
        },
        vdir::VdirStorage,
        CollectionId,
    };

    #[tokio::test]
    async fn test_plan_no_mappings() {
        let dir_a = Builder::new().prefix("vstorage").tempdir().unwrap();
        let dir_b = Builder::new().prefix("vstorage").tempdir().unwrap();

        let storage_a = Arc::new(VdirStorage::<IcsItem>::new(
            dir_a.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));
        let storage_b = Arc::from(VdirStorage::<IcsItem>::new(
            dir_b.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));

        // This sync would be a no-op, but it's not "wrong".
        let pair = StoragePair::new(storage_a.clone(), storage_b.clone());
        assert!(Plan::new(&pair, None).await.is_ok());
    }

    #[tokio::test]
    async fn test_plan_simple_mapping() {
        let dir_a = Builder::new().prefix("vstorage").tempdir().unwrap();
        let dir_b = Builder::new().prefix("vstorage").tempdir().unwrap();

        let storage_a = Arc::new(VdirStorage::<IcsItem>::new(
            dir_a.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));
        let storage_b = Arc::from(VdirStorage::<IcsItem>::new(
            dir_b.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));
        // This sync is okay.
        let collection = CollectionId::from_str("test").unwrap();
        let pair = StoragePair::new(storage_a.clone(), storage_b.clone())
            .with_mapping(DeclaredMapping::direct(collection));

        let mappings = create_mappings_for_pair(&pair).await.unwrap();
        assert_eq!(mappings.len(), 1);

        let plan = Plan::new(&pair, None).await.unwrap();
        assert_eq!(plan.collection_plans.len(), 1);
    }

    #[tokio::test]
    async fn test_plan_duplicate_mapping() {
        let dir_a = Builder::new().prefix("vstorage").tempdir().unwrap();
        let dir_b = Builder::new().prefix("vstorage").tempdir().unwrap();

        let storage_a = Arc::new(VdirStorage::<IcsItem>::new(
            dir_a.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));
        let storage_b = Arc::from(VdirStorage::<IcsItem>::new(
            dir_b.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));

        // Duplicate mapping
        let collection = CollectionId::from_str("test").unwrap();
        let pair = StoragePair::new(storage_a.clone(), storage_b.clone())
            .with_mapping(DeclaredMapping::direct(collection.clone()))
            .with_mapping(DeclaredMapping::direct(collection));

        let err = create_mappings_for_pair(&pair).await.unwrap_err();
        assert!(matches!(err, PlanError::ConflictingMappings(..)));
    }

    #[tokio::test]
    async fn test_plan_conflicting_mapping() {
        let dir_a = Builder::new().prefix("vstorage").tempdir().unwrap();
        let dir_b = Builder::new().prefix("vstorage").tempdir().unwrap();

        let storage_a = Arc::new(VdirStorage::<IcsItem>::new(
            dir_a.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));
        let storage_b = Arc::from(VdirStorage::<IcsItem>::new(
            dir_b.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));
        // This sync has duplicate items.
        let collection = CollectionId::from_str("test").unwrap();
        let pair = StoragePair::new(storage_a.clone(), storage_b.clone())
            .with_mapping(DeclaredMapping::direct(collection.clone()))
            .with_mapping(DeclaredMapping::Mapped {
                alias: "test".to_string(),
                a: CollectionDescription::Id { id: collection },
                b: CollectionDescription::Id {
                    id: CollectionId::from_str("test_2").unwrap(),
                },
            });

        let err = create_mappings_for_pair(&pair).await.unwrap_err();
        assert!(matches!(err, PlanError::ConflictingMappings(..)));
    }

    #[tokio::test]
    async fn test_plan_same_from_both_sides() {
        let dir_a = Builder::new().prefix("vstorage").tempdir().unwrap();
        let dir_b = Builder::new().prefix("vstorage").tempdir().unwrap();

        let storage_a = Arc::new(VdirStorage::<IcsItem>::new(
            dir_a.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));
        let storage_b = Arc::from(VdirStorage::<IcsItem>::new(
            dir_b.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        ));
        // `from_a` and `from_b` with collection existing on both sides.
        // This particular scenario is special-cased.
        std::fs::create_dir(dir_a.path().join("one")).unwrap();
        std::fs::create_dir(dir_b.path().join("one")).unwrap();

        let disco = storage_a.discover_collections().await.unwrap();
        assert_eq!(disco.collections().len(), 1);

        let pair = StoragePair::new(storage_a.clone(), storage_b.clone())
            .with_all_from_a()
            .with_all_from_b();

        let mappings = create_mappings_for_pair(&pair).await.unwrap();
        assert_eq!(mappings.len(), 1);

        let plan = Plan::new(&pair, None).await.unwrap();
        assert_eq!(plan.collection_plans.len(), 1);
    }
}

/// A mapping resolved based on the storage's current state.
#[derive(Debug, PartialEq)]
pub(super) struct ResolvedMapping {
    pub(super) alias: String,
    pub(super) a: ResolvedCollection,
    pub(super) b: ResolvedCollection,
}

impl ResolvedMapping {
    async fn from_declared_mapping<I: Item>(
        declared: &DeclaredMapping,
        storage_a: &dyn Storage<I>,
        storage_b: &dyn Storage<I>,
        disco_a: &Discovery,
        disco_b: &Discovery,
    ) -> Result<Self, crate::Error> {
        match declared {
            DeclaredMapping::Direct { description } => Ok(ResolvedMapping {
                alias: description.alias(),
                a: ResolvedCollection::from_declaration(description, disco_a, storage_a).await?,
                b: ResolvedCollection::from_declaration(description, disco_b, storage_b).await?,
            }),
            DeclaredMapping::Mapped { a, b, alias } => Ok(ResolvedMapping {
                alias: alias.to_string(),
                a: ResolvedCollection::from_declaration(a, disco_a, storage_a).await?,
                b: ResolvedCollection::from_declaration(b, disco_b, storage_b).await?,
            }),
        }
    }
}

/// A collection as resolved based on existing data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedCollection {
    pub(super) id: Option<CollectionId>,
    pub(super) href: Href,
    exists: bool,
}

impl ResolvedCollection {
    /// Resolve the collection based on a storage and its collections.
    async fn from_declaration<I: Item>(
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
                    Ok(ResolvedCollection {
                        id: Some(id.clone()),
                        href: storage.href_for_collection_id(id)?,
                        exists: false,
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
            exists: false,
        }),
    }
}

/// Actions required to sync a collection between two storages.
#[derive(Debug)]
pub struct CollectionPlan<I: Item> {
    pub collection_action: CollectionAction,
    pub item_actions: Vec<ItemAction>,
    pub property_actions: Vec<PropertyPlan<I>>,
    pub(super) mapping: ResolvedMapping,
}

impl<I: Item> CollectionPlan<I> {
    /// Calculate actions to sync a collection between two storages.
    async fn new(
        pair: &StoragePair<I>,
        mapping: ResolvedMapping,
        status: Option<&StatusDatabase>,
    ) -> Result<CollectionPlan<I>, PlanError> {
        let (href_a, href_b) = (&mapping.a.href, &mapping.b.href);
        let mapping_uid = status
            .map(|s| s.get_mapping_uid(href_a, href_b))
            .transpose()?
            .flatten();

        let (items_a, items_b) = tokio::try_join!(
            items_for_collection(status, pair.storage_a(), href_a, Side::A),
            items_for_collection(status, pair.storage_b(), href_b, Side::B),
        )?;

        let status_uids = match (status, &mapping_uid) {
            (Some(s), Some(m)) => s.all_uids(m)?,
            _ => Vec::new(),
        };

        if (items_a.is_empty() || items_b.is_empty())
            && !status_uids.is_empty()
            && pair.on_empty == OnEmpty::Skip
        {
            let mapping_uid =
                mapping_uid.expect("If mapping_uid is None, then status_uid must be empty.");
            warn!("Collection has been emptied on one side; skipping.");
            return Ok(CollectionPlan {
                collection_action: CollectionAction::NoAction(mapping_uid),
                item_actions: Vec::new(),
                property_actions: Vec::new(),
                mapping,
            });
        }

        let all_uids = items_a
            .iter()
            .chain(items_b.iter())
            .map(|i| &i.uid)
            .chain(status_uids.iter())
            .collect::<HashSet<_>>(); // Collecting into HashSet removes duplicates.

        let item_actions = all_uids
            .into_iter()
            .map(|uid| {
                let item_a = items_a.iter().find(|i| i.uid == *uid);
                let item_b = items_b.iter().find(|i| i.uid == *uid);

                let previous = match (status, &mapping_uid) {
                    (Some(s), Some(m)) => s.get_item_hash_by_uid(m, uid)?,
                    _ => None,
                };

                Ok(ItemAction::for_item(item_a, item_b, previous, uid))
            })
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, PlanError>>()?;

        let collection_action =
            CollectionAction::new(mapping.a.exists, mapping.b.exists, mapping_uid.clone());

        // TODO: need to pass items_a and items_b to map Href->UID for item properties.
        let property_actions =
            match PropertyPlan::create_for_collection(pair, &mapping, status, mapping_uid).await {
                Ok(plan) => plan,
                // If a storage doesn't support properties, don't bail, simply no-op.
                Err(err) => 'unsupported: {
                    if let PlanError::Storage(e) = &err {
                        if e.kind == ErrorKind::Unsupported {
                            break 'unsupported Vec::<PropertyPlan<I>>::new();
                        }
                    }
                    return Err(err);
                }
            };

        Ok(CollectionPlan {
            collection_action,
            item_actions,
            property_actions,
            mapping,
        })
    }

    #[must_use]
    pub fn alias(&self) -> &str {
        &self.mapping.alias
    }
}

/// Operation to execute on an item during synchronising.
#[derive(PartialEq, Debug, Clone)]
pub enum ItemAction {
    /// Item is new and identical on both sides.
    SaveToStatus {
        a: ItemState,
        b: ItemState,
    },
    /// Item has changed and is identical on both sides.
    UpdateStatus {
        hash: String,
        /// As previously seen on a.
        old_a: ItemRef,
        /// As previously seen on b.
        old_b: ItemRef,
        new_a: ItemRef,
        new_b: ItemRef,
    },
    /// Item is gone from both sides but still present in status db.
    ClearStatus {
        uid: String,
    },
    Create {
        side: Side,
        source: ItemState,
    },
    Update {
        side: Side,
        source: ItemState,
        target: Href,
        old_a: ItemRef,
        old_b: ItemRef,
    },
    Delete {
        side: Side,
        target: ItemState,
    },
    /// Item is in conflict which needs to be resolved externally.
    Conflict {
        a: ItemState,
        b: ItemState,
        // Indicates that the item is new on both sides.
        is_new: bool,
    },
}

impl ItemAction {
    #[allow(clippy::too_many_lines)]
    #[must_use]
    fn for_item(
        current_a: Option<&ItemState>,
        current_b: Option<&ItemState>,
        previous: Option<StatusForItem>,
        uid: &str,
    ) -> Option<ItemAction> {
        match (current_a, current_b, previous) {
            (None, None, None) => unreachable!("no action for item that doesn't exist anywhere"),
            (None, None, Some(_)) => Some(ItemAction::ClearStatus {
                uid: uid.to_string(),
            }),
            (None, Some(b), None) => Some(ItemAction::Create {
                side: Side::A,
                source: b.clone(),
            }),
            (None, Some(b), Some(prev)) => {
                if b.hash == prev.hash {
                    Some(ItemAction::Delete {
                        side: Side::B,
                        target: b.clone(),
                    })
                } else {
                    warn!("Item deleted in A but changed B: {}.", b.uid);
                    Some(ItemAction::Create {
                        side: Side::A,
                        source: b.clone(),
                    })
                }
            }
            (Some(a), None, None) => Some(ItemAction::Create {
                side: Side::B,
                source: a.clone(),
            }),
            (Some(a), None, Some(prev)) => {
                if a.hash == prev.hash {
                    Some(ItemAction::Delete {
                        side: Side::A,
                        target: a.clone(),
                    })
                } else {
                    warn!("Item deleted in B but changed A: {}.", a.uid);
                    Some(ItemAction::Create {
                        side: Side::B,
                        source: a.clone(),
                    })
                }
            }
            (Some(a), Some(b), Some(prev)) => {
                if a.hash == b.hash {
                    // Item is equivalent on both sides.

                    if a.hash != prev.hash // If content has changed
                        || a.href != prev.href_a // ... or an href has changed ...
                        || b.href != prev.href_b
                        || a.etag != prev.etag_a // ... or an etag has changed ...
                        || b.etag != prev.etag_b
                    {
                        // ... update the status to prevent fetching the item until changes again.
                        Some(ItemAction::UpdateStatus {
                            hash: a.hash.clone(),
                            old_a: ItemRef {
                                href: prev.href_a,
                                etag: prev.etag_a,
                            },
                            old_b: ItemRef {
                                href: prev.href_b.clone(),
                                etag: prev.etag_b,
                            },
                            new_a: ItemRef {
                                href: a.href.clone(),
                                etag: a.etag.clone(),
                            },
                            new_b: ItemRef {
                                href: b.href.clone(),
                                etag: b.etag.clone(),
                            },
                        })
                    } else {
                        // Item is identical, has not moved and Etag has not changed.
                        None
                    }
                } else if a.hash == prev.hash {
                    // Side A has not changed
                    Some(ItemAction::Update {
                        side: Side::A,
                        source: b.clone(),
                        target: a.href.clone(),
                        old_a: a.to_item_ref(),
                        old_b: ItemRef {
                            href: prev.href_b,
                            etag: prev.etag_b,
                        },
                    })
                } else if b.hash == prev.hash {
                    // Side B has not changed
                    Some(ItemAction::Update {
                        side: Side::B,
                        source: a.clone(),
                        target: b.href.clone(),
                        old_a: ItemRef {
                            href: prev.href_a,
                            etag: prev.etag_a,
                        },
                        old_b: b.to_item_ref(),
                    })
                } else {
                    // Both sides have changed
                    Some(ItemAction::Conflict {
                        a: a.clone(),
                        b: b.clone(),
                        is_new: false,
                    })
                }
            }
            (Some(a), Some(b), None) => {
                if a.hash == b.hash {
                    Some(ItemAction::SaveToStatus {
                        a: a.clone(),
                        b: b.clone(),
                    })
                } else {
                    Some(ItemAction::Conflict {
                        a: a.clone(),
                        b: b.clone(),
                        is_new: true,
                    })
                }
            }
        }
    }
}

impl std::fmt::Display for ItemAction {
    /// This function is mostly implemented to be used for error reporting.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemAction::SaveToStatus { a, .. } => write!(f, "save to status (uid: {})", a.uid),
            ItemAction::UpdateStatus { old_a, .. } => {
                write!(f, "update in status (a.href: {})", old_a.href)
            }
            ItemAction::ClearStatus { uid } => write!(f, "clear from status (uid: {uid})"),
            ItemAction::Create { side, source } => {
                write!(f, "create in storage {} (uid: {})", side, source.uid)
            }
            ItemAction::Update { source, side, .. } => {
                write!(f, "update in storage {side} (href: {})", source.href)
            }
            ItemAction::Delete { side, target } => {
                write!(f, "delete in storage {} (uid: {})", side, target.uid)
            }
            ItemAction::Conflict { a, .. } => {
                write!(f, "conflict (uid: {})", a.uid)
            }
        }
    }
}

/// Operation to execute on a collection during synchronising.
#[allow(private_interfaces)]
#[derive(PartialEq, Debug, Clone)]
pub enum CollectionAction {
    NoAction(MappingUid),
    SaveToStatus,
    CreateInA,
    CreateInB,
    CreateInBoth,
    Delete(MappingUid, Side),
    // TODO: FlushState (for collections that are gone)
}

impl CollectionAction {
    /// Calculate action for a given collection
    ///
    /// # Note on collection deletion
    ///
    /// Collections should only be deleted if they were found via discovery and discovery is
    /// still enabled. If it was explicitly removed from the configuration, it should remain.
    ///
    /// Therefore, this function should only take as input:
    ///
    /// - Explicitly configured collections
    /// - Discovered collections
    ///
    /// It SHOULD NOT be called with collections that only exist in the status database.
    ///
    /// Collections previously auto-discovered and deleted on both sides should never reach this
    /// stage. Explicit collections removed from configuration will also not reach this stage.
    fn new(current_a: bool, current_b: bool, mapping_uid: Option<MappingUid>) -> CollectionAction {
        match (current_a, current_b, mapping_uid) {
            (false, false, _) => CollectionAction::CreateInBoth, // Deleted or missing on both sides.
            (true, true, None) => CollectionAction::SaveToStatus, // New on both sides.
            (true, true, Some(m)) => CollectionAction::NoAction(m), // No change.
            (false, true, Some(m)) => CollectionAction::Delete(m, Side::B), // Deleted from A
            (false, true, None) => CollectionAction::CreateInA,  // New in B
            (true, false, None) => CollectionAction::CreateInB,  // New in A
            (true, false, Some(m)) => CollectionAction::Delete(m, Side::A), // Deleted from B.
        }
    }
}

impl std::fmt::Display for CollectionAction {
    /// Only the action itself is displayed.
    ///
    /// This function is mostly implemented to be used for error reporting.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollectionAction::NoAction(_) => write!(f, "no action"),
            CollectionAction::SaveToStatus => write!(f, "save to status"),
            CollectionAction::CreateInA => write!(f, "create in storage a"),
            CollectionAction::CreateInB => write!(f, "create in storage b"),
            CollectionAction::CreateInBoth => write!(f, "create in both storages"),
            CollectionAction::Delete(_, side) => write!(f, "delete from {side}"),
        }
    }
}

/// Returns the state of all items for a collection.
///
/// Returns only items that currently exist in the remote storage.
///
/// If an item has changed `href`, the updated `href` is returned.
async fn items_for_collection<I: Item>(
    status: Option<&StatusDatabase>,
    storage: &dyn Storage<I>,
    collection: &Href,
    side: Side,
) -> Result<Vec<ItemState>, PlanError> {
    debug!("Resolving state for collection: {}.", collection);
    let mut items = Vec::new();

    let prefetched = if let Some(status) = status {
        let mut to_prefetch = Vec::new();

        let listed_items = match storage.list_items(collection).await {
            Ok(i) => i,
            Err(err) if err.kind == ErrorKind::DoesNotExist => {
                return Ok(Vec::new());
            }
            Err(err) => return Err(err.into()),
        };

        for item_ref in listed_items {
            if let Some(prev_item) = status.get_item_by_href(side, &item_ref.href)? {
                if prev_item.etag == item_ref.etag {
                    // Item has not changed; nothing to fetch.
                    items.push(prev_item);
                    continue;
                } // else: item has changed
            } // else: item is new OR item has moved
            to_prefetch.push(item_ref.href);
        }

        let to_prefetch = to_prefetch.iter().map(String::as_str).collect::<Vec<_>>();
        storage.get_many_items(&to_prefetch).await?
    } else {
        match storage.get_all_items(collection).await {
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
            data: Some(item.into()),
        });
    items.extend(prefetched);

    Ok(items)
}

#[derive(Debug)]
pub enum PropertyAction {
    WriteToA { value: String },
    WriteToB { value: String },
    DeleteInA,
    DeleteInB,
    ClearStatus,
    UpdateStatus { value: String },
    Conflict,
}

impl std::fmt::Display for PropertyAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PropertyAction::WriteToA { value } => write!(f, "Write to a: {value}"),
            PropertyAction::WriteToB { value } => write!(f, "Write to b: {value}"),
            PropertyAction::DeleteInA => write!(f, "Delete in a"),
            PropertyAction::DeleteInB => write!(f, "Delete in b"),
            PropertyAction::ClearStatus => write!(f, "Clear status"),
            PropertyAction::UpdateStatus { .. } => write!(f, "Update status"),
            PropertyAction::Conflict => write!(f, "Conflict"),
        }
    }
}

#[derive(Debug)]
pub struct PropertyPlan<I: Item> {
    pub(super) property: I::Property,
    pub(super) action: PropertyAction,
}

impl<I: Item> PropertyPlan<I> {
    async fn create_for_collection(
        pair: &StoragePair<I>,
        mapping: &ResolvedMapping,
        status: Option<&StatusDatabase>,
        uid: Option<MappingUid>,
    ) -> Result<Vec<PropertyPlan<I>>, PlanError> {
        let props_a = pair.storage_a().list_properties(&mapping.a.href).await?;
        let props_b = pair.storage_b().list_properties(&mapping.b.href).await?;

        let props_status = match (status, uid) {
            (Some(s), Some(u)) => s.list_properties_for_collection(&u)?,
            _ => Vec::new(),
        };

        let all_props = props_a
            .iter()
            .chain(props_b.iter())
            .map(|p| &p.property)
            .collect::<HashSet<_>>(); // Collecting into HashSet removes duplicates.

        let mut actions = Vec::new();
        for property in all_props {
            // TODO: it is safe to POP from the vec and handle owned data.
            let a = props_a.iter().find(|p| p.property == *property);
            let b = props_b.iter().find(|p| p.property == *property);
            let state = props_status.iter().find(|p| p.property == property.name());

            let action = match (a, b, state) {
                (None, None, None) => None,
                (None, None, Some(_)) => Some(PropertyAction::ClearStatus),
                (None, Some(b), None) => Some(PropertyAction::WriteToA {
                    value: b.value.clone(),
                }),
                (None, Some(_), Some(_)) => Some(PropertyAction::DeleteInB {}),
                (Some(a), None, None) => Some(PropertyAction::WriteToB {
                    value: a.value.clone(),
                }),
                (Some(_), None, Some(_)) => Some(PropertyAction::DeleteInA {}),
                (Some(a), Some(b), None) => {
                    if a.value == b.value {
                        Some(PropertyAction::UpdateStatus {
                            value: a.value.clone(),
                        })
                    } else {
                        Some(PropertyAction::Conflict)
                    }
                }
                (Some(a), Some(b), Some(s)) => {
                    if a.value == b.value {
                        if s.value == a.value {
                            None
                        } else {
                            Some(PropertyAction::UpdateStatus {
                                value: a.value.clone(),
                            })
                        }
                    } else if a.value == s.value {
                        Some(PropertyAction::WriteToA {
                            value: b.value.clone(),
                        })
                    } else if b.value == s.value {
                        Some(PropertyAction::WriteToB {
                            value: a.value.clone(),
                        })
                    } else {
                        Some(PropertyAction::Conflict)
                    }
                }
            };

            if let Some(action) = action {
                actions.push(PropertyPlan {
                    action,
                    property: property.clone(), // TODO: don't clone
                });
            }
        }

        Ok(actions)
    }
}
