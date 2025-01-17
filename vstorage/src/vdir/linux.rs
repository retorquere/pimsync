// Copyright 2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use futures_util::{
    future::{select, BoxFuture, Either},
    StreamExt as _,
};
use inotify::{EventMask, Inotify, WatchMask};
use log::error;
use std::{pin::pin, time::Duration};
use tokio::time::{Instant, Interval};

use crate::{
    base::Item,
    watch::{Event, EventKind, SpecificEvent, StorageMonitor},
    Error, ErrorKind, Result,
};

use super::VdirStorage;

/// Monitor for a [`VdirStorage`] instance.
///
/// # Quirks
///
/// Due to underlying limitations of inotify(7), it's possible that some events are
/// missed. When this happens, an [`Event::General`][general] is returned.
///
/// [general]: [crate::watch::Event::General]
pub struct VdirMonitor {
    extension: String,
    timer: Interval,
    events: inotify::EventStream<Vec<u8>>,
}

impl VdirMonitor {
    /// Create a new monitor for `storage`.
    ///
    /// # Errors
    ///
    /// If an error occurs setting up the underlying filesystem watcher.
    pub fn new<I: Item>(storage: &VdirStorage<I>, interval: Duration) -> Result<VdirMonitor> {
        // TODO: errors don't help understand the root cause.
        let inotify = Inotify::init().map_err(|err| Error::new(ErrorKind::Io, err))?;
        inotify
            .watches()
            .add(
                &storage.path,
                WatchMask::MODIFY
                    | WatchMask::CREATE
                    | WatchMask::DELETE
                    | WatchMask::CLOSE_WRITE
                    | WatchMask::MOVE,
            )
            .map_err(|err| Error::new(ErrorKind::Io, err))?;
        let buf = Vec::with_capacity(4096); // XXX: Does inotify grow it if necessary?
        let events = inotify
            .into_event_stream(buf)
            .map_err(|err| Error::new(ErrorKind::Io, err))?;

        let mut timer = tokio::time::interval_at(Instant::now() + interval, interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        Ok(VdirMonitor {
            extension: storage.extension.clone(),
            timer,
            events,
        })
    }
}

impl StorageMonitor for VdirMonitor {
    fn next_event(&mut self) -> BoxFuture<Event> {
        Box::pin(async {
            loop {
                let next = pin!(self.events.next());
                // TODO: It should be possible to disable the timer.
                let timeout = pin!(self.timer.tick());
                match select(next, timeout).await {
                    Either::Left((None, _)) => unreachable!("End of stream for inotify events."),
                    Either::Left((Some(Ok(event)), _)) => {
                        if event.mask.contains(EventMask::Q_OVERFLOW) {
                            // TODO: drain the whole queue as well.
                            return Event::General;
                        }
                        let Some(name) = event.name else {
                            return Event::General;
                        };
                        if !name.as_encoded_bytes().ends_with(self.extension.as_bytes()) {
                            continue; // Irrelevant file
                        }
                        let path = match name.into_string() {
                            Ok(s) => s,
                            Err(os) => {
                                error!("Event for non-UTF8 file: {}.", os.to_string_lossy());
                                continue;
                            }
                        };
                        return if event.mask.contains(EventMask::DELETE) {
                            Event::Specific(SpecificEvent {
                                href: path,
                                kind: EventKind::Delete,
                            })
                        } else {
                            Event::Specific(SpecificEvent {
                                href: path,
                                kind: EventKind::Change,
                            })
                        };
                    }
                    Either::Left((Some(Err(err)), _)) => {
                        error!("Error reading from inotify: {err}");
                        // TODO: re-initialise inotify?
                        return Event::General;
                    }
                    Either::Right(..) => return Event::General,
                }
            }
        })
    }
}
