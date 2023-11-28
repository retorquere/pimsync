// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Plan::execute`](Plan::execute).

use std::sync::Arc;

use log::trace;

use crate::{
    base::{Item, Storage},
    sync::{plan::Action, state::ItemState},
};

use super::{
    plan::{ItemAction, Plan, ResolvedCollection, ResolvedMapping},
    state::{CollectionState, StorageState},
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
                Action::CopyToB => {
                    copy_item(
                        state_a.ok_or("collection missing from state a")?,
                        state_b.ok_or("collection missing from state b")?,
                        storage_a,
                        storage_b,
                        self.uid(),
                    )
                    .await?;
                }
                Action::CopyToA => {
                    copy_item(
                        state_b.ok_or("collection missing from state b")?,
                        state_a.ok_or("collection missing from state a")?,
                        storage_b,
                        storage_a,
                        self.uid(),
                    )
                    .await?;
                }
                Action::DeleteInA => {
                    delete_item(
                        state_a.ok_or("collection is missing from state a")?,
                        storage_a,
                        self.uid(),
                    )
                    .await?;
                }
                Action::DeleteInB => {
                    delete_item(
                        state_b.ok_or("collection is missing from state b")?,
                        storage_b,
                        self.uid(),
                    )
                    .await?;
                }
                Action::Conflict => todo!("conflict resolution"),
            };
            Ok(())
        }
    }
}

