use crate::Href;

/// Event yielded when monitoring a storage.
pub enum Event {
    /// Details of the specific are known.
    Specific(SpecificEvent),
    /// No details are known; only that something has changed.
    General,
}

pub struct SpecificEvent {
    pub href: Href,
    pub kind: EventKind,
}

pub enum EventKind {
    Create,
    Update,
    Delete,
    Property { name: String },
    Unknown,
}
