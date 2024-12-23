// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Executor`] as the main type in this module.

use log::{debug, error, info, warn};

use crate::{
    base::{Item, ItemRef, Property, Storage},
    disco::DiscoveredCollection,
    CollectionId, Href,
};

use super::{
    error::SyncError,
    plan::{CollectionAction, CollectionPlan, ItemAction, Plan, PropertyPlan, ResolvedMapping},
    status::{ItemState, MappingUid, Side, StatusDatabase, StatusError},
};

/// Executes a plan or individual actions.
///
/// At this time, an executor can only execute a plan, but in future it may be able to execute a
/// stream of actions for keeping collections continuously in sync.
pub struct Executor<I: Item> {
    on_error: fn(SyncError<I>),
}

impl<I: Item> Executor<I> {
    /// Create a new instance.
    ///
    /// Use the given `on_error` function to handle non-fatal errors. See [`Executor::plan`] for
    /// further details on error handling.
    pub fn new(on_error: fn(SyncError<I>)) -> Executor<I> {
        Executor { on_error }
    }

    /// Executes a whole plan.
    ///
    /// # Errors
    ///
    /// Returns `Err(_)` if a fatal error occurred when interacting with the status database. Fatal
    /// errors are errors that occur when interacting with the status database, which tracks the
    /// current state of multiple storages. When a fatal error occurs, neither the status database
    /// nor the `Executor` instance should be re-used until the underlying issue is resolved.
    ///
    /// When a non-fatal error occurs, the `on_error` shall be invoked. E.g.: when the plan
    /// requires creating many items, if a single item fails, the error for this operation is
    /// passed to the `on_error` function, while the overall operation continues. This allows
    /// handling individual errors (e.g.: displaying them to a user) without interrupting the
    /// operation or having to wait for the completion of the entire operation.
    pub async fn plan(&self, plan: Plan<I>, status: &StatusDatabase) -> Result<(), StatusError> {
        let storage_a = plan.storage_a.as_ref();
        let storage_b = plan.storage_b.as_ref();

        // Clear these first, since they might conflict with a new mapping.
        if !plan.stale_collections.is_empty() {
            info!("Flushing stale collections: {:?}", plan.stale_collections);
            status.flush_stale_mappings(plan.stale_collections)?;
        }

        for plan in plan.collection_plans {
            let CollectionPlan {
                collection_action,
                item_actions,
                property_actions,
                mapping,
            } = plan;

            let (mapping_uid, side_to_delete) = match self
                .collection(&collection_action, status, &mapping, storage_a, storage_b)
                .await?
            {
                Ok((m, s)) => (m, s),
                Err(err) => {
                    (self.on_error)(SyncError::collection(collection_action, mapping, err));
                    continue;
                }
            };

            let a = mapping.a();
            let b = mapping.b();

            for item_action in item_actions {
                if let Err(err) = self
                    .item(
                        &item_action,
                        storage_a,
                        storage_b,
                        &mapping,
                        status,
                        mapping_uid,
                    )
                    .await?
                {
                    (self.on_error)(SyncError::item(item_action, err));
                };
            }

            for prop_action in property_actions {
                if let Err(err) = self
                    .property(
                        &prop_action,
                        storage_a,
                        storage_b,
                        status,
                        mapping_uid,
                        &mapping,
                    )
                    .await?
                {
                    (self.on_error)(SyncError::property(prop_action.action, err));
                };
            }

            match side_to_delete {
                None => {}
                Some(Side::A) => {
                    if let Err(err) =
                        delete_collection(a.href(), status, storage_a, mapping_uid).await?
                    {
                        let action = CollectionAction::Delete(mapping_uid, Side::A);
                        (self.on_error)(SyncError::collection(action, mapping, err));
                    };
                }
                Some(Side::B) => {
                    if let Err(err) =
                        delete_collection(b.href(), status, storage_b, mapping_uid).await?
                    {
                        let action = CollectionAction::Delete(mapping_uid, Side::B);
                        (self.on_error)(SyncError::collection(action, mapping, err));
                    };
                }
            };
        }

        Ok(())
    }