async fn copy_item<I: Item>(
    src_state: &CollectionState,
    dst_state: &mut CollectionState,
    src_storage: &Arc<dyn Storage<I>>,
    dst_storage: &Arc<dyn Storage<I>>,
    uid: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let item_state = src_state.get_item_by_uid(uid).ok_or("item is missing")?;
    let (item, _) = src_storage.get_item(&item_state.href).await?;

    let col = dst_storage.open_collection(&dst_state.href)?;

    if let Some(dst_item_state) = dst_state.get_item_by_uid_mut(uid) {
        trace!("Updating {uid}");
        let new_etag = dst_storage
            .update_item(&dst_item_state.href, &dst_item_state.etag, &item)
            .await?;
        dst_item_state.etag = new_etag;
        dst_item_state.hash = item.hash();
    } else {
        trace!("Creating {uid}");
        let new_ref = dst_storage.add_item(&col, &item).await?;
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

    storage
        .delete_item(&item_state.href, &item_state.etag)
        .await?;

    state.items.swap_remove(pos);

    Ok(())
}

impl<'pair, I: Item> Plan<'pair, I> {
    /// Executes a synchronization plan.
    ///
    /// Always returns a final state, regardless of what changes were applied. The returned value
    /// will include any errors that occurred during synchronisation. If any errors exist, then
    /// both storage may still be  out of sync.
    pub async fn execute(self) -> FinalState {
        let mut final_state = FinalState {
            state_a: self.current_state_a().clone(),
            state_b: self.current_state_b().clone(),
            errors: Vec::new(),
        };
        let storage_a = &self.pair.storage_a;
        let storage_b = &self.pair.storage_b;

        for cp in self.collection_plans {
            let mut delete_collection_in_a = false;
            let mut delete_collection_in_b = false;
            match cp.collection_action() {
                None => {}
                Some(Action::CopyToB) => {
                    create_collection(
                        storage_b,
                        cp.mapping().collection_b(),
                        &mut final_state.state_b,
                        &mut final_state.errors,
                        &Action::CopyToB,
                        cp.mapping(),
                    )
                    .await;
                }
                Some(Action::CopyToA) => {
                    create_collection(
                        storage_a,
                        cp.mapping().collection_a(),
                        &mut final_state.state_a,
                        &mut final_state.errors,
                        &Action::CopyToA,
                        cp.mapping(),
                    )
                    .await;
                }
                Some(Action::Conflict) => {
                    final_state.errors.push(SynchronizationError {
                        action: Action::Conflict,
                        resource: FailedResource::Collection {
                            collection: cp.mapping().clone(),
                        },
                        /// FIXME: actually, meta can conflict.
                        error: "Invalid input: conflict between storages is senseless".into(),
                    });
                }
                Some(Action::DeleteInA) => {
                    delete_collection_in_a = true;
                }
                Some(Action::DeleteInB) => {
                    delete_collection_in_b = true;
                }
            }

            for item_action in cp.items() {
                // FIXME: I need to somehow move these two calls outside of the "for" loop.
                let state_a = final_state
                    .state_a
                    .find_collection_state_mut(&cp.mapping().a);
                let state_b = final_state
                    .state_b
                    .find_collection_state_mut(&cp.mapping().b);

                if let Err(err) = item_action
                    .execute(storage_a, storage_b, state_a, state_b)
                    .await
                {
                    final_state.errors.push(SynchronizationError {
                        action: item_action.action(),
                        resource: FailedResource::Item {
                            uid: item_action.uid().to_string(),
                        },
                        error: err,
                    });
                };
            }
            if delete_collection_in_a {
                delete_collection(
                    storage_a,
                    cp.mapping().collection_a(),
                    &mut final_state.state_a,
                    &mut final_state.errors,
                    &Action::DeleteInA,
                    cp.mapping(),
                )
                .await;
            }
            if delete_collection_in_b {
                delete_collection(
                    storage_b,
                    cp.mapping().collection_b(),
                    &mut final_state.state_b,
                    &mut final_state.errors,
                    &Action::DeleteInA,
                    cp.mapping(),
                )
                .await;
            }
        }

        final_state
    }
}

/// The state of a storage pair after synchronisation.
///
/// Storages may have been mutated before an error occurred, so the final state for both is always
/// returned, even in case of an error.
#[must_use]
pub struct FinalState {
    /// The state of `storage_a` after executing a plan.
    pub(super) state_a: StorageState,
    /// The state of `storage_b` after executing a plan.
    pub(super) state_b: StorageState,
    /// Any errors that may have occurred during synchronisation.
    pub(super) errors: Vec<SynchronizationError>,
}

impl FinalState {
    /// Returns `true` if both storages are in sync.
    #[must_use]
    pub fn synchronised_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// The state of `storage_a` after synchronisation.
    ///
    /// This value should be persisted and supplied as a `previous_state` the next time this storage
    /// is synchronised.
    #[must_use]
    pub fn final_state_a(&self) -> &StorageState {
        &self.state_a
    }

    /// The state of `storage_b` after synchronisation.
    ///
    /// This value should be persisted and supplied as a `previous_state` the next time this storage
    /// is synchronised.
    #[must_use]
    pub fn final_state_b(&self) -> &StorageState {
        &self.state_b
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
    errors: &mut Vec<SynchronizationError>,
    action: &Action,
    mapping: &ResolvedMapping,
) {
    let creation_result = match collection {
        ResolvedCollection::Id { id } => storage.create_collection_with_id(id).await,
        ResolvedCollection::Href { href } => storage.create_collection(href).await,
    };

    match creation_result {
        Ok(col) => {
            // FIXME: panics
            // To be honest, it doesn't make sense that this error would ever happen.
            // It implies that we managed to create a collection, but the `href` is not valid and
            // we can't get it's collection_id.
            let id = storage.collection_id(&col).unwrap();
            state.add_collection(id, col.href().to_string());
        }
        Err(e) => {
            errors.push(SynchronizationError {
                action: action.clone(),
                resource: FailedResource::Collection {
                    collection: mapping.clone(),
                },
                error: Box::new(e),
            });
        }
    };
}

async fn delete_collection<I: Item>(
    storage: &Arc<dyn Storage<I>>,
    collection: &ResolvedCollection,
    state: &mut StorageState,
    errors: &mut Vec<SynchronizationError>,
    action: &Action,
    mapping: &ResolvedMapping,
) {
    let href = match collection.href() {
        Some(h) => h,
        None => {
            // TODO: this should not be possible to model.
            //       deleting a collection requires specifying it by href. If no href is available,
            //       then the planning stage should no-op, because it doesn't exist.
            todo!("deleting collections without an href is not yet implemented")
        }
    };

    match storage.destroy_collection(href).await {
        Ok(()) => {
            state.remove_collection(href);
        }
        Err(e) => {
            errors.push(SynchronizationError {
                action: action.clone(),
                resource: FailedResource::Collection {
                    collection: mapping.clone(),
                },
                error: Box::new(e),
            });
        }
    };
}

/// Inner type for [`SynchronizationError`].
#[derive(Debug)]
pub enum FailedResource {
    Item { uid: String },
    Collection { collection: ResolvedMapping },
}

/// An error synchronising two items between storages.
#[derive(Debug)]
pub struct SynchronizationError {
    action: Action,
    resource: FailedResource,
    error: Box<dyn std::error::Error + 'static>,
}

impl SynchronizationError {
    /// The action that failed to execute.
    #[must_use]
    pub fn action(&self) -> &Action {
        &self.action
    }

    /// The resource that failed to execute.
    #[must_use]
    pub fn resource(&self) -> &FailedResource {
        &self.resource
    }
}

impl std::fmt::Display for SynchronizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Error performing {:?} on {:?}: {}",
            self.action, self.resource, self.error
        )
    }
}

impl std::error::Error for SynchronizationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.error)
    }
}
