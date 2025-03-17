//! Types and functions specific to calendars and events.
use libdav::{names, PropertyName};

use crate::{
    base::{uid, Item, Property},
    util::{replace_uid, ItemHash},
};

/// Immutable wrapper around a `VCALENDAR` or `VCARD`.
///
/// Note that this is not a proper validating parser for icalendar or vcard; it's a very simple one
/// with the sole purpose of extracting a UID. Proper parsing of components is out of scope, since
/// supporting potentially invalid items is required.
#[derive(Debug, Clone)]
pub struct IcsItem {
    raw: String,
}

impl Item for IcsItem {
    /// Calendar properties defined by `CalDav`.
    type Property = CalendarProperty;

    /// Returns the contents of the `UID` property, if defined.
    #[must_use]
    fn uid(&self) -> Option<String> {
        uid(&self.raw)
    }

    /// Returns the hash of the normalised content.
    #[must_use]
    fn hash(&self) -> ItemHash {
        crate::util::hash(&self.raw)
    }

    /// Returns a new copy of this Item with the supplied UID.
    #[must_use]
    fn with_uid(&self, new_uid: &str) -> Self {
        IcsItem::from(replace_uid(&self.raw, new_uid))
    }

    #[inline]
    #[must_use]
    /// Returns the raw contents of this item.
    fn as_str(&self) -> &str {
        &self.raw
    }
}

impl From<String> for IcsItem {
    /// Creates a new instance from valid iCalendar data.
    fn from(value: String) -> Self {
        IcsItem { raw: value }
    }
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
}

#[cfg(test)]
mod tests {
    use crate::{
        base::{Item as _, Storage},
        calendar::IcsItem,
    };

    #[test]
    fn test_single_line_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));

        let raw = ["BEGIN:VCARD", "UID:hel", "lo", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hel")));
        assert_eq!(item.ident(), String::from("hel"));

        let raw = [
            "BEGIN:VCARD",
            "UID:hello",
            "REV:20210307T195614Z\tthere",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hello")));
        assert_eq!(item.ident(), String::from("hello"));
    }

    #[test]
    fn test_multi_line_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "\tthere", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hellothere")));
        assert_eq!(item.ident(), String::from("hellothere"));

        let raw = [
            "BEGIN:VCARD",
            "UID:hello",
            "\tthere",
            "REV:20210307T195614Z",
            "\tnope",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), Some(String::from("hellothere")));
        assert_eq!(item.ident(), String::from("hellothere"));
    }

    #[test]
    fn test_missing_uid() {
        let raw = [
            "BEGIN:VCARD",
            "UIDX:hello",
            "REV:20210307T195614Z\tthere",
            "END:VCARD",
        ]
        .join("\r\n");
        let item = IcsItem::from(raw);
        assert_eq!(item.uid(), None);
        assert_eq!(item.ident(), item.hash().to_string());
    }

    #[test]
    fn test_storage_is_object_safe() {
        #[allow(dead_code)]
        fn dummy(_: Box<dyn Storage<IcsItem>>) {}
    }

    #[test]
    fn test_with_uid() {
        let raw = ["BEGIN:VCARD", "UID:hello", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        let item2 = item.with_uid("goodbye");
        assert_eq!(item2.uid(), Some(String::from("goodbye")));
        assert_eq!(item2.ident(), String::from("goodbye"));
    }

    #[test]
    fn test_with_uid_without_uid() {
        let raw = ["BEGIN:VCARD", "SUMMARY:hello", "END:VCARD"].join("\r\n");
        let item = IcsItem::from(raw);
        let item2 = item.with_uid("goodbye");
        assert_eq!(item2.uid(), None);
    }
}
