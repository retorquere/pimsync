// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Types for specifying rules for a synchronisation.
use std::sync::Arc;

use crate::{
    base::{Item, Storage},
    CollectionId, Href,
};

/// Collection declared either via its `href` or `collection_id`.
///
/// This type represents a user-configured collection, which may or may not exist.
#[derive(Debug, Clone)]
pub enum CollectionDescription {
    /// Refers to a collection with a matching collection id.
    ///
    /// If it does not exist, a collection will be created with the expectation that discovery
    /// would find it with this collection id.
    Id {
        /// The id for the declared collection.
        id: CollectionId,
    },
    /// Refers to a collection with a matching `href`.
    ///
    /// If it does not exist, a collection with this exact `href` shall be created.
    Href {
        /// The href for the declared collection.
        href: Href,
    },
}

impl CollectionDescription {
    pub(crate) fn alias(&self) -> String {
        match self {
            CollectionDescription::Id { id } => id.to_string(),
            CollectionDescription::Href { href } => format!("href:{href}"),
        }
    }
}

/// A mapping between of a pair of collections across storages.
///
/// An unresolved mapping, as declared by a user, which may be lacking information on one side.
#[derive(Debug, Clone)]
pub enum DeclaredMapping {
    /// Copy between two collections with the same definition on both sides.
    ///
    /// Usage of [`CollectionDescription::Href`] between different storage implementations is
    /// discouraged.
    Direct {
        /// Description which applies to the collection on both sides.
        description: CollectionDescription,
    },
    /// Copy between two collections with explicit definitions on both sides.
    Mapped {
        /// A descriptive name used for logging and display.
        alias: String,
        /// The description for the collection on side `a`.
        a: CollectionDescription,
        /// The description for the collection on side `b`.
        b: CollectionDescription,
    },
}

impl DeclaredMapping {
    /// Create a direct mapping.
    ///
    /// This creates the simplest kind of mapping: it maps two collections with the same
    /// [`CollectionId`].
    #[must_use]
    pub fn direct(id: CollectionId) -> Self {
        DeclaredMapping::Direct {
            description: CollectionDescription::Id { id },
        }
    }
}

/// A pair of storage that are to be synchronised.
///
/// This type merely wraps around the declaration of what shall be synchronised. It can be
/// constructed offline and is the entry point to create a [`Plan`] and then execute it.
///
/// New pairs can be created via [`StoragePair::new`].
///
/// [`Plan`]: crate::sync::plan::Plan
pub struct StoragePair<I: Item> {
    pub(super) storage_a: Arc<dyn Storage<I>>,
    pub(super) storage_b: Arc<dyn Storage<I>>,
    pub(super) mappings: Vec<DeclaredMapping>,
    pub(super) all_from_a: bool,
    pub(super) all_from_b: bool,
    pub(super) on_empty: OnEmpty,
    pub(super) on_delete: OnDelete,
}

impl<I: Item> StoragePair<I> {
    /// Create a new instance.
    ///
    /// By default, no collections are to be synchronised. See other associated functions for
    /// details con configuring additional collections.
    #[must_use]
    pub fn new(storage_a: Arc<dyn Storage<I>>, storage_b: Arc<dyn Storage<I>>) -> StoragePair<I> {
        StoragePair {
            storage_a,
            storage_b,
            mappings: Vec::new(),
            all_from_a: false,
            all_from_b: false,
            on_empty: OnEmpty::Skip,
            on_delete: OnDelete::Sync,
        }
    }

    /// Include the specified mapping when synchronising.
    #[must_use]
    pub fn with_mapping(mut self, mapping: DeclaredMapping) -> Self {
        self.mappings.push(mapping);
        self
    }

    /// Include all collections from storage A when synchronising.
    ///
    /// By default, only explicitly included collections are synchronised.
    #[must_use]
    pub fn with_all_from_a(mut self) -> Self {
        self.all_from_a = true;
        self
    }

    /// Include all collections from storage B when synchronising.
    ///
    /// By default, only explicitly included collections are synchronised.
    #[must_use]
    pub fn with_all_from_b(mut self) -> Self {
        self.all_from_b = true;
        self
    }

    /// Returns a reference to storage a.
    #[must_use]
    pub fn storage_a(&self) -> &dyn Storage<I> {
        self.storage_a.as_ref()
    }

    /// Returns a reference to storage a.
    #[must_use]
    pub fn storage_b(&self) -> &dyn Storage<I> {
        self.storage_b.as_ref()
    }

    /// Action to take when a collection is completely emptied.
    #[must_use]
    pub fn on_empty(mut self, action: OnEmpty) -> Self {
        self.on_empty = action;
        self
    }

    /// Action to take when a collection is deleted.
    #[must_use]
    pub fn on_delete(mut self, action: OnDelete) -> Self {
        self.on_delete = action;
        self
    }
}

/// Action to take when a collection has been emptied on one side.
#[derive(Debug, PartialEq, Default)]
pub enum OnEmpty {
    /// Skip synchronising this pair of collecitons.
    #[default]
    Skip,
    /// Synchronise changes (e.g.: emptying the other side too).
    Sync,
}

/// Action to take when a collection has been deleted on one side.
#[derive(Debug, PartialEq, Default)]
pub enum OnDelete {
    /// Skip synchronising this collection.
    Skip,
    /// Synchronising the deletion.
    ///
    /// Note that only empty collections are deleted, so the collection on the opposite side will
    /// not be deleted if it is non-empty.
    #[default]
    Sync,
}
