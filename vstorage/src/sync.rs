// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Components used for synchronising storages.
//!
//! The general gist behind synchronising is:
//!
//! - Create a new [`declare::StoragePair`]. This type specifies which storages and which
//!   collections are to be synchronised. A [`status::StatusDatabase`] instance with details of the
//!   previous synchronisation should be provided, if it exists.
//! - [`plan::Plan::new`] is used to create a [`Plan`](plan::Plan). This contains a list of actions
//!   to be executed to synchronise both storages. This instance can also be inspected before
//!   executing any actions (e.g.: as a from of dry-run).
//! - [`plan::Plan::execute`] executes the plan itself. It updates the status to
//!   reflect which items exist on both side and their metadata. This will be used on the next
//!   cycle to understand which items have changed on which sides.
//!
//! The synchronization algorithm is based on [the algorithm from the original vdirsyncer][orig].
//!
//! [orig]: https://unterwaditzer.net/2016/sync-algorithm.html

pub mod declare;
mod error;
mod execute;
pub mod plan;
pub mod status;

pub use error::SomeAction;
pub use error::SyncError;
pub use execute::ExecutionError;
