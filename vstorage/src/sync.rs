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

use crate::Href;

use self::status::{Side, StatusError};

pub mod declare;
pub mod error;
pub mod execute;
pub mod plan;
pub mod status;

#[derive(thiserror::Error, Debug)]
pub enum PlanError {
    #[error("Conflicting mappings on side {0} for href {1}.")]
    ConflictingMappings(Side, Href),

    #[error("Discovery failed for storage A: {0}")]
    DiscoveryFailedA(#[source] crate::Error),

    #[error("Discovery failed for storage B: {0}")]
    DiscoveryFailedB(#[source] crate::Error),

    #[error("Error interacting with underlying storage: {0}")]
    Storage(#[from] crate::Error),

    #[error("Error querying status database: {0}")]
    StatusDb(#[from] StatusError),
}
