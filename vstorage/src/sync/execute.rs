// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Plan::execute`](Plan::execute).

use std::sync::Arc;

use log::{debug, error};

use crate::{
    base::{Item, Storage},
    sync::{plan::Action, state::ItemState},
    Href,
};

use super::{
    plan::{CollectionAction, ItemAction, Plan, ResolvedCollection},
    state::{CollectionState, PairState, StorageState},
};

impl ItemAction {
    /// Execution the action on the item.
    ///
    /// The `state_a` or `state_b` variables should only be `None` if the collection does not exist
    /// in that storage. That should only really happen if the storage has been deleted.
    #[inline]
    async fn execute<I: Item>(
        &self,
        storage_a: &Arc<dyn Storage<I>>,
        storage_b: &Arc<dyn Storage<I>>,
        state_a: Option<&mut CollectionState>,
        state_b: Option<&mut CollectionState>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        {
            match self.action() {
                Action::CopyToB { source } => {
                    copy_item(
                        source,
                        state_b.ok_or("collection missing from state b")?,
                        storage_a,
                        storage_b,
                        self.uid(),
                    )
                    .await?;
                }
                Action::CopyToA { source } => {
                    copy_item(
                        source,
                        state_a.ok_or("collection missing from state a")?,
                        storage_b,
                        storage_a,
                        self.uid(),
                    )
                    .await?;
                }
                Action::DeleteInA { href } => {
                    delete_item(
                        href,
                        state_a.ok_or("collection is missing from state a")?,
                        storage_a,
                        self.uid(),
                    )
                    .await?;
                }
                Action::DeleteInB { href } => {
                    delete_item(
                        href,
                        state_b.ok_or("collection is missing from state b")?,
                        storage_b,
                        self.uid(),
                    )
                    .await?;
                }
                Action::Conflict => {
                    error!("Conflict for items {}. Skipping.", self.uid());
                }
            };
            Ok(())
        }
    }
}

async fn copy_item<I: Item>(
    src_href: &Href,
    dst_state: &mut CollectionState,
    src_storage: &Arc<dyn Storage<I>>,
    dst_storage: &Arc<dyn Storage<I>>,
    uid: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (item, _) = src_storage.get_item(src_href).await?;

    if let Some(dst_item_state) = dst_state.get_item_by_uid_mut(uid) {
        debug!("Updating {uid}");
        let new_etag = dst_storage
            .update_item(&dst_item_state.href, &dst_item_state.etag, &item)
            .await?;
        dst_item_state.etag = new_etag;
        dst_item_state.hash = item.hash();
    } else {
        debug!("Creating {uid}");
        let new_ref = dst_storage.add_item(&dst_state.href, &item).await?;
        dst_state.items.push(ItemState {
            href: new_ref.href,
            uid: uid.to_string(),
            etag: new_ref.etag,
            hash: item.hash(),
        });
    };

    Ok(())
}

async fn delete_item<I: Item>(
    href: &Href,
    state: &mut CollectionState,
    storage: &Arc<dyn Storage<I>>,
    uid: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let pos = state
        .items
        .iter()
        .position(|i| i.uid == *uid)
        .ok_or("item pending deletion is missing from state")?;
    let item_state = &state.items[pos];

    storage.delete_item(href, &item_state.etag).await?;

    state.items.swap_remove(pos);

    Ok(())
}

