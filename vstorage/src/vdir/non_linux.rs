// Copyright 2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use futures_util::future::BoxFuture;
use std::time::Duration;
use tokio::time::{Instant, Interval};

use crate::{
    base::Item,
    watch::{Event, StorageMonitor},
    Result,
};

use super::VdirStorage;

/// Monitor for a [`VdirStorage`] instance.
///
/// # Quirks
///
/// This implementation does not use any in-kernel API, and simply polls every `interval`,
/// returning an [`Event::General`], which indicates that the entire storage must be re-scanned.
///
/// [general]: [crate::watch::Event::General]
pub struct VdirMonitor {
    timer: Interval,
}

impl VdirMonitor {
    /// Create a new monitor for `storage`.
    ///
    /// # Errors
    ///
    /// This portable implementation is infallible.
    pub fn new<I: Item>(_: &VdirStorage<I>, interval: Duration) -> Result<VdirMonitor> {
        let mut timer = tokio::time::interval_at(Instant::now() + interval, interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        Ok(VdirMonitor { timer })
    }
}

impl StorageMonitor for VdirMonitor {
    fn next_event(&mut self) -> BoxFuture<Event> {
        Box::pin(async {
            self.timer.tick().await;
            Event::General
        })
    }
}
