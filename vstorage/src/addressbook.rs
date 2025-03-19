//! Types and functions specific to address books and contacts.
use libdav::{names, PropertyName};

use crate::base::{ItemKind, Property};

/// Immutable wrapper around a `VCARD`.
///
/// Note that this is not a proper validating parser for vcard; it's a very simple one with the
/// sole purpose of extracting a UID. Proper parsing of components is out of scope, since we want
/// to enable operating on potentially invalid items too.
#[derive(Debug, Clone)]
pub struct VcardItem;

impl ItemKind for VcardItem {
    type Property = AddressBookProperty;
}

/// Properties supported for address books.
///
/// This is strongly based on the properties supported by `CardDav`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum AddressBookProperty {
    /// A user-friendly name for a collection.
    ///
    /// It is recommended to show this name in user interfaces.
    DisplayName,
    /// Human readable description of the collection.
    Description,
}

impl AddressBookProperty {
    /// Returns the name of the corresponding DAV property.
    #[must_use]
    pub fn dav_propname(&self) -> &PropertyName<'_, '_> {
        match self {
            AddressBookProperty::DisplayName => &names::DISPLAY_NAME,
            AddressBookProperty::Description => &names::ADDRESSBOOK_DESCRIPTION,
        }
    }
}

impl Property for AddressBookProperty {
    fn name(&self) -> &str {
        match self {
            AddressBookProperty::DisplayName => "displayname",
            AddressBookProperty::Description => "description",
        }
    }

    fn known_properties() -> &'static [Self] {
        &[
            AddressBookProperty::DisplayName,
            AddressBookProperty::Description,
        ]
    }

    fn filename(&self) -> &'static str {
        match self {
            AddressBookProperty::DisplayName => "displayname",
            AddressBookProperty::Description => "description",
        }
    }
}