impl<'pair, I: Item> Plan<'pair, I> {
    /// Executes a synchronization plan.
    ///
    /// Always returns a final state, regardless of what changes were applied. The returned value
    /// will include any errors that occurred during synchronisation. If any errors exist, then
    /// both storage may still be  out of sync.
    pub async fn execute(self) -> SyncResult {
        let mut final_state = self.current_state().clone();
        let mut errors = Vec::new();
        let storage_a = &self.pair.storage_a;
        let storage_b = &self.pair.storage_b;

        for cp in self.collection_plans {
            let mut deletion_action = None;
            let (mapping, collection_action, item_actions) = cp.into_parts();
            if let Some(action) = collection_action {
                match action {
                    CollectionAction::CreateInB { collection: ref c } => {
                        if let Err(e) = create_collection(storage_b, c, &mut final_state.b).await {
                            errors.push(SynchronizationError::new(action, e));
                        };
                    }
                    CollectionAction::CreateInA { collection: ref c } => {
                        if let Err(e) = create_collection(storage_a, c, &mut final_state.a).await {
                            errors.push(SynchronizationError::new(action, e));
                        }
                    }
                    CollectionAction::DeleteInA { .. } | CollectionAction::DeleteInB { .. } => {
                        deletion_action = Some(action);
                    }
                }
            }

            for item_action in item_actions {
                // FIXME: I need to somehow move these two calls outside of the "for" loop.
                let state_a = final_state
                    .a
                    .find_collection_state_mut(mapping.collection_a());
                let state_b = final_state
                    .b
                    .find_collection_state_mut(mapping.collection_b());

                if let Err(err) = item_action
                    .execute(storage_a, storage_b, state_a, state_b)
                    .await
                {
                    errors.push(SynchronizationError::new(item_action.action().clone(), err));
                };
            }

            if let Some(action) = deletion_action {
                match action {
                    CollectionAction::DeleteInA { ref href } => {
                        match storage_a.destroy_collection(href).await {
                            Ok(()) => final_state.a.remove_collection(href),
                            Err(e) => errors.push(SynchronizationError::new(action, e)),
                        };
                    }
                    CollectionAction::DeleteInB { ref href } => {
                        match storage_b.destroy_collection(href).await {
                            Ok(()) => final_state.b.remove_collection(href),
                            Err(e) => errors.push(SynchronizationError::new(action, e)),
                        };
                    }
                    _ => unreachable!(),
                }
            }
        }

        SyncResult {
            final_state,
            errors,
        }
    }
}

/// The result of executing a synchronisation.
///
/// Storages may have been mutated before an error occurred, so the final state for both is always
/// returned, even in case of an error.
#[must_use]
#[derive(Debug)]
pub struct SyncResult {
    /// The state of this pair after synchronisation.
    final_state: PairState,
    /// Any errors that may have occurred during synchronisation.
    errors: Vec<SynchronizationError>,
}

impl SyncResult {
    /// Returns `true` if both storages are in sync.
    #[must_use]
    pub fn synchronised_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// The state the pair of storages after synchronisation.
    ///
    /// This value should be persisted and supplied as a `previous_state` the next time this storage
    /// is synchronised.
    #[must_use]
    pub fn final_state(&self) -> &PairState {
        &self.final_state
    }

    /// Errors that occurred during synchronisation, if any.
    #[must_use]
    pub fn errors(&self) -> &[SynchronizationError] {
        &self.errors
    }
}

/// Creates a collection and updates the state and error list accordingly.
async fn create_collection<I: Item>(
    storage: &Arc<dyn Storage<I>>,
    collection: &ResolvedCollection,
    state: &mut StorageState,
) -> Result<(), crate::Error> {
    let creation_result = match collection {
        ResolvedCollection::Id { id } => storage.create_collection_with_id(id).await,
        ResolvedCollection::Href { href } => storage.create_collection(href).await,
    };

    match creation_result {
        Ok(col) => {
            // FIXME: panics
            // It doesn't make sense that this error would ever happen. It implies a collection was
            // created, but the `href` is not valid and a collection_id cannot be resolved.
            let id = storage.collection_id(col.href()).unwrap();
            state.add_collection(id, col.href().to_string());
            Ok(())
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug)]
pub enum SomeAction {
    Item(Action),
    Collection(CollectionAction),
}

impl From<Action> for SomeAction {
    fn from(item: Action) -> Self {
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
    error: Box<dyn std::error::Error + 'static>,
}

impl SynchronizationError {
    #[must_use]
    pub fn new(
        action: impl Into<SomeAction>,
        error: impl Into<Box<dyn std::error::Error + 'static>>,
    ) -> Self {
        Self {
            action: action.into(),
            error: error.into(),
        }
    }

    /// Action that failed to execute.
    #[must_use]
    pub fn action(&self) -> &SomeAction {
        &self.action
    }

    /// Underlying error during the operation.
    #[must_use]
    #[allow(clippy::borrowed_box)] // side of inner type is unknown at compile time.
    pub fn error(&self) -> &Box<dyn std::error::Error + 'static> {
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
        Some(&*self.error)
    }
}
