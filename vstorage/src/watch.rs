//! Monitor a storage for changes as they occur.
use futures_util::future::BoxFuture;

use crate::Href;

/// Monitors a storage for live events.
///
/// Implementations shall yield [`Event::General`] if:
///
/// - The underlying storage does not support monitoring for individual changes. In this case, the
///   event should be returned every `interval`.
/// - There is a possibility that some events are lost (e.g.: due to having to reconnect, or a
///   buffer overflow).
///
/// # See also
///
/// - [[`Storage::monitor`][crate::base::Storage::monitor]
pub trait StorageMonitor: Send {
    /// Return the next event on this storage.
    ///
    /// Returns a future which resolves the next change event.
    ///
    /// # Cancel safety
    ///
    /// This method MUST be cancel safe. If the returned future is dropped before completion, the
    /// next call MUST return the following event without dropping any events.
    fn next_event(&mut self) -> BoxFuture<Event>;
}

/// Event yielded when monitoring a storage.
///
/// Returned by [`StorageMonitor::next_event`]
#[derive(Debug)]
pub enum Event {
    /// No details are known; only that something has changed.
    ///
    /// Indicates that a storage provided insufficient granularity and needs to be rescanned.
    /// Example situations are a buffer being full, a stateful connection was dropped or the
    /// storage does not provide gradual events.
    General,
    /// Details of the specific are known.
    Specific(SpecificEvent),
}

/// Specific event on a given resource or collection.
///
/// See also: [`Event`].
#[derive(Debug)]
pub struct SpecificEvent {
    /// The location of the resource.
    pub href: Href,
    /// The type of event that occurred on the resource.
    pub kind: EventKind,
}

/// Kind of event.
///
/// See: [`SpecificEvent`]
#[derive(Debug)]
pub enum EventKind {
    /// A new element was created or an existing one was modified.
    Change,
    /// A previously seen element has been deleted.
    Delete,
    // /// A property has changed.
    // Property { name: String },
    // Unknown,
}
