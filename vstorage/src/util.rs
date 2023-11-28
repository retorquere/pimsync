// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Miscellaneous helpers.
use sha2::{Digest, Sha256};
use vparser::Parser;

/// Return the SHA256 hash of an icalendar or vcard.
pub(crate) fn hash<S: AsRef<str>>(input: S) -> String {
    // TODO: See (in vdirsyncer-py) IGNORE_PROPS for more props that might make sense to ignore.
    let mut hasher = Sha256::new();
    let parser = Parser::new(input.as_ref());
    for line in parser {
        if line.name() == "PRODID" {
            continue; // Frequently mutated and only adds noise when comparing.
        }
        // TODO: strip/normalize timezones (tip: they are sometimes renamed)?
        // TODO: normalise order?
        let raw = line.raw();
        if raw.is_empty() {
            continue;
        }
        hasher.update(raw);
        hasher.update("\r\n"); // Included even for the last line.
    }
    format!("{:X}", hasher.finalize())
}

#[cfg(test)]
mod test {
    use crate::util::hash;

    #[test]
    fn compare_hashing_with_and_without_prodid() {
        let without_prodid = vec![
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
        let with_prodid = vec![
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
}
