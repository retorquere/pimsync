// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::{borrow::Cow, collections::HashMap};

use vparser::{ContentLine, Parser};

/// A simple component model that only cares about the basic structure.
///
/// This is used to split components and other simple operations. However, this
/// is not a full parser. It won't validate much beyond `BEGIN` and `END`
/// properly matching. The intent of this parser is not to be validating, but
/// to be very tolerant with inputs, so as to allow operating on somewhat
/// invalid inputs.
///
/// # Known Issues
///
/// Works only with iCalendar, not with vCard.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Component<'a> {
    kind: Cow<'a, str>,
    lines: Vec<ContentLine<'a>>,
    subcomponents: Vec<Component<'a>>,
    uid: Option<Cow<'a, str>>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub(crate) enum ComponentError {
    #[error("unknown (or unimplemented) component: {0}")]
    UnknownComponent(String),
    #[error("found data after END of root component")]
    DataAfterEnd,
    #[error("reached end of file while parsing data")]
    UnexpectedEof,
    #[error("unbalanced BEGIN and END lines")]
    WrongEnd,
    #[error("END line had no matching BEGIN line")]
    EndWithoutBegin,
    #[error("found data after last END: line")]
    DataOutsideBeginEnd,
}

impl<'a> Component<'a> {
    fn new(kind: Cow<'a, str>) -> Self {
        Component {
            kind,
            lines: Vec::new(),
            subcomponents: Vec::new(),
            uid: None,
        }
    }

    /// Parse a component from a raw string input.
    pub(crate) fn parse(input: &str) -> Result<Component, ComponentError> {
        let mut stack = Vec::new();
        let mut current: Option<Component> = None;

        let mut parser = Parser::new(input);
        while let Some(line) = parser.next() {
            if line.name() == "BEGIN" {
                let new = Component::new(line.value());
                if let Some(previous) = current.replace(new) {
                    stack.push(previous);
                }
            } else if line.name() == "END" {
                let ending = current.take().ok_or(ComponentError::EndWithoutBegin)?;
                if line.value() != ending.kind {
                    return Err(ComponentError::WrongEnd);
                }
                match stack.pop() {
                    Some(mut previous) => {
                        previous.subcomponents.push(ending);
                        current = Some(previous);
                    }
                    None => {
                        return if parser.next().is_some_and(|line| !line.raw().is_empty()) {
                            Err(ComponentError::DataAfterEnd)
                        } else {
                            Ok(ending)
                        };
                    }
                }
            } else if let Some(ref mut current) = current {
                if line.name() == "UID" {
                    current.uid = Some(line.value());
                }
                current.lines.push(line);
            } else {
                return Err(ComponentError::DataOutsideBeginEnd);
            };
        }

        Err(ComponentError::UnexpectedEof)
    }

    // Breaks up a component collection into individual components.
    //
    // For a calendar with multiple `VEVENT`s and `VTIMEZONE`, it will return individual `VEVENT`
    // with the `VTIMEZONE` duplicated into each one, making them fully standalone components.
    pub(crate) fn into_split_collection(
        self: Component<'a>,
    ) -> Result<Vec<Component<'a>>, ComponentError> {
        let mut inline = Vec::new();
        let mut items_with_uid = HashMap::new();
        let mut items_without_uid = Vec::new();

        self.split_inner(&mut inline, &mut items_with_uid, &mut items_without_uid)?;

        let items_with_timezones = items_with_uid
            .into_values()
            .map(|mut wrapper| {
                for entry in &mut *wrapper.subcomponents {
                    // Clone here because `append` empties the passed input.
                    entry.subcomponents.append(&mut (inline.clone()));
                    // FIXME: this copies all timezones into all components. I can do better.
                }
                wrapper
            })
            .collect();

        Ok(items_with_timezones)
    }

    /// Split components inside this one recursively.
    ///
    /// Subcomponents are split into three groups:
    ///
    /// - `inline`: those that must be copied inline (e.g.: `VTIMEZONE`)
    /// - `items`: items with a UID (which is the key for the `HashMap`.
    /// - `without_uid`: items which as missing a UID.
    ///
    /// Both `items` and `without_uid` are free-standing items for [`Collection`]s.
    ///
    /// Calendar components will be put inside their own wrapper (e.g.: a `VEVENT` will be wrapped
    /// inside its own `VCALENDAR`.
    ///
    /// [`Collection`]: crate::base::Collection
    fn split_inner(
        self: Component<'a>,
        inline: &mut Vec<Component<'a>>,
        items: &mut HashMap<Cow<'a, str>, Component<'a>>,
        without_uid: &mut Vec<Component<'a>>,
    ) -> Result<(), ComponentError> {
        match self.kind.as_ref() {
            "VTIMEZONE" => {
                inline.push(self);
            }
            "VTODO" | "VJOURNAL" | "VEVENT" => {
                // Hint: we don't recurse into these, so VALARM components remain untouched.
                match &self.uid {
                    Some(uid) => {
                        items
                            .entry(uid.clone())
                            .or_insert(Component {
                                kind: Cow::Borrowed("VCALENDAR"),
                                lines: Vec::new(),
                                subcomponents: Vec::new(),
                                uid: None,
                            })
                            .subcomponents
                            .push(self);
                    }
                    None => {
                        without_uid.push(self);
                    }
                }
            }
            "VCALENDAR" => {
                for component in self.subcomponents {
                    Self::split_inner(component, inline, items, without_uid)?;
                }
            }
            kind => return Err(ComponentError::UnknownComponent(kind.to_string())),
        }

        Ok(())
    }
}

