// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Plan::execute`](Plan::execute).

use log::{debug, error};

use crate::{
    base::{Item, ItemRef, Storage},
    disco::DiscoveredCollection,
    sync::{plan::Action, status::ItemState},
    CollectionId, Href,
};

use super::{
    plan::{CollectionAction, CollectionPlan, ItemAction, Plan},
    status::{Side, StatusDatabase, StatusError},
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
        col_a: &str,
        col_b: &str,
        status: &StatusDatabase,
    ) -> Result<(), ExecutionError> {
        {
            match self.action() {
                Action::SaveToState { a, b } => {
                    status.add_item(Side::A, col_a, a)?;
                    status.add_item(Side::B, col_b, b)?;
                }
                Action::ClearState => {
                    status.delete_item(self.uid())?;
                }
                Action::CreateInB { source } => {
                    create_item(source, status, col_a, col_b, a, b, Side::B).await?;
                }
                Action::UpdateInB { source, target } => {
                    update_item(source, target, status, a, b, Side::B).await?;
                }
                Action::CreateInA { source } => {
                    create_item(source, status, col_b, col_a, b, a, Side::A).await?;
                }
                Action::UpdateInA { source, target } => {
                    update_item(source, target, status, b, a, Side::A).await?;
                }
                Action::DeleteInA { target } => {
                    delete_item(target, status, a, self.uid()).await?;
                }
                Action::DeleteInB { target } => {
                    delete_item(target, status, b, self.uid()).await?;
                }
                Action::Conflict => {
                    error!("Conflict for items {}. Skipping.", self.uid());
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
    from: &Href,
    status: &StatusDatabase,
    source_collection: &str,
    target_collection: &str,
    src_storage: &dyn Storage<I>,
    dst_storage: &dyn Storage<I>,
    side: Side,
) -> Result<(), ExecutionError> {
    debug!("Creating item from {from}");

    let (item_data, source_etag) = src_storage.get_item(from).await?;
    let uid = item_data.ident();
    let new_item = dst_storage.add_item(target_collection, &item_data).await?;
    let hash = item_data.hash();

    // Save new item.
    status.add_item(
        side,
        target_collection,
        &ItemState {
            href: new_item.href,
            uid: uid.clone(),
            etag: new_item.etag.clone(),
            hash: hash.clone(),
        },
    )?;
    // Also save source item.
    status.add_item(
        side.opposite(),
        source_collection,
        &ItemState {
            href: from.to_string(),
            uid,
            etag: source_etag,
            hash,
        },
    )?;

    Ok(())
}

async fn update_item<I: Item>(
    src_href: &Href,
    target: &ItemRef,
    status: &StatusDatabase,
    src_storage: &dyn Storage<I>,
    dst_storage: &dyn Storage<I>,
    side: Side,
) -> Result<(), ExecutionError> {
    debug!("Updating {}", target.href);
    let (item, source_etag) = src_storage.get_item(src_href).await?;

    let new_etag = dst_storage
        .update_item(&target.href, &target.etag, &item)
        .await?;

    let hash = item.hash();
    status.update_item(side, &new_etag, &hash, &target.href)?;
    status.update_item(side.opposite(), &source_etag, &hash, src_href)?;

    Ok(())
}

async fn delete_item<I: Item>(
    item_ref: &ItemRef,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    uid: &str,
) -> Result<(), ExecutionError> {
    debug!("Deleting {}", item_ref.href);
    storage.delete_item(&item_ref.href, &item_ref.etag).await?;
    status.delete_item(uid)?;

    Ok(())
}

async fn delete_collection<I: Item>(
    href: &Href,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    side: Side,
    opposite_href: &Href,
) -> Result<(), ExecutionError> {
    storage.destroy_collection(href).await?;
    status.remove_collection(side, href)?;
    status.remove_collection(side.opposite(), opposite_href)?;

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
            let mut deletion_action = None;
            let CollectionPlan {
                collection_action,
                item_actions,
                href_a,
                href_b,
                id_a,
                id_b,
                ..
            } = plan;

            if let Some(action) = collection_action {
                match action {
                    CollectionAction::SaveToStatus => {
                        if let Err(err) = save_collection_to_status(
                            &href_a,
                            &href_b,
                            id_a.as_ref(),
                            id_b.as_ref(),
                            status,
                        ) {
                            errors.push(SynchronizationError::new(action, err));
                            continue;
                        };
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
                            Ok(href) => href,
                            Err(e) => {
                                errors.push(SynchronizationError::new(action, e));
                                continue;
                            }
                        };
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
                            Ok(href) => href,
                            Err(e) => {
                                errors.push(SynchronizationError::new(action, e));
                                continue;
                            }
                        };
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
                            Ok(href) => href,
                            Err(e) => {
                                errors.push(SynchronizationError::new(action, e));
                                continue;
                            }
                        };
                    }
                    CollectionAction::DeleteInA | CollectionAction::DeleteInB => {
                        deletion_action = Some(action);
                    }
                };
            };

            for item_action in item_actions {
                if let Err(err) = item_action
                    .execute(storage_a, storage_b, &href_a, &href_b, status)
                    .await
                {
                    errors.push(SynchronizationError::new(item_action, err));
                };
            }

            if let Some(action) = deletion_action {
                match action {
                    CollectionAction::DeleteInA => {
                        if let Err(e) =
                            delete_collection(&href_a, status, storage_a, Side::A, &href_b).await
                        {
                            errors.push(SynchronizationError::new(action, e));
                        };
                    }
                    CollectionAction::DeleteInB => {
                        if let Err(e) =
                            delete_collection(&href_b, status, storage_b, Side::B, &href_a).await
                        {
                            errors.push(SynchronizationError::new(action, e));
                        };
                    }
                    _ => unreachable!(),
                }
            }
        }

        SyncResult { errors }
    }
}

