// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Types for specifying rules for a synchronisation.
use std::sync::Arc;

use crate::{
    base::{Item, Storage},
    CollectionId, Href,
};

/// A collection as declared by consumer of this library.
///
/// A collection can be declared either via its `href` or `collection_id`.
#[derive(Debug, Clone)]
pub enum CollectionDescription {
    Id { id: CollectionId },
    Href { href: Href },
}

/// A mapping between of a pair of collections across storages.
///
/// This is an unresolved mapping which may be lacking information on one side.
#[derive(Debug, Clone)]
pub enum DeclaredMapping {
    /// Copy between two collections with the same definition on both sides.
    ///
    /// Usage of [`CollectionDescription::Href`] with this variant is highly discouraged.
    Direct { description: CollectionDescription },
    /// Copy a collection from storage `a` to another in `b` with the same collection id.
    FromA { description: CollectionDescription },
    /// Copy a collection from storage `b` to another in `a` with the same collection id.
    FromB { description: CollectionDescription },
    /// Copy between two collections with explicit definitions on both sides.
    Mapped {
        /// This is descriptive and only used for logging / display.
        alias: String,
        a: CollectionDescription,
        b: CollectionDescription,
    },
}

impl DeclaredMapping {
    /// Create a direct mapping.
    ///
    /// This creates the simplest kind of mapping: it maps two collections with the same
    /// `CollectionId`.
    #[must_use]
    pub fn direct(id: CollectionId) -> Self {
        DeclaredMapping::Direct {
            description: CollectionDescription::Id { id },
        }
    }
}

/// A builder for the [`StoragePair`] type.
///
/// Use [`StoragePair::builder`] as a starting point.
pub struct StoragePairBuilder<I: Item> {
    storage_a: Arc<dyn Storage<I>>,
    storage_b: Arc<dyn Storage<I>>,
    mappings: Vec<DeclaredMapping>,
    all_from_a: bool,
    all_from_b: bool,
}

impl<I: Item> StoragePairBuilder<I> {
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

    /// Build the `StoragePair` instance, which can no longer be mutated.
    #[must_use]
    pub fn build(self) -> StoragePair<I> {
        StoragePair {
            storage_a: self.storage_a,
            storage_b: self.storage_b,
            mappings: self.mappings,
            all_from_a: self.all_from_a,
            all_from_b: self.all_from_b,
        }
    }
}

/// A pair of storage that are to be synchronised.
///
/// This type merely wraps around the declaration of what shall be synchronised. It can be
/// constructed offline and is the entry point to create a [`Plan`] and then execute it.
///
/// For details on creating a new instance, see [`StoragePairBuilder`].
///
/// [`Plan`]: crate::sync::plan::Plan
pub struct StoragePair<I: Item> {
    pub(super) storage_a: Arc<dyn Storage<I>>,
    pub(super) storage_b: Arc<dyn Storage<I>>,
    pub(super) mappings: Vec<DeclaredMapping>,
    pub(super) all_from_a: bool,
    pub(super) all_from_b: bool,
}

impl<I: Item> StoragePair<I> {
    /// Build a pair defining how to synchronise two storages.
    pub fn builder(
        storage_a: Arc<dyn Storage<I>>,
        storage_b: Arc<dyn Storage<I>>,
    ) -> StoragePairBuilder<I> {
        StoragePairBuilder {
            storage_a,
            storage_b,
            mappings: Vec::new(),
            all_from_a: false,
            all_from_b: false,
        }
    }

    #[must_use]
    pub fn storage_a(&self) -> Arc<dyn Storage<I>> {
        self.storage_a.clone()
    }

    #[must_use]
    pub fn storage_b(&self) -> Arc<dyn Storage<I>> {
        self.storage_b.clone()
    }
}
