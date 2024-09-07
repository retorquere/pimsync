// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Plan::execute`](Plan::execute).

use log::{debug, error};

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

impl ItemAction {
    /// Execution the action on the item.
    ///
    /// The `state_a` or `state_b` variables should only be `None` if the collection does not exist
    /// in that storage. That should only really happen if the storage has been deleted.
    #[inline]
    async fn execute<I: Item>(
        &self,
        a: &dyn Storage<I>,
        b: &dyn Storage<I>,
        col_a: &Href,
        col_b: &Href,
        status: &StatusDatabase,
        mapping_uid: &MappingUid,
    ) -> Result<Result<(), ExecutionError>, StatusError> {
        debug!("Executing item action: {self}");
        match self {
            ItemAction::SaveToStatus { a, b } => status
                .insert_item(
                    mapping_uid,
                    &a.uid,
                    &a.hash,
                    &a.to_item_ref(),
                    &b.to_item_ref(),
                )
                .map(|()| Ok(())),
            ItemAction::UpdateStatus {
                hash,
                old_a,
                old_b,
                new_a,
                new_b,
            } => status.update_item(hash, old_a, old_b, new_a, new_b).map(Ok),
            ItemAction::ClearStatus { uid } => {
                status.delete_item(mapping_uid, uid).map(|()| Ok(()))
            }
            ItemAction::CreateInB { source } => {
                create_item(source, status, col_b, a, b, mapping_uid, Side::B).await
            }
            ItemAction::UpdateInB {
                source,
                target,
                old_a,
                old_b,
            } => update_item(a, b, source, target, old_a, old_b, status, Side::B).await,
            ItemAction::CreateInA { source } => {
                create_item(source, status, col_a, b, a, mapping_uid, Side::A).await
            }
            ItemAction::UpdateInA {
                source,
                target,
                old_a,
                old_b,
            } => update_item(b, a, source, target, old_a, old_b, status, Side::A).await,
            ItemAction::Delete { side, target } => {
                let storage = if *side == Side::A { a } else { b };
                delete_item(target, status, storage, mapping_uid).await
            }
            ItemAction::Conflict { a, .. } => {
                error!("Conflict for items {}. Skipping.", a.uid);
                Ok(Ok(()))
            }
        }
    }
}

