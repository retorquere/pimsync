// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Plan::execute`](Plan::execute).

use log::{debug, error};

use crate::{
    base::{Item, ItemRef, Storage},
    sync::{plan::Action, state::ItemState},
    Etag, Href,
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
        storage_a: &dyn Storage<I>,
        storage_b: &dyn Storage<I>,
        state_a: Option<&mut CollectionState>,
        state_b: Option<&mut CollectionState>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        {
            match self.action() {
                Action::CreateInB { source } => {
                    create_item(
                        source,
                        state_b.ok_or("target collection missing when creating")?,
                        storage_a,
                        storage_b,
                    )
                    .await?;
                }
                Action::UpdateInB { source, target } => {
                    update_item(
                        source,
                        target,
                        state_b.ok_or("target collection missing when updating")?,
                        storage_a,
                        storage_b,
                    )
                    .await?;
                }
                Action::CreateInA { source } => {
                    create_item(
                        source,
                        state_a.ok_or("target collection missing when creating")?,
                        storage_b,
                        storage_a,
                    )
                    .await?;
                }
                Action::UpdateInA { source, target } => {
                    update_item(
                        source,
                        target,
                        state_a.ok_or("target collection missing when updating")?,
                        storage_b,
                        storage_a,
                    )
                    .await?;
                }
                Action::DeleteInA { href, etag } => {
                    delete_item(
                        href,
                        etag,
                        state_a.ok_or("target collection missing when deleting")?,
                        storage_a,
                    )
                    .await?;
                }
                Action::DeleteInB { href, etag } => {
                    delete_item(
                        href,
                        etag,
                        state_b.ok_or("target collection missing when deleting")?,
                        storage_b,
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

async fn create_item<I: Item>(
    src_href: &Href,
    dst_state: &mut CollectionState,
    src_storage: &dyn Storage<I>,
    dst_storage: &dyn Storage<I>,
) -> crate::Result<()> {
    debug!("Creating item from {src_href}");

    let (item_data, _) = src_storage.get_item(src_href).await?;
    let uid = item_data.ident();
    let new_item = dst_storage.add_item(&dst_state.href, &item_data).await?;

    dst_state.items.push(ItemState {
        href: new_item.href,
        uid,
        etag: new_item.etag,
        hash: item_data.hash(),
    });

    Ok(())
}

async fn update_item<I: Item>(
    src_href: &Href,
    target: &ItemRef,
    dst_state: &mut CollectionState,
    src_storage: &dyn Storage<I>,
    dst_storage: &dyn Storage<I>,
) -> Result<(), Box<dyn std::error::Error>> {
    debug!("Updating {}", target.href);

    let (item, _) = src_storage.get_item(src_href).await?;

    let new_etag = dst_storage
        .update_item(&target.href, &target.etag, &item)
        .await?;
    let dst_item_state = dst_state
        .get_item_by_href_mut(&target.href)
        .ok_or("item being updated must exist in state")?;
    dst_item_state.etag = new_etag;
    dst_item_state.hash = item.hash();

    Ok(())
}

async fn delete_item<I: Item>(
    href: &Href,
    etag: &Etag,
    state: &mut CollectionState,
    storage: &dyn Storage<I>,
) -> Result<(), Box<dyn std::error::Error>> {
    let pos = state
        .items
        .iter()
        .position(|i| i.href == *href)
        .ok_or("item pending deletion is missing from state")?;

    storage.delete_item(href, etag).await?;

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
        let storage_a = self.pair.storage_a.as_ref();
        let storage_b = self.pair.storage_b.as_ref();

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
                    errors.push(SynchronizationError::new(item_action, err));
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
    storage: &dyn Storage<I>,
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
