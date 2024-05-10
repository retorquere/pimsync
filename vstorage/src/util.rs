// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Miscellaneous helpers.
use sha2::{Digest, Sha256};
use vparser::Parser;

// TODO: See (in vdirsyncer-py) IGNORE_PROPS for more props that might make sense to ignore.
const ICS_FIELDS_TO_IGNORE: &[&str] = &[
    // Servers often mutate this; resulting in noise when comparing.
    "PRODID",
    // When the information was last revised.
    // I don't think that servers SHOULD modify this, but they often do.
    // See: https://www.rfc-editor.org/rfc/rfc5545#section-3.8.7.2
    "DTSTAMP",
    // Ditto
    // See: https://www.rfc-editor.org/rfc/rfc5545#section-3.8.7.3
    "LAST-MODIFIED",
];

/// Return the SHA256 hash of an icalendar or vcard.
pub(crate) fn hash(input: impl AsRef<str>) -> String {
    let mut hasher = Sha256::new();
    let parser = Parser::new(input.as_ref());
    for line in parser {
        if ICS_FIELDS_TO_IGNORE.contains(&line.name().as_ref()) {
            continue;
        }
        // TODO: strip/normalize timezones (tip: they are sometimes renamed)?
        // TODO: normalise order of lines inside each component?
        let raw = line.raw();
        if raw.is_empty() {
            continue;
        }
        // Use unfolded lines to ignore discrepancies in folding.
        hasher.update(line.unfolded().as_ref());
        hasher.update("\r\n"); // Included even for the last line.
    }
    format!("{:X}", hasher.finalize())
}

/// Replaces the UID for an input vobject.
///
/// Expects that the provided vobject has exactly one component of type VEVENT, VTODO, VJOURNAL,
/// VCARD.
pub(crate) fn replace_uid(orig: &str, new_uid: &str) -> String {
    let mut inside_component = false;
    let mut new = String::new();

    for line in Parser::new(orig) {
        if line.name() == "BEGIN"
            && ["VEVENT", "VTODO", "VJOURNAL", "VCARD"].contains(&line.value().as_ref())
        {
            inside_component = true;
        }
        if line.name() == "END"
            && ["VEVENT", "VTODO", "VJOURNAL", "VCARD"].contains(&line.value().as_ref())
        {
            inside_component = false;
        }
        if inside_component && line.name() == "UID" {
            new.push_str("UID:");
            new.push_str(new_uid);
            new.push_str("\r\n");
        } else {
            new.push_str(line.raw());
            new.push_str("\r\n");
        }
    }

    new
}

#[cfg(test)]
mod test {
    use crate::util::hash;

    #[test]
    fn compare_hashing_with_and_without_prodid() {
        let without_prodid = [
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");
        let with_prodid = [
            "PRODID:test-client",
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");

        assert_eq!(hash(without_prodid), hash(with_prodid));
    }

    #[test]
    fn compare_hashing_with_different_folding() {
        let first = [
            "DESCRIPTION:Voor meer informatie zie https://nluug.nl/evenementen/nluug/na",
            " jaarsconferentie-2023/",
        ]
        .join("\r\n");
        let second = [
            "DESCRIPTION:Voor meer informatie zie https:",
            " //nluug.nl/evenementen/nluug/najaarsconferentie-2023/",
        ]
        .join("\r\n");

        assert_eq!(hash(first), hash(second));
    }
}
