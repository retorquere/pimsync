//! Types and functions specific to calendars and events.
use libdav::{names, PropertyName};

use crate::base::Property;

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
    /// Returns the name of the corresponding CalDAV property.
    #[must_use]
    pub fn dav_propname(&self) -> &PropertyName<'_, '_> {
        match self {
            CalendarProperty::Colour => &names::CALENDAR_COLOUR,
            CalendarProperty::DisplayName => &names::DISPLAY_NAME,
            CalendarProperty::Description => &names::CALENDAR_DESCRIPTION,
            CalendarProperty::Order => &names::CALENDAR_ORDER,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            CalendarProperty::DisplayName => "displayname",
            CalendarProperty::Colour => "color",
            CalendarProperty::Description => "description",
            CalendarProperty::Order => "order",
        }
    }

    #[must_use]
    pub fn known_properties() -> &'static [Property] {
        &[
            Property::Calendar(CalendarProperty::DisplayName),
            Property::Calendar(CalendarProperty::Colour),
            Property::Calendar(CalendarProperty::Description),
            Property::Calendar(CalendarProperty::Order),
        ]
    }

    #[must_use]
    pub fn filename(&self) -> &'static str {
        match self {
            CalendarProperty::DisplayName => "displayname",
            CalendarProperty::Colour => "color",
            CalendarProperty::Description => "description",
            CalendarProperty::Order => "order",
        }
    }
}

impl From<CalendarProperty> for Property {
    fn from(value: CalendarProperty) -> Self {
        Property::Calendar(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::base::Storage;

    #[test]
    fn test_storage_is_object_safe() {
        #[allow(dead_code)]
        fn dummy(_: Box<dyn Storage>) {}
    }
}