    /// Execute this collection's action.
    ///
    /// Returns the [`MappingUid`] for this collection and the side that needs to be deleted, if
    /// any.
    ///
    /// # Errors
    ///
    /// - Returns `Err(_)` if a fatal error occurred when interacting with the status database.
    /// - Returns `Ok(Err(_))` in case of non-fatal error.
    async fn collection(
        &self,
        action: &CollectionAction,
        status: &StatusDatabase,
        mapping: &ResolvedMapping,
        storage_a: &dyn Storage<I>,
        storage_b: &dyn Storage<I>,
    ) -> Result<Result<(MappingUid, Option<Side>), ExecutionError>, StatusError> {
        let a = mapping.a();
        let b = mapping.b();
        match action {
            CollectionAction::NoAction(mapping_uid) => Ok(Ok((*mapping_uid, None))),
            CollectionAction::SaveToStatus => status
                .get_or_add_collection(a.href(), b.href(), a.id(), b.id())
                .map(|uid| Ok((uid, None))),
            CollectionAction::CreateInOne(side) => {
                let storage = match side {
                    Side::A => storage_a,
                    Side::B => storage_b,
                };
                create_collection(storage, status, mapping, *side)
                    .await
                    .map(|r| r.map(|uid| (uid, None)))
            }
            CollectionAction::CreateInBoth => {
                create_both_collections(storage_a, storage_b, mapping, status)
                    .await
                    .map(|r| r.map(|uid| (uid, None)))
            }
            CollectionAction::Delete(mapping, side) => Ok(Ok((*mapping, Some(*side)))),
        }
    }

    /// Execute action for a single item.
    ///
    /// # Errors
    ///
    /// - Returns `Err(_)` if a fatal error occurred when interacting with the status database.
    /// - Returns `Ok(Err(_))` in case of non-fatal error.
    #[inline]
    async fn item(
        &self,
        item: &ItemAction<I>,
        a: &dyn Storage<I>,
        b: &dyn Storage<I>,
        mapping: &ResolvedMapping,
        status: &StatusDatabase,
        mapping_uid: MappingUid,
    ) -> Result<Result<(), ExecutionError>, StatusError> {
        debug!("Executing item action: {item}");
        match item {
            ItemAction::SaveToStatus { a, b, uid, hash } => {
                status.insert_item(mapping_uid, uid, hash, a, b).map(Ok)
            }
            ItemAction::UpdateStatus { hash, old, new } => status
                .update_item(hash, &old.0, &old.1, &new.0, &new.1)
                .map(Ok),
            ItemAction::ClearStatus { uid } => status.delete_item(mapping_uid, uid).map(Ok),
            ItemAction::Create { side, source } => {
                create_item(source, status, mapping, a, b, mapping_uid, *side).await
            }
            ItemAction::Update {
                side,
                source,
                target,
                old,
            } => match side {
                Side::A => update_item(b, a, source, target, old, status, Side::A).await,
                Side::B => update_item(a, b, source, target, old, status, Side::B).await,
            },
            ItemAction::Delete { side, target, uid } => {
                let storage = if *side == Side::A { a } else { b };
                delete_item(target, status, storage, mapping_uid, uid).await
            }
            ItemAction::Conflict { a, .. } => {
                error!("Conflict for items {}. Skipping.", a.uid);
                Ok(Ok(()))
            }
        }
    }

