// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Types for specifying rules for a synchronisation.
use crate::{
    base::{Item, Storage},
    sync::state::StorageState,
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

pub(super) struct PairInfo<'a, I: Item> {
    pub(super) storage_a: &'a mut dyn Storage<I>,
    pub(super) storage_b: &'a mut dyn Storage<I>,
    pub(super) previous_state_a: Option<&'a StorageState>,
    pub(super) previous_state_b: Option<&'a StorageState>,
    pub(super) mappings: Vec<DeclaredMapping>,
    pub(super) all_from_a: bool,
    pub(super) all_from_b: bool,
}

pub struct StoragePairBuilder<'a, I: Item> {
    info: PairInfo<'a, I>,
}

impl<'a, I: Item> StoragePairBuilder<'a, I> {
    /// Include the specified mapping when synchronising.
    #[must_use]
    pub fn with_mapping(mut self, mapping: DeclaredMapping) -> Self {
        self.info.mappings.push(mapping);
        self
    }

    /// Include all collections from storage A when synchronising.
    #[must_use]
    pub fn with_all_from_a(mut self) -> Self {
        self.info.all_from_a = true;
        self
    }

    /// Include all collections from storage B when synchronising.
    #[must_use]
    pub fn with_all_from_b(mut self) -> Self {
        self.info.all_from_b = true;
        self
    }

    /// Provide a previous state for storage A.
    #[must_use]
    pub fn with_previous_state_for_a(mut self, state: &'a StorageState) -> Self {
        self.info.previous_state_a = Some(state);
        self
    }

    /// Provide a previous state for storage B.
    #[must_use]
    pub fn with_previous_state_for_b(mut self, state: &'a StorageState) -> Self {
        self.info.previous_state_b = Some(state);
        self
    }

    /// Build the `StoragePair` instance, which can no longer be mutated.
    #[must_use]
    pub fn build(self) -> StoragePair<'a, I> {
        StoragePair { info: self.info }
    }
}

/// A pair of storage that are to be synchronised.
///
/// This type merely wraps around the declaration of what shall be synchronised. It can be
/// constructed offline and is the entry point to create a [`Plan`] and then execute it.
///
/// [`Plan`]: crate::sync::plan::Plan
pub struct StoragePair<'a, I: Item> {
    pub(super) info: PairInfo<'a, I>,
}

impl<I: Item> StoragePair<'_, I> {
    /// Build a pair defining how to synchronise two storages.
    pub fn builder<'a>(
        storage_a: &'a mut dyn Storage<I>,
        storage_b: &'a mut dyn Storage<I>,
    ) -> StoragePairBuilder<'a, I> {
        StoragePairBuilder {
            info: PairInfo {
                storage_a,
                storage_b,
                previous_state_a: None,
                previous_state_b: None,
                mappings: Vec::new(),
                all_from_a: false,
                all_from_b: false,
            },
        }
    }
}
