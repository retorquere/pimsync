// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! See [`Plan::execute`](Plan::execute).

use log::{debug, error};

use crate::{
    base::{Item, ItemRef, Storage},
    disco::DiscoveredCollection,
    CollectionId, Href,
};

use super::{
    plan::{CollectionAction, CollectionPlan, ItemAction, Plan},
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
    ) -> Result<(), ExecutionError> {
        {
            match self {
                ItemAction::SaveToStatus { a, b } => {
                    status.insert_item(
                        mapping_uid,
                        &a.uid,
                        &a.hash,
                        &a.to_item_ref(),
                        &b.to_item_ref(),
                    )?;
                }
                ItemAction::ClearStatus { uid } => {
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
    #[error("error querying status database: {0}")]
    StatusDb(#[from] StatusError),
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
    /// # Non-fatal errors
    ///
    /// When a non-fatal error occurs (e.g.: an item being uploaded is rejected), the `on_error`
    /// function will be called with details on the exact error.
    ///
    /// # Errors
    ///
    /// A [`StatusError`] is returned in case writing to the status database fails.
    #[allow(clippy::too_many_lines)]
    pub async fn execute(
        self,
        status: &StatusDatabase,
        on_error: impl Fn(SyncError),
    ) -> Result<(), StatusError> {
        let storage_a = self.storage_a.as_ref();
        let storage_b = self.storage_b.as_ref();

        for plan in self.collection_plans {
            let CollectionPlan {
                alias,
                collection_action,
                item_actions,
                href_a,
                href_b,
                id_a,
                id_b,
            } = plan;

            let (mapping_uid, side_to_delete) = match collection_action
                .clone() // FIXME: cloning is a bit of a hack here
                .execute(status, &href_a, &href_b, id_a, id_b, storage_a, storage_b)
                .await
            {
                Ok((m, s)) => (m, s),
                Err(ExecutionError::StatusDb(err)) => return Err(err),
                Err(err) => {
                    on_error(SyncError::collection(collection_action, alias, err));
                    continue;
                }
            };

            for item_action in item_actions {
                if let Err(err) = item_action
                    .execute(storage_a, storage_b, &href_a, &href_b, status, &mapping_uid)
                    .await
                {
                    on_error(SyncError::item(item_action, err));
                };
            }

            match side_to_delete {
                None => {}
                Some(Side::A) => {
                    if let Err(err) =
                        delete_collection(&href_a, status, storage_a, &mapping_uid).await
                    {
                        let action = CollectionAction::Delete(mapping_uid, Side::A);
                        on_error(SyncError::collection(action, alias, err));
                    };
                }
                Some(Side::B) => {
                    if let Err(err) =
                        delete_collection(&href_b, status, storage_b, &mapping_uid).await
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
    ) -> Result<(MappingUid, Option<Side>), ExecutionError> {
        match self {
            CollectionAction::NoAction(mapping_uid) => Ok((mapping_uid, None)),
            CollectionAction::SaveToStatus => {
                let mapping_uid =
                    status.get_or_add_collection(href_a, href_b, id_a.as_ref(), id_b.as_ref())?;
                Ok((mapping_uid, None))
            }
            CollectionAction::CreateInB => {
                let mapping_uid = create_collection(
                    storage_b,
                    href_b,
                    href_a,
                    status,
                    Side::B,
                    id_b.as_ref(),
                    id_a.as_ref(),
                )
                .await?;
                Ok((mapping_uid, None))
            }
            CollectionAction::CreateInA => {
                let mapping_uid = create_collection(
                    storage_a,
                    href_a,
                    href_b,
                    status,
                    Side::A,
                    id_a.as_ref(),
                    id_b.as_ref(),
                )
                .await?;
                Ok((mapping_uid, None))
            }
            CollectionAction::CreateInBoth => {
                let mapping_uid = create_both_collections(
                    storage_a,
                    storage_b,
                    href_a,
                    href_b,
                    status,
                    id_a.as_ref(),
                    id_b.as_ref(),
                )
                .await?;
                Ok((mapping_uid, None))
            }
            CollectionAction::Delete(mapping, side) => Ok((mapping, Some(side))),
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
    Collection {
        action: CollectionAction,
        alias: String,
    },
}

impl std::fmt::Display for SomeAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SomeAction::Item(action) => {
                write!(f, "item action '{action}'")
            }
            SomeAction::Collection { action, alias } => {
                write!(f, "collection action '{action}' for '{alias}'")
            }
        }
    }
}

/// An error synchronising two items between storages.
#[derive(Debug)]
pub struct SyncError {
    action: SomeAction,
    error: ExecutionError,
}

impl SyncError {
    #[must_use]
    pub fn item(action: ItemAction, error: ExecutionError) -> Self {
        Self {
            action: SomeAction::Item(Box::from(action)),
            error,
        }
    }

    #[must_use]
    pub fn collection(action: CollectionAction, alias: String, error: ExecutionError) -> Self {
        Self {
            action: SomeAction::Collection { action, alias },
            error,
        }
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error executing {}: {}", self.action, self.error)
    }
}

impl std::error::Error for SyncError {
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

#[cfg(test)]
mod test {
    use std::backtrace::Backtrace;

    use crate::sync::{
        execute::{ExecutionError, SomeAction},
        plan::{CollectionAction, ItemAction},
        status::ItemState,
    };

    use super::SyncError;

    #[test]
    fn test_syncerror_item_display() {
        let err = SyncError {
            action: SomeAction::Item(Box::from(ItemAction::CreateInA {
                source: ItemState {
                    href: "/path/to/some/file.vcf".into(),
                    uid: "d99ed506-dceb-49f2-a1c9-efa63c68acd0".into(),
                    etag: "123890".into(),
                    hash: "AAAAAZZZZZ".into(),
                },
            })),
            error: ExecutionError::Storage(crate::Error {
                kind: crate::ErrorKind::AccessDenied,
                source: Some(Box::from(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Not enough mana",
                ))),
                backtrace: Backtrace::capture(),
            }),
        };
        let msg = err.to_string();
        let expected = concat!(
            "Error executing item action 'create in storage a (uid: d99ed506-dceb-49f2-a1c9-efa63c68acd0, from: /path/to/some/file.vcf)': ",
            "storage operation returned error: ",
            "access to the resource was denied: ",
            "Not enough mana"
        );
        assert_eq!(msg, expected);
    }

    #[test]
    fn test_syncerror_collection_display() {
        let err = SyncError {
            action: SomeAction::Collection {
                action: CollectionAction::CreateInB,
                alias: "guests".into(),
            },
            error: ExecutionError::Storage(crate::Error {
                kind: crate::ErrorKind::AccessDenied,
                source: Some(Box::from(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Creating new collections is forbidden",
                ))),
                backtrace: Backtrace::capture(),
            }),
        };
        let msg = err.to_string();
        let expected = concat!(
            "Error executing collection action 'create in storage b' for 'guests': ",
            "storage operation returned error: ",
            "access to the resource was denied: ",
            "Creating new collections is forbidden"
        );
        assert_eq!(msg, expected);
    }
}