    /// Execute action for a single property
    ///
    /// # Errors
    ///
    /// - Returns `Err(_)` if a fatal error occurred when interacting with the status database.
    /// - Returns `Ok(Err(_))` in case of non-fatal error.
    async fn property(
        &self,
        plan: &PropertyPlan<I>,
        a: &dyn Storage<I>,
        b: &dyn Storage<I>,
        status: &StatusDatabase,
        mapping_uid: MappingUid,
        mapping: &ResolvedMapping,
    ) -> Result<Result<(), ExecutionError>, StatusError> {
        let href_a = mapping.a().href();
        let href_b = mapping.b().href();
        match &plan.action {
            super::plan::PropertyAction::WriteToA { value } => {
                if let Err(err) = a.set_property(href_a, plan.property.clone(), value).await {
                    return Ok(Err(ExecutionError::from(err)));
                };
                status.set_property(mapping_uid, href_a, href_b, &plan.property.name(), value)?;
            }
            super::plan::PropertyAction::WriteToB { value } => {
                if let Err(err) = b.set_property(href_b, plan.property.clone(), value).await {
                    return Ok(Err(ExecutionError::from(err)));
                };
                status.set_property(mapping_uid, href_a, href_b, &plan.property.name(), value)?;
            }
            super::plan::PropertyAction::DeleteInA => {
                if let Err(err) = a.unset_property(href_a, plan.property.clone()).await {
                    return Ok(Err(ExecutionError::from(err)));
                };
                status.delete_property(
                    mapping_uid,
                    href_a,
                    href_b,
                    plan.property.name().as_str(),
                )?;
            }
            super::plan::PropertyAction::DeleteInB => {
                if let Err(err) = b.unset_property(href_b, plan.property.clone()).await {
                    return Ok(Err(ExecutionError::from(err)));
                };
                status.delete_property(
                    mapping_uid,
                    href_a,
                    href_b,
                    plan.property.name().as_str(),
                )?;
            }
            super::plan::PropertyAction::ClearStatus => {
                status.delete_property(
                    mapping_uid,
                    href_a,
                    href_b,
                    plan.property.name().as_str(),
                )?;
            }
            super::plan::PropertyAction::UpdateStatus { value } => {
                status.set_property(mapping_uid, href_a, href_b, &plan.property.name(), value)?;
            }
            super::plan::PropertyAction::Conflict => {
                error!("Conflict for property {}. Skipping.", plan.property.name());
            }
        };
        Ok(Ok(()))
    }
}

