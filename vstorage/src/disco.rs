//! Types related to collection discovery.
//!
//! Discovery is the process of automatically locating collections inside a storage.

use std::collections::HashSet;

use crate::CollectionId;

/// Collection found during discovery.
pub struct DiscoveredCollection {
    // TODO: this does an eager calculation of the CollectionId.
    //       ideally, we'd do this on-demand.
    href: String,
    id: CollectionId,
}

impl DiscoveredCollection {
    /// Create a new instance.
    ///
    /// This method should only be used when implementing discovery for storage implementations.
    #[must_use]
    pub fn new(href: String, id: CollectionId) -> DiscoveredCollection {
        DiscoveredCollection { href, id }
    }

    /// Return the path for this collection.
    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }

    /// Return the collection id for this collection.
    #[must_use]
    pub fn id(&self) -> &CollectionId {
        &self.id
    }
}

/// Result of running discovery on a `Storage`.
///
/// See [`crate::base::Storage::discover_collections`].
pub struct Discovery {
    // INVARIANT: each collection has a unique id.
    collections: Vec<DiscoveredCollection>,
}

impl Discovery {
    /// All discovered collections.
    #[must_use]
    pub fn collections(&self) -> &[DiscoveredCollection] {
        &self.collections
    }

    /// Total amount of discovered collections.
    #[must_use]
    pub fn collection_count(&self) -> usize {
        self.collections.len()
    }

    /// Find a collection with a matching id.
    pub(super) fn find_collection_by_id<'disco>(
        self: &'disco Discovery,
        id: &CollectionId,
    ) -> Option<&'disco DiscoveredCollection> {
        self.collections().iter().find(|c| c.id == *id)
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Multiple collections share the same id.")]
pub struct DuplicateIds;

impl TryFrom<Vec<DiscoveredCollection>> for Discovery {
    type Error = DuplicateIds;

    fn try_from(collections: Vec<DiscoveredCollection>) -> Result<Self, DuplicateIds> {
        let mut seen_ids = HashSet::new();
        for collection in &collections {
            if !seen_ids.insert(&collection.id) {
                return Err(DuplicateIds);
            }
        }
        Ok(Discovery { collections })
    }
}