impl ToString for Component<'_> {
    /// Returns a fully encoded representation of this item.
    fn to_string(&self) -> String {
        let mut raw = String::new();
        raw.push_str("BEGIN:");
        raw.push_str(self.kind.as_ref());
        raw.push_str("\r\n");
        for line in &self.lines {
            raw.push_str(line.raw());
            raw.push_str("\r\n");
        }
        for component in &self.subcomponents {
            raw.push_str(&component.to_string());
        }
        raw.push_str("END:");
        raw.push_str(self.kind.as_ref());
        raw.push_str("\r\n");

        raw
    }
}

#[cfg(test)]
mod test {
    use std::borrow::Cow;

    use crate::simple_component::ComponentError;

    #[test]
    fn test_parse_and_split_collection() {
        use super::Component;

        let calendar = vec![
            "BEGIN:VCALENDAR",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Rome",
            "X-LIC-LOCATION:Europe/Rome",
            "BEGIN:DAYLIGHT",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "TZNAME:CEST",
            "DTSTART:19700329T020000",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
            "END:DAYLIGHT",
            "BEGIN:STANDARD",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "TZNAME:CET",
            "DTSTART:19701025T030000",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10",
            "END:STANDARD",
            "END:VTIMEZONE",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "X-SOMETHING:r",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party (copy)",
            "X-SOMETHING:s",
            "UID:b8d52b8b-dd6b-4ef9-9249-0ad7c28f9e5a",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");

        let component = Component::parse(&calendar).unwrap();
        assert_eq!(component.kind, Cow::Borrowed("VCALENDAR"));

        let serialised_split = Component::into_split_collection(component)
            .unwrap()
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>();

        let expected_first = vec![
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party (copy)",
            "X-SOMETHING:s",
            "UID:b8d52b8b-dd6b-4ef9-9249-0ad7c28f9e5a",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Rome",
            "X-LIC-LOCATION:Europe/Rome",
            "BEGIN:DAYLIGHT",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "TZNAME:CEST",
            "DTSTART:19700329T020000",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
            "END:DAYLIGHT",
            "BEGIN:STANDARD",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "TZNAME:CET",
            "DTSTART:19701025T030000",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10",
            "END:STANDARD",
            "END:VTIMEZONE",
            "END:VEVENT",
            "END:VCALENDAR",
            "",
        ]
        .join("\r\n");
        let expected_second = vec![
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "X-SOMETHING:r",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Rome",
            "X-LIC-LOCATION:Europe/Rome",
            "BEGIN:DAYLIGHT",
            "TZOFFSETFROM:+0100",
            "TZOFFSETTO:+0200",
            "TZNAME:CEST",
            "DTSTART:19700329T020000",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3",
            "END:DAYLIGHT",
            "BEGIN:STANDARD",
            "TZOFFSETFROM:+0200",
            "TZOFFSETTO:+0100",
            "TZNAME:CET",
            "DTSTART:19701025T030000",
            "RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10",
            "END:STANDARD",
            "END:VTIMEZONE",
            "END:VEVENT",
            "END:VCALENDAR",
            "",
        ]
        .join("\r\n");

        // Comparing like this since the order is not deterministic.
        assert!(serialised_split.iter().any(|c| **c == expected_first));
        assert!(serialised_split.iter().any(|c| **c == expected_second));
    }

    #[test]
    fn test_missing_end() {
        use super::Component;

        let calendar = vec![
            "BEGIN:VCALENDAR",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Rome",
            "END:VTIMEZONE",
            "BEGIN:VEVENT",
            "SUMMARY:This event is probably invalid due to missing fields",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
        ]
        .join("\r\n");

        assert_eq!(
            Component::parse(&calendar),
            Err(ComponentError::UnexpectedEof)
        );
    }

    #[test]
    fn test_unknown_kind() {
        use super::Component;

        let calendar = vec![
            "BEGIN:VCALENDAR",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Rome",
            "END:VTIMEZONE",
            "BEGIN:VEVENT",
            "SUMMARY:This event is probably invalid due to missing fields",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "BEGIN:VAUTOMOBILE",
            "END:VAUTOMOBILE",
            "END:VCALENDAR",
        ]
        .join("\r\n");

        assert_eq!(
            Component::parse(&calendar).unwrap().into_split_collection(),
            Err(ComponentError::UnknownComponent("VAUTOMOBILE".to_string()))
        );
    }

    #[test]
    fn test_multiline_uid() {
        use super::Component;

        let calendar = vec![
            "BEGIN:VCALENDAR",
            "BEGIN:VTIMEZONE",
            "TZID:Europe/Rome",
            "END:VTIMEZONE",
            "BEGIN:VEVENT",
            "SUMMARY:This event is probably invalid due to missing fields",
            "UID:horrible-",
            " example",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");

        let calendar = Component::parse(&calendar)
            .unwrap()
            .into_split_collection()
            .unwrap()
            .pop()
            .unwrap();

        assert_eq!(
            calendar.subcomponents[0].uid.as_ref().unwrap(),
            "horrible-example"
        );
    }
}
