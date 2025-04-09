// Copyright 2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use camino::{Utf8Path, Utf8PathBuf};
use futures_util::{
    future::{select, BoxFuture, Either},
    StreamExt as _,
};
use inotify::{EventMask, EventStream, Inotify, WatchMask};
use log::{error, info};
use std::{pin::pin, time::Duration};
use tokio::time::Interval;

use crate::{
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
    events: Option<EventStream<Vec<u8>>>,
    // Required to re-initialise in case of failure.
    path: Utf8PathBuf,
}

impl VdirMonitor {
    /// Create a new monitor for `storage`.
    ///
    /// # Errors
    ///
    /// If an error occurs setting up the underlying filesystem watcher.
    pub fn new(storage: &VdirStorage, interval: Duration) -> Result<VdirMonitor> {
        // TODO: errors don't help understand the root cause.
        let events = init_inotify(&storage.path)?;

        let mut timer = tokio::time::interval(interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        Ok(VdirMonitor {
            extension: storage.extension.clone(),
            timer,
            events: Some(events),
            path: storage.path.clone(),
        })
    }
}

fn init_inotify(path: &Utf8Path) -> Result<EventStream<Vec<u8>>> {
    let inotify = Inotify::init().map_err(|err| Error::new(ErrorKind::Io, err))?;
    inotify
        .watches()
        .add(
            path,
            WatchMask::MODIFY
                | WatchMask::CREATE
                | WatchMask::DELETE
                | WatchMask::CLOSE_WRITE
                | WatchMask::MOVE,
        )
        .map_err(|err| Error::new(ErrorKind::Io, err))?;
    let buf = Vec::with_capacity(4096); // XXX: Review the size of this buffer.
    let events = inotify
        .into_event_stream(buf)
        .map_err(|err| Error::new(ErrorKind::Io, err))?;
    Ok(events)
}

impl StorageMonitor for VdirMonitor {
    fn next_event(&mut self) -> BoxFuture<Event> {
        Box::pin(async {
            loop {
                // TODO: It should be possible to disable the timer.
                let next = if let Some(ref mut n) = self.events {
                    pin!(n.next())
                } else {
                    // No inotify listener exists.
                    self.timer.tick().await;
                    match init_inotify(&self.path) {
                        Ok(events) => {
                            info!("Re-initialised inotify listener");
                            self.events = Some(events);
                        }
                        Err(err) => {
                            // Should not happen.
                            error!("Could not re-initialise inotify: {err}");
                            self.events = None;
                        }
                    }
                    return Event::General;
                };

                let timeout = pin!(self.timer.tick());
                match select(next, timeout).await {
                    Either::Left((None, _)) => unreachable!("End of stream for inotify events."),
                    Either::Left((Some(Ok(event)), _)) => {
                        // Directory deletions trigger an Event::General due to inotify requiring
                        // re-initialisation, so we don't consider directories in this branch.

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
                        // This branch reached when a collection is deleted locally.
                        error!("Error reading from inotify: {err}");
                        match init_inotify(&self.path) {
                            Ok(events) => {
                                info!("Re-initialised inotify listener");
                                self.events = Some(events);
                            }
                            Err(err) => {
                                // Should not happen.
                                error!("Could not re-initialise inotify: {err}");
                                self.events = None;
                            }
                        }
                        return Event::General;
                    }
                    Either::Right(..) => return Event::General,
                }
            }
        })
    }
}

#[cfg(test)]
mod test {
    use std::{env::temp_dir, sync::Arc, time::Duration};

    use camino::Utf8PathBuf;
    use rand::{distributions::Alphanumeric, thread_rng, Rng as _};

    use crate::{
        base::Storage,
        vdir::{ItemKind, VdirStorage},
        watch::Event,
    };

    fn temp_path() -> Utf8PathBuf {
        let name = thread_rng()
            .sample_iter(Alphanumeric)
            .take(12)
            .map(char::from)
            .collect::<String>();
        temp_dir().join(name).try_into().unwrap()
    }

    #[tokio::test]
    async fn inotify_after_deletion() {
        let path_a = temp_path();
        // let path_b = temp_path();

        std::fs::create_dir(&path_a).unwrap();
        std::fs::create_dir(path_a.join("one")).unwrap();
        let storage_a = VdirStorage::new(path_a.clone(), "ics".into(), ItemKind::Calendar);
        let storage_a: Arc<dyn Storage> = Arc::new(storage_a);

        // Interval is high enough that we shouldn't reach it
        let mut monitor = storage_a.monitor(Duration::from_secs(5)).await.unwrap();

        // Initial general event.
        assert_eq!(Event::General, monitor.next_event().await);

        std::fs::remove_dir(path_a.join("one")).unwrap();
        // Event for the directory deletion.
        let _ = tokio::time::timeout(Duration::from_secs(1), monitor.next_event()).await;

        // No more events should arrive after the first one.
        let next = tokio::time::timeout(Duration::from_secs(1), monitor.next_event()).await;
        assert!(next.is_err(), "Expected timeout, got event");
    }
}
