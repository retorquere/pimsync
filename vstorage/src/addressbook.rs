//! Types and functions specific to address books and contacts.
use libdav::{names, PropertyName};

use crate::base::Property;

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
    /// Returns the name of the corresponding CardDAV property.
    #[must_use]
    pub fn dav_propname(&self) -> &PropertyName<'_, '_> {
        match self {
            AddressBookProperty::DisplayName => &names::DISPLAY_NAME,
            AddressBookProperty::Description => &names::ADDRESSBOOK_DESCRIPTION,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            AddressBookProperty::DisplayName => "displayname",
            AddressBookProperty::Description => "description",
        }
    }

    #[must_use]
    pub fn known_properties() -> &'static [Property] {
        &[
            Property::AddressBook(AddressBookProperty::DisplayName),
            Property::AddressBook(AddressBookProperty::Description),
        ]
    }

    #[must_use]
    pub fn filename(&self) -> &'static str {
        match self {
            AddressBookProperty::DisplayName => "displayname",
            AddressBookProperty::Description => "description",
        }
    }
}

impl From<AddressBookProperty> for Property {
    fn from(value: AddressBookProperty) -> Self {
        Property::AddressBook(value)
    }
}
