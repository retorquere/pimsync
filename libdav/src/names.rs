// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Names of common dav attributes and properties.

use crate::PropertyName;

/// Namespace for properties defined in the WebDav specifications.
pub const DAV: &str = "DAV:";
/// Namespace for properties defined in the CalDav specifications.
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
/// Namespace for properties defined in the CardDav specifications.
pub const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
/// Namespace for properties defined by Apple / ical.
pub const APPLE: &str = "http://apple.com/ns/ical/";

pub const COLLECTION: PropertyName = PropertyName::from_static(DAV, "collection");
/// Property name for collections display name.
///
/// From <https://www.rfc-editor.org/rfc/rfc3744#section-4>:
///
/// > A principal MUST have a non-empty DAV:displayname property
pub const DISPLAY_NAME: PropertyName = PropertyName::from_static(DAV, "displayname");
pub const GETCONTENTTYPE: PropertyName = PropertyName::from_static(DAV, "getcontenttype");
pub const GETETAG: PropertyName = PropertyName::from_static(DAV, "getetag");
pub const HREF: PropertyName = PropertyName::from_static(DAV, "href");
pub const RESOURCETYPE: PropertyName = PropertyName::from_static(DAV, "resourcetype");
pub const RESPONSE: PropertyName = PropertyName::from_static(DAV, "response");
pub const STATUS: PropertyName = PropertyName::from_static(DAV, "status");
pub const PROPSTAT: PropertyName = PropertyName::from_static(DAV, "propstat");
pub const SUPPORTED_REPORT_SET: PropertyName =
    PropertyName::from_static(DAV, "supported-report-set");
pub const SYNC_COLLECTION: PropertyName = PropertyName::from_static(DAV, "sync-collection");
pub const CURRENT_USER_PRINCIPAL: PropertyName =
    PropertyName::from_static(DAV, "current-user-principal");

pub const CALENDAR: PropertyName = PropertyName::from_static(CALDAV, "calendar");
/// From: <https://www.rfc-editor.org/rfc/rfc4791#section-5.2.1>
pub const CALENDAR_DESCRIPTION: PropertyName =
    PropertyName::from_static(CALDAV, "calendar-description");
/// Defined in <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
pub const CALENDAR_HOME_SET: PropertyName = PropertyName::from_static(CALDAV, "calendar-home-set");
/// A calendar's colour.
///
/// This is not a formally standardised property, but is relatively widespread. The value of this
/// property should be an unescaped hex value with a leading pound sign (e.g. `#ff0000`).
pub const CALENDAR_COLOUR: PropertyName =
    PropertyName::from_static("http://apple.com/ns/ical/", "calendar-color");
pub const CALENDAR_DATA: PropertyName = PropertyName::from_static(CALDAV, "calendar-data");
pub const CALENDAR_ORDER: PropertyName = PropertyName::from_static(APPLE, "calendar-order");

pub const ADDRESSBOOK: PropertyName = PropertyName::from_static(CARDDAV, "addressbook");
/// From: <https://www.rfc-editor.org/rfc/rfc6352#section-6.2.1>
pub const ADDRESSBOOK_DESCRIPTION: PropertyName =
    PropertyName::from_static(CARDDAV, "addressbook-description");
pub const ADDRESSBOOK_HOME_SET: PropertyName =
    PropertyName::from_static("urn:ietf:params:xml:ns:carddav", "addressbook-home-set");
pub const ADDRESS_DATA: PropertyName = PropertyName::from_static(CARDDAV, "address-data");
