// Copyright 2023-2024 Hugo Osvaldo Barrera
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
//! - The plan may have conflicting items (represented as [`ItemAction::Conflict`]). These MAY be
//!   replaced with different actions (e.g.: [`ItemAction::Update`] to overwrite data in one
//!   storage with data from the other).
//!   - If resolving conflicts requires writing a merged file into both storages, this needs to be
//!     done using the `Storage` APIs directly. The next synchronisation will detect the resolved
//!     conflict automatically and update the status database.
//! - Use the [`execute::Executor`] type to execute the plan itself. It updates the status database
//!   to reflect which items exist on both side and their metadata. This will be used on the next
//!   cycle to understand which items have changed on which sides.
//!
//! The synchronization algorithm is based on [the algorithm from the original vdirsyncer][orig].
//!
//! [orig]: https://unterwaditzer.net/2016/sync-algorithm.html
//! [`ItemAction::Conflict`]: crate::sync::plan::ItemAction::Conflict
//! [`ItemAction::Update`]: crate::sync::plan::ItemAction::Update

pub mod declare;
mod error;
pub mod execute;
pub mod plan;
pub mod status;

pub use error::SomeAction;
pub use error::SyncError;
pub use execute::ExecutionError;
