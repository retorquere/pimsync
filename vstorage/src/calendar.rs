//! Types and functions specific to calendars and events.
use libdav::{names, PropertyName};

use crate::base::{ItemKind, Property};

/// Immutable wrapper around a `VCALENDAR` or `VCARD`.
///
/// Note that this is not a proper validating parser for icalendar or vcard; it's a very simple one
/// with the sole purpose of extracting a UID. Proper parsing of components is out of scope, since
/// supporting potentially invalid items is required.
#[derive(Debug, Clone)]
pub struct IcsItem;

impl ItemKind for IcsItem {
    /// Calendar properties defined by `CalDav`.
    type Property = CalendarProperty;
}

/// Properties supported for calendars.
///
/// This is strongly based on the properties supported by `CalDav`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum CalendarProperty {
    /// A colour to be used when displaying this collection.
    ///
    /// Graphical interfaces may use this for the collection itself or its items.
    Colour,
    /// A user-friendly name for a collection.
    ///
    /// It is recommended to show this name in user interfaces.
    DisplayName,
    /// Human readable description of the collection.
    Description,
    /// Sorting order for this collection.
    Order,
}

impl CalendarProperty {
    /// Returns the name of the corresponding DAV property.
    #[must_use]
    pub fn dav_propname(&self) -> &PropertyName<'_, '_> {
        match self {
            CalendarProperty::Colour => &names::CALENDAR_COLOUR,
            CalendarProperty::DisplayName => &names::DISPLAY_NAME,
            CalendarProperty::Description => &names::CALENDAR_DESCRIPTION,
            CalendarProperty::Order => &names::CALENDAR_ORDER,
        }
    }
}

impl Property for CalendarProperty {
    fn name(&self) -> &str {
        match self {
            CalendarProperty::DisplayName => "displayname",
            CalendarProperty::Colour => "color",
            CalendarProperty::Description => "description",
            CalendarProperty::Order => "order",
        }
    }

    fn known_properties() -> &'static [Self] {
        &[
            CalendarProperty::DisplayName,
            CalendarProperty::Colour,
            CalendarProperty::Description,
            CalendarProperty::Order,
        ]
    }

    fn filename(&self) -> &'static str {
        match self {
            CalendarProperty::DisplayName => "displayname",
            CalendarProperty::Colour => "color",
            CalendarProperty::Description => "description",
            CalendarProperty::Order => "order",
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{base::Storage, calendar::IcsItem};

    #[test]
    fn test_storage_is_object_safe() {
        #[allow(dead_code)]
        fn dummy(_: Box<dyn Storage<IcsItem>>) {}
    }
}
