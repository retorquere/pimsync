// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Plan::execute`](Plan::execute).

use log::{debug, error};

use crate::{
    base::{Item, ItemRef, Storage},
    disco::DiscoveredCollection,
    sync::status::ItemState,
    CollectionId, Href,
};

use super::{
    plan::{CollectionAction, CollectionPlan, ItemAction, Plan},
    status::{MappingUid, Side, StatusDatabase, StatusError},
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
    ) -> Result<(), ExecutionError> {
        {
            match self {
                ItemAction::SaveToState { a, b } => {
                    status.insert_item(
                        mapping_uid,
                        &a.uid,
                        &a.hash,
                        &a.to_item_ref(),
                        &b.to_item_ref(),
                    )?;
                }
                ItemAction::ClearState { uid } => {
                    status.delete_item(mapping_uid, uid)?;
                }
                ItemAction::CreateInB { source } => {
                    create_item(source, status, col_b, a, b, mapping_uid, Side::B).await?;
                }
                ItemAction::UpdateInB { source, target } => {
                    update_item(source, target, status, a, b, Side::B).await?;
                }
                ItemAction::CreateInA { source } => {
                    create_item(source, status, col_a, b, a, mapping_uid, Side::A).await?;
                }
                ItemAction::UpdateInA { source, target } => {
                    update_item(source, target, status, b, a, Side::A).await?;
                }
                ItemAction::DeleteInA { target } => {
                    delete_item(target, status, a, mapping_uid).await?;
                }
                ItemAction::DeleteInB { target } => {
                    delete_item(target, status, b, mapping_uid).await?;
                }
                ItemAction::Conflict { uid } => {
                    error!("Conflict for items {}. Skipping.", uid);
                }
            };
            Ok(())
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ExecutionError {
    #[error("collection missing from status when creating item")]
    MissingCollection,
    #[error("error querying status database")]
    StatusDb(#[from] StatusError),
    #[error("error interacting with storage")]
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
) -> Result<(), ExecutionError> {
    debug!("Creating item from {}", source.href);

    let (item_data, source_etag) = src_storage.get_item(&source.href).await?;
    let uid = item_data.ident();
    let new_item = dst_storage.add_item(target_collection, &item_data).await?;

    // The original Etag MAY have changed.
    let source_ref = ItemRef {
        href: source.href.clone(),
        etag: source_etag,
    };

    match side {
        Side::A => status.insert_item(mapping_uid, &uid, &item_data.hash(), &new_item, &source_ref),
        Side::B => status.insert_item(mapping_uid, &uid, &item_data.hash(), &source_ref, &new_item),
    }?;

    Ok(())
}

async fn update_item<I: Item>(
    source: &ItemState,
    target: &ItemRef,
    status: &StatusDatabase,
    src_storage: &dyn Storage<I>,
    dst_storage: &dyn Storage<I>,
    side: Side,
) -> Result<(), ExecutionError> {
    debug!("Updating {}", target.href);
    let (item, source_etag) = src_storage.get_item(&source.href).await?;

    let new_etag = dst_storage
        .update_item(&target.href, &target.etag, &item)
        .await?;

    let hash = item.hash();
    match side {
        Side::A => status.update_item(&hash, &new_etag, &target.href, &source_etag, &source.href),
        Side::B => status.update_item(&hash, &source_etag, &source.href, &new_etag, &target.href),
    }?;

    Ok(())
}

async fn delete_item<I: Item>(
    target: &ItemState,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    mapping_uid: &MappingUid,
) -> Result<(), ExecutionError> {
    debug!("Deleting {}", target.href);
    storage.delete_item(&target.href, &target.etag).await?;
    status.delete_item(mapping_uid, &target.uid)?;

    Ok(())
}

async fn delete_collection<I: Item>(
    href: &Href,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    mapping_uid: &MappingUid,
) -> Result<(), ExecutionError> {
    storage.destroy_collection(href).await?;
    status.remove_collection(mapping_uid)?;

    Ok(())
}

impl<I: Item> Plan<I> {
    /// Executes a synchronization plan.
    ///
    /// Always returns a final state, regardless of what changes were applied. The returned value
    /// will include any errors that occurred during synchronisation. If any errors exist, then
    /// both storage may still be  out of sync.
    #[allow(clippy::too_many_lines)]
    // TODO: should take a channel/sender where errors can be sent.
    //       implementations can handle this as they prefer
    pub async fn execute(self, status: &StatusDatabase) -> SyncResult {
        // FIXME: shouldn't we bail immediately if status fails to write?
        let mut errors = Vec::new();
        let storage_a = self.storage_a.as_ref();
        let storage_b = self.storage_b.as_ref();

        for plan in self.collection_plans {
            let mut side_to_delete = None;
            let CollectionPlan {
                collection_action,
                item_actions,
                href_a,
                href_b,
                id_a,
                id_b,
            } = plan;

            let mapping_uid = match collection_action {
                CollectionAction::NoAction(mapping_uid) => mapping_uid,
                CollectionAction::SaveToStatus => {
                    match status.get_or_add_collection(
                        &href_a,
                        &href_b,
                        id_a.as_ref(),
                        id_b.as_ref(),
                    ) {
                        Ok(mapping_uid) => mapping_uid,
                        Err(err) => {
                            errors.push(SynchronizationError::new(collection_action, err.into()));
                            continue;
                        }
                    }
                }
                CollectionAction::CreateInB => {
                    match create_collection(
                        storage_b,
                        &href_b,
                        &href_a,
                        status,
                        Side::B,
                        id_b.as_ref(),
                        id_a.as_ref(),
                    )
                    .await
                    {
                        Ok(mapping_uid) => mapping_uid,
                        Err(e) => {
                            errors.push(SynchronizationError::new(collection_action, e));
                            continue;
                        }
                    }
                }
                CollectionAction::CreateInA => {
                    match create_collection(
                        storage_a,
                        &href_a,
                        &href_b,
                        status,
                        Side::A,
                        id_a.as_ref(),
                        id_b.as_ref(),
                    )
                    .await
                    {
                        Ok(mapping_uid) => mapping_uid,
                        Err(e) => {
                            errors.push(SynchronizationError::new(collection_action, e));
                            continue;
                        }
                    }
                }
                CollectionAction::CreateInBoth => {
                    match create_both_collections(
                        storage_a,
                        storage_b,
                        &href_a,
                        &href_b,
                        status,
                        id_a.as_ref(),
                        id_b.as_ref(),
                    )
                    .await
                    {
                        Ok(mapping_uid) => mapping_uid,
                        Err(e) => {
                            errors.push(SynchronizationError::new(collection_action, e));
                            continue;
                        }
                    }
                }
                CollectionAction::Delete(mapping, side) => {
                    side_to_delete = Some(side);
                    mapping
                }
            };

            for item_action in item_actions {
                if let Err(err) = item_action
                    .execute(storage_a, storage_b, &href_a, &href_b, status, &mapping_uid)
                    .await
                {
                    errors.push(SynchronizationError::new(item_action, err));
                };
            }

            match side_to_delete {
                None => {}
                Some(Side::A) => {
                    if let Err(err) =
                        delete_collection(&href_a, status, storage_a, &mapping_uid).await
                    {
                        let action = CollectionAction::Delete(mapping_uid, Side::A);
                        errors.push(SynchronizationError::new(action, err));
                    };
                }
                Some(Side::B) => {
                    if let Err(err) =
                        delete_collection(&href_b, status, storage_b, &mapping_uid).await
                    {
                        let action = CollectionAction::Delete(mapping_uid, Side::B);
                        errors.push(SynchronizationError::new(action, err));
                    };
                }
            };
        }

        // TODO: should flush state for any collections that are stale.

        SyncResult { errors }
    }
}

/// The result of executing a synchronisation.
///
/// Storages may have been mutated before an error occurred, so the final state for both is always
/// returned, even in case of an error.
#[must_use]
#[derive(Debug)]
pub struct SyncResult {
    /// Any errors that may have occurred during synchronisation.
    errors: Vec<SynchronizationError>,
}

impl SyncResult {
    /// Returns `true` if both storages are in sync.
    #[must_use]
    pub fn synchronised_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Errors that occurred during synchronisation, if any.
    #[must_use]
    pub fn errors(&self) -> &[SynchronizationError] {
        &self.errors
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
) -> Result<MappingUid, ExecutionError> {
    let new_collection = storage.create_collection(href).await?;
    let new_href = new_collection.href();

    check_id_matches_expected(expected_id, storage, new_href, side).await?;
    let mapping_uid = match side {
        Side::A => status.get_or_add_collection(href, opposite_href, expected_id, opposite_id),
        Side::B => status.get_or_add_collection(opposite_href, href, opposite_id, expected_id),
    }?;
    Ok(mapping_uid)
}

async fn create_both_collections<I: Item>(
    storage_a: &dyn Storage<I>,
    storage_b: &dyn Storage<I>,
    href_a: &Href,
    href_b: &Href,
    status: &StatusDatabase,
    id_a: Option<&CollectionId>,
    id_b: Option<&CollectionId>,
) -> Result<MappingUid, ExecutionError> {
    let new_a = storage_a.create_collection(href_a).await?;
    check_id_matches_expected(id_a, storage_a, new_a.href(), Side::A).await?;
    let new_b = storage_b.create_collection(href_b).await?;
    check_id_matches_expected(id_b, storage_b, new_b.href(), Side::B).await?;

    status
        .get_or_add_collection(href_a, href_b, id_a, id_b)
        .map_err(ExecutionError::StatusDb)
}

#[derive(Debug)]
pub enum SomeAction {
    Item(Box<ItemAction>),
    // TODO: this is missing the details of the collection itself (e.g.: alias?).
    Collection(CollectionAction),
}

impl From<ItemAction> for SomeAction {
    fn from(item: ItemAction) -> Self {
        SomeAction::Item(Box::new(item))
    }
}

impl From<CollectionAction> for SomeAction {
    fn from(collection: CollectionAction) -> Self {
        SomeAction::Collection(collection)
    }
}

/// An error synchronising two items between storages.
#[derive(Debug)]
pub struct SynchronizationError {
    action: SomeAction,
    error: ExecutionError,
}

impl SynchronizationError {
    #[must_use]
    pub fn new(action: impl Into<SomeAction>, error: ExecutionError) -> Self {
        Self {
            action: action.into(),
            error,
        }
    }

    /// Action that failed to execute.
    #[must_use]
    pub fn action(&self) -> &SomeAction {
        &self.action
    }

    /// Underlying error during the operation.
    #[must_use]
    pub fn error(&self) -> &ExecutionError {
        &self.error
    }
}

impl std::fmt::Display for SynchronizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error executing {:?}: {}", self.action, self.error)
    }
}

impl std::error::Error for SynchronizationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
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
