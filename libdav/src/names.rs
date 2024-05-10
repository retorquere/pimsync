// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Names of common dav attributes.

use crate::Property;

/// Namespace for properties defined in the WebDav specifications.
pub const DAV: &str = "DAV:";
/// Namespace for properties defined in the CalDav specifications.
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
/// Namespace for properties defined in the CardDav specifications.
pub const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";

pub const COLLECTION: Property = Property::from_static(DAV, "collection");
pub const DISPLAY_NAME: Property = Property::from_static(DAV, "displayname");
pub const GETCONTENTTYPE: Property = Property::from_static(DAV, "getcontenttype");
pub const GETETAG: Property = Property::from_static(DAV, "getetag");
pub const HREF: Property = Property::from_static(DAV, "href");
pub const RESOURCETYPE: Property = Property::from_static(DAV, "resourcetype");
pub const RESPONSE: Property = Property::from_static(DAV, "response");
pub const STATUS: Property = Property::from_static(DAV, "status");
pub const PROPSTAT: Property = Property::from_static(DAV, "propstat");
pub const SUPPORTED_REPORT_SET: Property = Property::from_static(DAV, "supported-report-set");
pub const SYNC_COLLECTION: Property = Property::from_static(DAV, "sync-collection");
pub const CURRENT_USER_PRINCIPAL: Property = Property::from_static(DAV, "current-user-principal");

pub const CALENDAR: Property = Property::from_static(CALDAV, "calendar");
/// Defined in <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
pub const CALENDAR_HOME_SET: Property = Property::from_static(CALDAV, "calendar-home-set");
pub const CALENDAR_COLOUR: Property =
    Property::from_static("http://apple.com/ns/ical/", "calendar-color");
pub const CALENDAR_DATA: Property = Property::from_static(CALDAV, "calendar-data");

pub const ADDRESSBOOK: Property = Property::from_static(CARDDAV, "addressbook");
pub const ADDRESSBOOK_HOME_SET: Property =
    Property::from_static("urn:ietf:params:xml:ns:carddav", "addressbook-home-set");
pub const ADDRESS_DATA: Property = Property::from_static(CARDDAV, "address-data");
