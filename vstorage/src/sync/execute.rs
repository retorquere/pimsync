// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Plan::execute`](Plan::execute).

use std::borrow::Cow;

use log::{debug, error};

use crate::{
    base::{Item, ItemRef, Storage},
    sync::{plan::Action, status::ItemState},
    Etag, Href,
};

use super::{
    plan::{CollectionAction, ItemAction, Plan, ResolvedCollection, ResolvedMapping},
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
        mapping: &ResolvedMapping,
        status: &StatusDatabase,
    ) -> std::result::Result<(), ExecutionError> {
        {
            match self.action() {
                Action::SaveToState { a, b } => {
                    status.add_item(Side::A, a)?;
                    status.add_item(Side::B, b)?;
                }
                Action::CreateInB { source } => {
                    let collection = mapping.collection_b();
                    create_item(source, status, collection, a, b, Side::B).await?;
                }
                Action::UpdateInB { source, target } => {
                    update_item(source, target, status, a, b, Side::B).await?;
                }
                Action::CreateInA { source } => {
                    let collection = mapping.collection_a();
                    create_item(source, status, collection, b, a, Side::A).await?;
                }
                Action::UpdateInA { source, target } => {
                    update_item(source, target, status, b, a, Side::A).await?;
                }
                Action::DeleteInA { href, etag } => {
                    delete_item(href, etag, status, a, Side::A).await?;
                }
                Action::DeleteInB { href, etag } => {
                    delete_item(href, etag, status, b, Side::B).await?;
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
}

async fn create_item<I: Item>(
    from: &Href,
    status: &StatusDatabase,
    collection: &ResolvedCollection,
    src_storage: &dyn Storage<I>,
    dst_storage: &dyn Storage<I>,
    side: Side,
) -> Result<(), ExecutionError> {
    debug!("Creating item from {from}");

    let collection_href = match collection {
        ResolvedCollection::Id { id } => Cow::Owned(
            status
                .get_collection_href(side, id)?
                .ok_or(ExecutionError::MissingCollection)?,
        ),
        ResolvedCollection::Href { href } => Cow::Borrowed(href),
    };

    let (item_data, _) = src_storage.get_item(from).await?;
    let uid = item_data.ident();
    let new_item = dst_storage.add_item(&collection_href, &item_data).await?;

    status.add_item(
        side,
        &ItemState {
            href: new_item.href,
            uid,
            etag: new_item.etag,
            hash: item_data.hash(),
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
    let (item, _) = src_storage.get_item(src_href).await?;

    let new_etag = dst_storage
        .update_item(&target.href, &target.etag, &item)
        .await?;
    status.update_item(side, &new_etag, &item.hash(), &target.href)?;

    Ok(())
}

async fn delete_item<I: Item>(
    href: &Href,
    etag: &Etag,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    side: Side,
) -> Result<(), ExecutionError> {
    storage.delete_item(href, etag).await?;
    status.delete_item(side, href)?;

    Ok(())
}

async fn delete_collection<I: Item>(
    href: &Href,
    status: &StatusDatabase,
    storage: &dyn Storage<I>,
    side: Side,
) -> Result<(), ExecutionError> {
    storage.destroy_collection(href).await?;
    status.remove_collection(side, href)?;

    Ok(())
}

impl<'pair, I: Item> Plan<'pair, I> {
    /// Executes a synchronization plan.
    ///
    /// Always returns a final state, regardless of what changes were applied. The returned value
    /// will include any errors that occurred during synchronisation. If any errors exist, then
    /// both storage may still be  out of sync.
    pub async fn execute(self, status: &StatusDatabase) -> SyncResult {
        // FIXME: shouldn't we bail immediately if status fails to write?
        let mut errors = Vec::new();
        let storage_a = self.pair.storage_a.as_ref();
        let storage_b = self.pair.storage_b.as_ref();

        for cp in self.collection_plans {
            let mut deletion_action = None;
            let (mapping, collection_action, item_actions) = cp.into_parts();
            if let Some(action) = collection_action {
                match action {
                    CollectionAction::CreateInB { collection: ref c } => {
                        if let Err(e) = create_collection(storage_b, c, status, Side::B).await {
                            errors.push(SynchronizationError::new(action, e));
                        };
                    }
                    CollectionAction::CreateInA { collection: ref c } => {
                        if let Err(e) = create_collection(storage_a, c, status, Side::B).await {
                            errors.push(SynchronizationError::new(action, e));
                        }
                    }
                    CollectionAction::DeleteInA { .. } | CollectionAction::DeleteInB { .. } => {
                        deletion_action = Some(action);
                    }
                }
            }

            for item_action in item_actions {
                if let Err(err) = item_action
                    .execute(storage_a, storage_b, &mapping, status)
                    .await
                {
                    errors.push(SynchronizationError::new(item_action, err));
                };
            }

            if let Some(action) = deletion_action {
                match action {
                    CollectionAction::DeleteInA { ref href } => {
                        if let Err(e) = delete_collection(href, status, storage_a, Side::A).await {
                            errors.push(SynchronizationError::new(action, e));
                        };
                    }
                    CollectionAction::DeleteInB { ref href } => {
                        if let Err(e) = delete_collection(href, status, storage_b, Side::B).await {
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
    collection: &ResolvedCollection,
    status: &StatusDatabase,
    side: Side,
) -> Result<(), ExecutionError> {
    let creation_result = match collection {
        ResolvedCollection::Id { id } => storage.create_collection_with_id(id).await,
        ResolvedCollection::Href { href } => storage.create_collection(href).await,
    };

    let new_collection = creation_result?;

    // FIXME: panics
    // It doesn't make sense that this error would ever happen. It implies a collection was
    // created, but the `href` is not valid and a collection_id cannot be resolved.
    let id = storage.collection_id(new_collection.href()).unwrap();
    status.add_collection(side, &id, new_collection.href())?;
    Ok(())
}

#[derive(Debug)]
pub enum SomeAction {
    Item(ItemAction),
    Collection(CollectionAction),
}

impl From<ItemAction> for SomeAction {
    fn from(item: ItemAction) -> Self {
        SomeAction::Item(item)
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
