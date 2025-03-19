// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Miscellaneous helpers.
use std::{collections::VecDeque, str::FromStr, sync::Arc};

use sha2::{Digest, Sha256};
use vparser::Parser;

// TODO: See (in vdirsyncer) IGNORE_PROPS for more props that might make sense to ignore.
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

/// The hash of an item. See [`Item::hash`].
#[derive(Default, PartialEq, Clone)]
pub struct ItemHash(Arc<[u8; 32]>);

// TODO: must confirm that this matches previous impl to ensure statusDb makes sense.
impl std::fmt::Display for ItemHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0.iter() {
            write!(f, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for ItemHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ItemHash(")?;
        for byte in self.0.iter() {
            write!(f, "{byte:02X}")?;
        }
        write!(f, ")")
    }
}

/// Error returned by [`ItemHash::from_str`].
#[derive(Debug, thiserror::Error)]
pub enum ItemHashError {
    #[error("Hash must be exactly 64 characters long")]
    InvalidLength,
    #[error("Invalid character in hash representation")]
    InvalidCharacter,
}

impl FromStr for ItemHash {
    type Err = ItemHashError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64 {
            return Err(ItemHashError::InvalidLength);
        }

        let mut bytes = [0u8; 32];
        for (i, chunk) in value.as_bytes().chunks(2).enumerate() {
            let hex = std::str::from_utf8(chunk).map_err(|_| ItemHashError::InvalidCharacter)?;
            bytes[i] = u8::from_str_radix(hex, 16).map_err(|_| ItemHashError::InvalidCharacter)?;
        }

        Ok(ItemHash(Arc::new(bytes)))
    }
}

/// Return the SHA256 hash of an icalendar or vcard.
pub(crate) fn hash(input: impl AsRef<str>) -> ItemHash {
    let mut hasher = Sha256::new();
    let parser = Parser::new(input.as_ref());

    let mut in_tz = false;
    let mut tz_lines = VecDeque::new();

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

        // Swallow timezones, so we place them at the end.
        if line.name() == "BEGIN" && line.value() == "VTIMEZONE" {
            in_tz = true;
        }
        if in_tz {
            if line.name() == "END" && line.value() == "VTIMEZONE" {
                in_tz = false;
            }
            tz_lines.push_back(line);
            continue;
        }

        // Place all timezones at the end to normalise discrepancies in ordering.
        if line.name() == "END" && line.value() == "VCALENDAR" {
            while let Some(l) = tz_lines.pop_front() {
                hasher.update(l.unfolded().as_ref());
                hasher.update("\r\n"); // Included even for the last line.
            }
        }

        // Use unfolded lines to ignore discrepancies in folding.
        hasher.update(line.unfolded().as_ref());
        hasher.update("\r\n"); // Included even for the last line.
    }

    // Only extremely malformed entries will match this branch,
    // well-formed icalendar files will have drained this queue already.
    while let Some(l) = tz_lines.pop_front() {
        hasher.update(l.unfolded().as_ref());
        hasher.update("\r\n"); // Included even for the last line.
    }

    ItemHash(Arc::from(<[u8; 32]>::from(hasher.finalize())))
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

        assert_eq!(hash(&without_prodid), hash(with_prodid));
        assert_eq!(
            hash(without_prodid).to_string(),
            "E6DF19EB84E6DCE351EFB015D25C76D31A1FE09F2A8732BE6BC565A01EFA1A41"
        );
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

        assert_eq!(hash(&first), hash(second));
        assert_eq!(
            hash(first).to_string(),
            "9FCE34302FB7B6677542987089C91FDDF79F18F1D42862B03B1DEDF8E72F0CE2"
        );
    }

    #[test]
    fn hash_with_reordered_timezone() {
        let timezone_first = [
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "CALSCALE:GREGORIAN",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Amsterdam",
            "X-LIC-LOCATION:Europe/Amsterdam",
            "BEGIN:DAYLIGHT",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "TZNAME:CEST",
            "DTSTART:19700329T020000",
            "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU",
            "END:DAYLIGHT",
            "BEGIN:STANDARD",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "TZNAME:CET",
            "DTSTART:19701025T030000",
            "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU",
            "END:STANDARD",
            "END:VTIMEZONE",
            "BEGIN:VEVENT",
            "UID:DF1E090791D8A93F3B530CFDA9CBFC0573CE3AB61C63A02AA33051B903F68A82",
            "SUMMARY:NLUUG najaarsconferentie 2023",
            "DESCRIPTION:Voor meer informatie zie https://nluug.nl/evenementen/nluug/najaarsconferentie-2023/",
            "DTSTART;TZID=Europe/Amsterdam:20231128T083000",
            "DTEND;TZID=Europe/Amsterdam:20231128T180000",
            "LOCATION:Winthontlaan 4-6, Utrecht, The Netherlands",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");
        let timezone_last = [
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "CALSCALE:GREGORIAN",
            "BEGIN:VEVENT",
            "UID:DF1E090791D8A93F3B530CFDA9CBFC0573CE3AB61C63A02AA33051B903F68A82",
            "SUMMARY:NLUUG najaarsconferentie 2023",
            "DESCRIPTION:Voor meer informatie zie https://nluug.nl/evenementen/nluug/najaarsconferentie-2023/",
            "DTSTART;TZID=Europe/Amsterdam:20231128T083000",
            "DTEND;TZID=Europe/Amsterdam:20231128T180000",
            "LOCATION:Winthontlaan 4-6, Utrecht, The Netherlands",
            "END:VEVENT",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Amsterdam",
            "X-LIC-LOCATION:Europe/Amsterdam",
            "BEGIN:DAYLIGHT",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "TZNAME:CEST",
            "DTSTART:19700329T020000",
            "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU",
            "END:DAYLIGHT",
            "BEGIN:STANDARD",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "TZNAME:CET",
            "DTSTART:19701025T030000",
            "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU",
            "END:STANDARD",
            "END:VTIMEZONE",
            "END:VCALENDAR",
        ]
        .join("\r\n")  ;

        assert_eq!(hash(timezone_first), hash(timezone_last));
    }
}
