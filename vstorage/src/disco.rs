//! Types related to collection discovery.

use std::sync::Arc;

use crate::{
    base::{Collection, Item, Storage},
    CollectionId, Result,
};

/// The result of running discovery on a `Storage`.
///
/// See `[crate::Storage::discover_collections`].
pub struct Discovery {
    collections: Vec<Collection>,
}

impl Discovery {
    #[must_use]
    pub fn collections(&self) -> &[Collection] {
        &self.collections
    }

    #[must_use]
    pub fn collection_count(&self) -> usize {
        self.collections.len()
    }

    /// Find a collection with a matching id.
    ///
    /// - Returns `Ok(Some(_))` if a matching collection was found.
    /// - Returns `Ok(None)` if no collection has the specified id.
    /// - Returns `Err(_)` if resolving the id of a collection failed.
    // TODO: Instances of this type could be associated to the Storage type that returned it.
    pub(super) fn find_collection_by_id<'disco, I: Item>(
        self: &'disco Discovery,
        storage: &Arc<dyn Storage<I>>,
        id: &CollectionId,
    ) -> Result<Option<&'disco Collection>> {
        self.collections()
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
}

impl From<Vec<Collection>> for Discovery {
    fn from(collections: Vec<Collection>) -> Discovery {
        Discovery { collections }
    }
}
