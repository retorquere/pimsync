// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use crate::{
    base::{Collection, Item, Storage},
    CollectionId, Result,
};

/// Find a collection with a matching id.
///
/// - Returns `Ok(Some(_))` if a matching collection was found.
/// - Returns `Ok(None)` if no collection has the specified id.
/// - Returns `Err(_)` if resolving the id of a collection failed.
pub(super) fn find_collection_by_id<'c, I: Item>(
    collections: &'c [Collection],
    storage: &dyn Storage<I>,
    id: &CollectionId,
) -> Result<Option<&'c Collection>> {
    collections
        .iter()
        .find_map(|c| match storage.collection_id(c) {
            Ok(c_id) => {
                if c_id == *id {
                    Some(Ok(c))
                } else {
                    None
                }
            }
            Err(err) => Some(Err(err)),
        })
        .transpose()
}