/// Error during execution of a synchronisation [`Plan`]. See [`SyncError`].
#[derive(thiserror::Error, Debug)]
pub enum ExecutionError {
    #[error(transparent)]
    Storage(#[from] crate::Error),
    #[error("created collection {1} on side {0} does not have the expected id, it has: {2:?}")]
    IdMismatch(Side, Href, Option<CollectionId>),
}

async fn create_item<I: Item>(
    // TODO: Unused field: source.hash, source.uid
    source: &ItemState<I>,
    status: &StatusDatabase,
    mapping: &ResolvedMapping,
    storage_a: &dyn Storage<I>,
    storage_b: &dyn Storage<I>,
    mapping_uid: MappingUid,
    side: Side,
) -> Result<Result<(), ExecutionError>, StatusError> {
    debug!("Creating item from {}", source.href);

    let (target_collection, src_storage, dst_storage) = match side {
        Side::A => (mapping.a().href(), storage_b, storage_a),
        Side::B => (mapping.b().href(), storage_a, storage_b),
    };

    let (item_data, source_etag) = if let Some(data) = &source.data {
        (data.clone(), source.etag.clone())
    } else {
        warn!("Fetching item to create during execution");
        match src_storage.get_item(&source.href).await {
            Ok((i, e)) => (i, e),
            Err(err) => return Ok(Err(ExecutionError::Storage(err))),
        }
    };

    let uid = item_data.ident();
    let new_item = match dst_storage.add_item(target_collection, &item_data).await {
        Ok(i) => i,
        Err(err) => return Ok(Err(ExecutionError::Storage(err))),
    };

    // The original Etag MAY have changed.
    let source_ref = ItemRef {
        href: source.href.clone(),
        etag: source_etag,
    };

    match side {
        Side::A => status.insert_item(mapping_uid, &uid, &item_data.hash(), &new_item, &source_ref),
        Side::B => status.insert_item(mapping_uid, &uid, &item_data.hash(), &source_ref, &new_item),
    }?;

    Ok(Ok(()))
}

async fn update_item<I: Item>(
    src_storage: &dyn Storage<I>,
    dst_storage: &dyn Storage<I>,
    // TODO: Unused field: source.hash, source.uid
    source: &ItemState<I>,
    target: &ItemRef,
    old: &(ItemRef, ItemRef),
    status: &StatusDatabase,
    side: Side,
) -> Result<Result<(), ExecutionError>, StatusError> {
    debug!("Updating from {}", source.href);
    let (source_item, source_etag) = if let Some(data) = &source.data {
        (data.clone(), source.etag.clone())
    } else {
        warn!("Fetching item to update during execution");
        match src_storage.get_item(&source.href).await {
            Ok((i, e)) => (i, e),
            Err(err) => return Ok(Err(ExecutionError::Storage(err))),
        }
    };

    let new_etag = match dst_storage
        .update_item(&target.href, &target.etag, &source_item)
        .await
    {
        Ok(i) => i,
        Err(err) => return Ok(Err(ExecutionError::Storage(err))),
    };

    let hash = source_item.hash();
    let target_ref = &ItemRef {
        href: target.href.clone(),
        etag: new_etag,
    };
    let source_ref = &ItemRef {
        href: source.href.clone(),
        etag: source_etag,
    };
    match side {
        Side::A => status.update_item(&hash, &old.0, &old.1, target_ref, source_ref),
        Side::B => status.update_item(&hash, &old.0, &old.1, source_ref, target_ref),
    }?;

    Ok(Ok(()))
}

async fn delete_item<I: Item>(
    target: &ItemRef,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    mapping_uid: MappingUid,
    uid: &str,
) -> Result<Result<(), ExecutionError>, StatusError> {
    debug!("Deleting {}", target.href);
    match storage.delete_item(&target.href, &target.etag).await {
        Ok(()) => Ok(Ok(status.delete_item(mapping_uid, uid)?)),
        Err(err) => Ok(Err(ExecutionError::Storage(err))),
    }
}

async fn delete_collection<I: Item>(
    href: &Href,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    mapping_uid: MappingUid,
) -> Result<Result<(), ExecutionError>, StatusError> {
    match storage.destroy_collection(href).await {
        Ok(()) => Ok(Ok(status.remove_collection(mapping_uid)?)),
        Err(err) => Ok(Err(ExecutionError::Storage(err))),
    }
}

/// Creates a collection and updates the state accordingly.
async fn create_collection<I: Item>(
    storage: &dyn Storage<I>,
    status: &StatusDatabase,
    mapping: &ResolvedMapping,
    side: Side,
) -> Result<Result<MappingUid, ExecutionError>, StatusError> {
    let (target, existing) = match side {
        Side::A => (mapping.a(), mapping.b()),
        Side::B => (mapping.b(), mapping.a()),
    };
    let new = match storage.create_collection(target.href()).await {
        Ok(c) => c,
        Err(err) => return Ok(Err(ExecutionError::Storage(err))),
    };
    if new.href() != target.href() {
        warn!(
            "Created collection has href {}, expected {}.",
            new.href(),
            target.href()
        );
    }

    if let Err(err) = check_id_matches_expected(target.id(), storage, new.href(), side).await {
        return Ok(Err(err));
    };
    let mapping_uid = match side {
        Side::A => status.add_collection(new.href(), existing.href(), target.id(), existing.id()),
        Side::B => status.add_collection(existing.href(), new.href(), existing.id(), target.id()),
    }?;
    Ok(Ok(mapping_uid))
}

async fn create_both_collections<I: Item>(
    storage_a: &dyn Storage<I>,
    storage_b: &dyn Storage<I>,
    mapping: &ResolvedMapping,
    status: &StatusDatabase,
) -> Result<Result<MappingUid, ExecutionError>, StatusError> {
    let href_a = mapping.a().href();
    let href_b = mapping.b().href();
    let id_a = mapping.a().id();
    let id_b = mapping.b().id();

    let new_a = match storage_a.create_collection(href_a).await {
        Ok(c) => c,
        Err(err) => return Ok(Err(ExecutionError::Storage(err))),
    };
    if let Err(err) = check_id_matches_expected(id_a, storage_a, new_a.href(), Side::A).await {
        return Ok(Err(err));
    };
    let new_b = match storage_b.create_collection(href_b).await {
        Ok(c) => c,
        Err(err) => return Ok(Err(ExecutionError::Storage(err))),
    };
    if let Err(err) = check_id_matches_expected(id_b, storage_b, new_b.href(), Side::B).await {
        return Ok(Err(err));
    };

    Ok(Ok(status.get_or_add_collection(href_a, href_b, id_a, id_b)?))
}

async fn check_id_matches_expected<I: Item>(
    expected_id: Option<&CollectionId>,
    storage: &dyn Storage<I>,
    collection: &Href,
    side: Side,
) -> Result<(), ExecutionError> {
    if let Some(expected_id) = expected_id {
        let disco = storage.discover_collections().await?;
        let created = disco.collections().iter().find(|c| c.href() == collection);
        // FIXME: returned error description is incorrect in case of None.
        let created_id = created.map(DiscoveredCollection::id);
        if created_id != Some(expected_id) {
            return Err(ExecutionError::IdMismatch(
                side,
                collection.to_string(),
                created_id.cloned(),
            ));
        }
    }
    Ok(())
}
