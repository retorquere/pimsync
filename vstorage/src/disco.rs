//! Types related to collection discovery.

use crate::CollectionId;

/// A collection found during discovery.
pub struct DiscoveredCollection {
    // TODO: this does an eager calculation of the CollectionId.
    //       ideally, we'd do this on-demand.
    href: String,
    id: CollectionId,
}

impl DiscoveredCollection {
    #[must_use]
    pub fn new(href: String, id: CollectionId) -> DiscoveredCollection {
        DiscoveredCollection { href, id }
    }

    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }

    #[must_use]
    pub fn id(&self) -> &CollectionId {
        // FIXME: duplicate ids?
        &self.id
    }
}

/// The result of running discovery on a `Storage`.
///
/// See [`crate::base::Storage::discover_collections`].
pub struct Discovery {
    collections: Vec<DiscoveredCollection>,
}

impl Discovery {
    #[must_use]
    pub fn collections(&self) -> &[DiscoveredCollection] {
        &self.collections
    }

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

impl From<Vec<DiscoveredCollection>> for Discovery {
    fn from(collections: Vec<DiscoveredCollection>) -> Discovery {
        Discovery { collections }
    }
}
