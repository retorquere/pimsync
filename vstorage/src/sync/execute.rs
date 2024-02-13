// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Plan::execute`](Plan::execute).

use std::borrow::Cow;

use log::{debug, error};

use crate::{
    base::{Item, ItemRef, Storage},
    sync::{plan::Action, status::ItemState},
    Href,
};

use super::{
    plan::{CollectionAction, ItemAction, Plan},
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
    pub async fn execute(self, status: &StatusDatabase) -> SyncResult {
        // FIXME: shouldn't we bail immediately if status fails to write?
        let mut errors = Vec::new();
        let storage_a = self.storage_a.as_ref();
        let storage_b = self.storage_b.as_ref();

        for cp in self.collection_plans {
            let mut deletion_action = None;
            let (_mapping, collection_action, item_actions) = cp.into_parts();
            let (href_a, href_b) = match collection_action {
                CollectionAction::NoAction { href_a, href_b } => {
                    (Cow::from(href_a), Cow::from(href_b))
                }
                CollectionAction::SaveToStatus {
                    ref href_a,
                    ref href_b,
                } => {
                    if let Err(err) =
                        save_collection_to_status(storage_a, href_a, storage_b, href_b, status)
                    {
                        errors.push(SynchronizationError::new(collection_action, err));
                        continue;
                    };
                    (Cow::from(href_a), Cow::from(href_b))
                }
                CollectionAction::CreateInB {
                    collection: ref c,
                    ref href_a,
                } => {
                    let href_b = match create_collection(storage_b, c, status, Side::B).await {
                        Ok(href) => href,
                        Err(e) => {
                            errors.push(SynchronizationError::new(collection_action, e));
                            continue;
                        }
                    };
                    (Cow::from(href_a), Cow::from(href_b))
                }
                CollectionAction::CreateInA {
                    collection: ref c,
                    ref href_b,
                } => {
                    let href_a = match create_collection(storage_a, c, status, Side::A).await {
                        Ok(href) => href,
                        Err(e) => {
                            errors.push(SynchronizationError::new(collection_action, e));
                            continue;
                        }
                    };
                    (Cow::from(href_a), Cow::from(href_b))
                }
                CollectionAction::DeleteInA {
                    ref href,
                    ref deleted_href,
                } => {
                    let hrefs = (Cow::from(href.clone()), Cow::from(deleted_href.clone()));
                    deletion_action = Some(collection_action);
                    hrefs
                }
                CollectionAction::DeleteInB {
                    ref href,
                    ref deleted_href,
                } => {
                    let hrefs = (Cow::from(deleted_href.clone()), Cow::from(href.clone()));
                    deletion_action = Some(collection_action);
                    hrefs
                }
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
                    CollectionAction::DeleteInA {
                        ref href,
                        ref deleted_href,
                    } => {
                        if let Err(e) =
                            delete_collection(href, status, storage_a, Side::A, deleted_href).await
                        {
                            errors.push(SynchronizationError::new(action, e));
                        };
                    }
                    CollectionAction::DeleteInB {
                        ref href,
                        ref deleted_href,
                    } => {
                        if let Err(e) =
                            delete_collection(href, status, storage_b, Side::B, deleted_href).await
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

fn save_collection_to_status<I: Item>(
    storage_a: &dyn Storage<I>,
    href_a: &str,
    storage_b: &dyn Storage<I>,
    href_b: &str,
    status: &StatusDatabase,
) -> Result<(), ExecutionError> {
    let id_a = storage_a.collection_id(href_a)?;
    let id_b = storage_b.collection_id(href_b)?;

    status.add_collection(Side::A, &id_a, href_a)?;
    status.add_collection(Side::B, &id_b, href_b)?;
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
    collection: &Href,
    status: &StatusDatabase,
    side: Side,
) -> Result<String, ExecutionError> {
    let new_collection = storage.create_collection(collection).await?;

    // FIXME: this probably should never error, but I might need to consider making Id optional for
    // collections. Also, I don't think we should save the Id if the collection won't later show up
    // in discovery.
    let id = storage.collection_id(new_collection.href())?;
    status.add_collection(side, &id, new_collection.href())?;
    Ok(new_collection.into_href())
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