/// Error during execution of a synchronisation [`Plan`]. See [`SyncError`].
#[derive(thiserror::Error, Debug)]
pub enum ExecutionError {
    #[error("collection missing from status when creating item")]
    MissingCollection,
    #[error("storage operation returned error: {0}")]
    Storage(#[from] crate::Error),
    #[error("created collection {1} on side {0:?} does not have the expected id, it has: {2:?}")]
    IdMismatch(Side, Href, Option<CollectionId>),
}

async fn create_item<I: Item>(
    source: &ItemState,
    status: &StatusDatabase,
    target_collection: &Href,
    src_storage: &dyn Storage<I>,
    dst_storage: &dyn Storage<I>,
    mapping_uid: &MappingUid,
    side: Side,
) -> Result<Result<(), ExecutionError>, StatusError> {
    debug!("Creating item from {}", source.href);

    let (item_data, source_etag) = match src_storage.get_item(&source.href).await {
        Ok((i, e)) => (i, e),
        Err(err) => return Ok(Err(ExecutionError::Storage(err))),
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

#[allow(clippy::too_many_arguments)]
async fn update_item<I: Item>(
    src_storage: &dyn Storage<I>,
    dst_storage: &dyn Storage<I>,
    source: &Href,
    target: &Href,
    old_a: &ItemRef,
    old_b: &ItemRef,
    status: &StatusDatabase,
    side: Side,
) -> Result<Result<(), ExecutionError>, StatusError> {
    debug!("Updating from {}", source);
    let (source_item, source_etag) = match src_storage.get_item(source).await {
        Ok(i) => i,
        Err(err) => return Ok(Err(ExecutionError::Storage(err))),
    };

    let old_etag = match side {
        Side::A => &old_a.etag,
        Side::B => &old_b.etag,
    };

    let new_etag = match dst_storage
        .update_item(target, old_etag, &source_item)
        .await
    {
        Ok(i) => i,
        Err(err) => return Ok(Err(ExecutionError::Storage(err))),
    };

    let hash = source_item.hash();
    match side {
        Side::A => status.update_item(
            &hash,
            old_a,
            old_b,
            &ItemRef {
                href: target.clone(),
                etag: new_etag,
            },
            &ItemRef {
                href: source.clone(),
                etag: source_etag,
            },
        ),
        Side::B => status.update_item(
            &hash,
            old_a,
            old_b,
            &ItemRef {
                href: source.clone(),
                etag: source_etag,
            },
            &ItemRef {
                href: target.clone(),
                etag: new_etag,
            },
        ),
    }?;

    Ok(Ok(()))
}

async fn delete_item<I: Item>(
    target: &ItemState,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    mapping_uid: &MappingUid,
) -> Result<Result<(), ExecutionError>, StatusError> {
    debug!("Deleting {}", target.href);
    match storage.delete_item(&target.href, &target.etag).await {
        Ok(()) => Ok(Ok(status.delete_item(mapping_uid, &target.uid)?)),
        Err(err) => Ok(Err(ExecutionError::Storage(err))),
    }
}

async fn delete_collection<I: Item>(
    href: &Href,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    mapping_uid: &MappingUid,
) -> Result<Result<(), ExecutionError>, StatusError> {
    match storage.destroy_collection(href).await {
        Ok(()) => Ok(Ok(status.remove_collection(mapping_uid)?)),
        Err(err) => Ok(Err(ExecutionError::Storage(err))),
    }
}

impl<I: Item> Plan<I> {
    /// Executes a synchronization plan.
    ///
    /// # Non-fatal errors
    ///
    /// When a non-fatal error occurs (e.g.: an item being uploaded is rejected), the `on_error`
    /// function will be called with details on the exact error.
    ///
    /// # Errors
    ///
    /// A [`StatusError`] is returned in case writing to the status database fails.
    pub async fn execute(
        self,
        status: &StatusDatabase,
        on_error: impl Fn(SyncError),
    ) -> Result<(), StatusError> {
        let storage_a = self.storage_a.as_ref();
        let storage_b = self.storage_b.as_ref();

        for plan in self.collection_plans {
            let CollectionPlan {
                collection_action,
                item_actions,
                property_actions,
                mapping,
            } = plan;
            let ResolvedMapping { alias, a, b } = mapping;

            let (mapping_uid, side_to_delete) = match collection_action
                .clone() // FIXME: cloning is a bit of a hack here
                .execute(status, &a.href, &b.href, a.id, b.id, storage_a, storage_b)
                .await?
            {
                Ok((m, s)) => (m, s),
                Err(err) => {
                    on_error(SyncError::collection(collection_action, alias, err));
                    continue;
                }
            };

            for item_action in item_actions {
                if let Err(err) = item_action
                    .execute(storage_a, storage_b, &a.href, &b.href, status, &mapping_uid)
                    .await?
                {
                    on_error(SyncError::item(item_action, err));
                };
            }

            for prop_action in property_actions {
                if let Err(err) = prop_action
                    // FIXME: won't work for item properties
                    .execute(storage_a, storage_b, status, &mapping_uid, &a.href, &b.href)
                    .await?
                {
                    on_error(SyncError::property(prop_action.action, err));
                };
            }

            match side_to_delete {
                None => {}
                Some(Side::A) => {
                    if let Err(err) =
                        delete_collection(&a.href, status, storage_a, &mapping_uid).await?
                    {
                        let action = CollectionAction::Delete(mapping_uid, Side::A);
                        on_error(SyncError::collection(action, alias, err));
                    };
                }
                Some(Side::B) => {
                    if let Err(err) =
                        delete_collection(&b.href, status, storage_b, &mapping_uid).await?
                    {
                        let action = CollectionAction::Delete(mapping_uid, Side::B);
                        on_error(SyncError::collection(action, alias, err));
                    };
                }
            };
        }

        // TODO: should flush state for any collections that are stale.

        Ok(())
    }
}

impl CollectionAction {
    /// Execute this collection's action.
    ///
    /// Returns the [`MappingUid`] for this collection and the side that needs to be deleted, if
    /// any.
    #[allow(clippy::too_many_arguments)]
    async fn execute<I: Item>(
        self,
        status: &StatusDatabase,
        href_a: &Href,
        href_b: &Href,
        id_a: Option<CollectionId>,
        id_b: Option<CollectionId>,
        storage_a: &dyn Storage<I>,
        storage_b: &dyn Storage<I>,
    ) -> Result<Result<(MappingUid, Option<Side>), ExecutionError>, StatusError> {
        match self {
            CollectionAction::NoAction(mapping_uid) => Ok(Ok((mapping_uid, None))),
            CollectionAction::SaveToStatus => status
                .get_or_add_collection(href_a, href_b, id_a.as_ref(), id_b.as_ref())
                .map(|mu| Ok((mu, None))),
            CollectionAction::CreateInB => create_collection(
                storage_b,
                href_b,
                href_a,
                status,
                Side::B,
                id_b.as_ref(),
                id_a.as_ref(),
            )
            .await
            .map(|r| r.map(|mu| (mu, None))),
            CollectionAction::CreateInA => create_collection(
                storage_a,
                href_a,
                href_b,
                status,
                Side::A,
                id_a.as_ref(),
                id_b.as_ref(),
            )
            .await
            .map(|r| r.map(|mu| (mu, None))),
            CollectionAction::CreateInBoth => create_both_collections(
                storage_a,
                storage_b,
                href_a,
                href_b,
                status,
                id_a.as_ref(),
                id_b.as_ref(),
            )
            .await
            .map(|r| r.map(|mu| (mu, None))),
            CollectionAction::Delete(mapping, side) => Ok(Ok((mapping, Some(side)))),
        }
    }
}

/// Creates a collection and updates the state and error list accordingly.
async fn create_collection<I: Item>(
    storage: &dyn Storage<I>,
    href: &Href,
    opposite_href: &Href,
    status: &StatusDatabase,
    side: Side,
    expected_id: Option<&CollectionId>,
    opposite_id: Option<&CollectionId>,
) -> Result<Result<MappingUid, ExecutionError>, StatusError> {
    let new_collection = match storage.create_collection(href).await {
        Ok(c) => c,
        Err(err) => return Ok(Err(ExecutionError::Storage(err))),
    };
    let new_href = new_collection.href();

    match check_id_matches_expected(expected_id, storage, new_href, side).await {
        Ok(()) => (),
        Err(err) => return Ok(Err(err)),
    };
    let mapping_uid = match side {
        Side::A => status.get_or_add_collection(href, opposite_href, expected_id, opposite_id),
        Side::B => status.get_or_add_collection(opposite_href, href, opposite_id, expected_id),
    }?;
    Ok(Ok(mapping_uid))
}

async fn create_both_collections<I: Item>(
    storage_a: &dyn Storage<I>,
    storage_b: &dyn Storage<I>,
    href_a: &Href,
    href_b: &Href,
    status: &StatusDatabase,
    id_a: Option<&CollectionId>,
    id_b: Option<&CollectionId>,
) -> Result<Result<MappingUid, ExecutionError>, StatusError> {
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
        let created_id = disco
            .collections()
            .iter()
            .find(|c| c.href() == collection)
            .map(DiscoveredCollection::id);
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

impl<I: Item> PropertyPlan<I> {
    async fn execute(
        &self,
        a: &dyn Storage<I>,
        b: &dyn Storage<I>,
        status: &StatusDatabase,
        mapping_uid: &MappingUid,
        href_a: &str,
        href_b: &str,
    ) -> Result<Result<(), ExecutionError>, StatusError> {
        match &self.action {
            super::plan::PropertyAction::WriteToA { value } => {
                if let Err(err) = a.set_property(href_a, self.property.clone(), value).await {
                    return Ok(Err(ExecutionError::from(err)));
                };
                status.set_property(mapping_uid, href_a, href_b, &self.property.name(), value)?;
            }
            super::plan::PropertyAction::WriteToB { value } => {
                if let Err(err) = b.set_property(href_b, self.property.clone(), value).await {
                    return Ok(Err(ExecutionError::from(err)));
                };
                status.set_property(mapping_uid, href_a, href_b, &self.property.name(), value)?;
            }
            super::plan::PropertyAction::DeleteInA => {
                if let Err(err) = a.unset_property(href_a, self.property.clone()).await {
                    return Ok(Err(ExecutionError::from(err)));
                };
                status.delete_property(
                    mapping_uid,
                    href_a,
                    href_b,
                    self.property.name().as_str(),
                )?;
            }
            super::plan::PropertyAction::DeleteInB => {
                if let Err(err) = b.unset_property(href_b, self.property.clone()).await {
                    return Ok(Err(ExecutionError::from(err)));
                };
                status.delete_property(
                    mapping_uid,
                    href_a,
                    href_b,
                    self.property.name().as_str(),
                )?;
            }
            super::plan::PropertyAction::ClearStatus => {
                status.delete_property(
                    mapping_uid,
                    href_a,
                    href_b,
                    self.property.name().as_str(),
                )?;
            }
            super::plan::PropertyAction::UpdateStatus { value } => {
                status.set_property(mapping_uid, href_a, href_b, &self.property.name(), value)?;
            }
            super::plan::PropertyAction::Conflict => {
                error!("Conflict for property {}. Skipping.", self.property.name());
            }
        };
        Ok(Ok(()))
    }
}
