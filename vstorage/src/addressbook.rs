//! Types and functions specific to address books and contacts.
use libdav::{names, PropertyName};

use crate::{
    base::{uid, Item, Property},
    util::{replace_uid, ItemHash},
};

/// Immutable wrapper around a `VCARD`.
///
/// Note that this is not a proper validating parser for vcard; it's a very simple one with the
/// sole purpose of extracting a UID. Proper parsing of components is out of scope, since we want
/// to enable operating on potentially invalid items too.
#[derive(Debug, Clone)]
pub struct VcardItem {
    raw: String,
}

impl Item for VcardItem {
    type Property = AddressBookProperty;
    /// Returns a unique identifier for this item.
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
        VcardItem::from(replace_uid(&self.raw, new_uid))
    }

    #[inline]
    #[must_use]
    /// Returns the raw contents of this item.
    fn as_str(&self) -> &str {
        &self.raw
    }
}

impl From<String> for VcardItem {
    /// Creates a new instance from valid Vcard data.
    fn from(value: String) -> Self {
        VcardItem { raw: value }
    }
}

/// Properties supported for address books.
///
/// This is strongly based on the properties supported by `CardDav`.
#[non_exhaustive]
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum AddressBookProperty {
    DisplayName,
    Description,
}

impl AddressBookProperty {
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
}
