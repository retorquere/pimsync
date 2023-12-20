// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Components used for synchronising storages.
//!
//! The general gist behind synchronising is:
//!
//! - A [`StoragePair`](declare::StoragePair) specified the details of storages and which
//!   collections are to be synchronised. This type, optionally, takes the state of the last
//!   synchronisation.
//! - A [`Plan`](plan::Plan) contains a list of actions to be executed to synchronise both
//!   storages. This instance can also be inspected before executing any actions (e.g.: as a from
//!   of dry-run).
//! - [`Plan::execute`](plan::Plan::execute) executes the plan itself and returns two opaque states
//!   that should be serialised and used as input for the next synchronisation. This data is used
//!   to understand which of both sides has changed when items diverge.
//!
//! The synchronization algorithm is based on [the algorithm from the original vdirsyncer][orig].
//!
//! [orig]: https://unterwaditzer.net/2016/sync-algorithm.html

use self::plan::ResolvedCollection;

pub mod declare;
pub mod execute;
pub mod plan;
pub mod state;

#[derive(thiserror::Error, Debug)]
pub enum PlanError {
    #[error("Duplicate collection defined for storage A: {0}")]
    DuplicateCollectionInA(ResolvedCollection),

    #[error("Duplicate collection defined for storage B: {0}")]
    DuplicateCollectionInB(ResolvedCollection),

    #[error("Discovery failed for storage A")]
    DiscoveryFailedA(#[source] crate::Error),

    #[error("Discovery failed for storage B")]
    DiscoveryFailedB(#[source] crate::Error),

    #[error("Invalid collection mappings provided")]
    BadCollectionMappings(#[source] crate::Error),

    #[error("Error determining current state for storage a")]
    StateA(#[source] crate::Error),

    #[error("Error determining current state for storage a")]
    StateB(#[source] crate::Error),
}