fn save_collection_to_status(
    href_a: &str,
    href_b: &str,
    id_a: Option<&CollectionId>,
    id_b: Option<&CollectionId>,
    status: &StatusDatabase,
) -> Result<(), ExecutionError> {
    // FIXME: I should save the hash of the resolved mapping.
    //        If it ever changes, it means the config has changed, and should bail.
    status.add_collection(Side::A, id_a, href_a)?;
    status.add_collection(Side::B, id_b, href_b)?;
    Ok(())
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
///
/// Returns the `href` of the collection.
async fn create_collection<I: Item>(
    storage: &dyn Storage<I>,
    href: &Href,
    opposite_href: &Href,
    status: &StatusDatabase,
    side: Side,
    expected_id: Option<&CollectionId>,
    opposite_id: Option<&CollectionId>,
) -> Result<String, ExecutionError> {
    let new_collection = storage.create_collection(href).await?;
    let collection = new_collection.href();

    check_id_matches_expected(expected_id, storage, collection, side).await?;
    status.add_collection(side, expected_id, collection)?;
    status.add_collection(side.opposite(), opposite_id, opposite_href)?;

    Ok(new_collection.into_href())
}

async fn create_both_collections<I: Item>(
    storage_a: &dyn Storage<I>,
    storage_b: &dyn Storage<I>,
    href_a: &str,
    href_b: &str,
    status: &StatusDatabase,
    id_a: Option<&CollectionId>,
    id_b: Option<&CollectionId>,
) -> Result<(String, String), ExecutionError> {
    let new_a = storage_a.create_collection(href_a).await?;
    check_id_matches_expected(id_a, storage_a, new_a.href(), Side::A).await?;
    status.add_collection(Side::A, id_a, new_a.href())?;

    let new_b = storage_b.create_collection(href_b).await?;
    check_id_matches_expected(id_b, storage_b, new_b.href(), Side::B).await?;
    status.add_collection(Side::B, id_b, new_b.href())?;

    Ok((new_a.into_href(), new_b.into_href()))
}

#[derive(Debug)]
pub enum SomeAction {
    Item(Box<ItemAction>),
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
    collection: &str,
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
