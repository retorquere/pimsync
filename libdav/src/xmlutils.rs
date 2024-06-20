// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Utilities for handling XML data.
use std::borrow::Cow;
use std::str::FromStr;

use http::status::InvalidStatusCode;
use http::StatusCode;
use percent_encoding::percent_encode;
use percent_encoding::{percent_decode_str, AsciiSet, NON_ALPHANUMERIC};
use roxmltree::Node;

use crate::dav::{check_status, WebDavError};
use crate::names;
use crate::PropertyName;

/// Characters that are escaped for hrefs.
pub const DISALLOWED_FOR_HREF: &AsciiSet = &NON_ALPHANUMERIC.remove(b'/').remove(b'.');

/// Check all the statuses in a `multistatus` response.
///
/// # Errors
///
/// - If any of the `<DAV:status>` nodes is missing the status text, returns
///   [`WebDavError::InvalidResponse`].
///
/// - If the text inside a `<DAV:status>` node is not a valid status line, returns
///   [`WebDavError::InvalidStatusCode`].
///
/// - If any of the statuses are non-success, returns [`WebDavError::BadStatusCode`].
pub fn check_multistatus(root: Node) -> Result<(), WebDavError> {
    let statuses = root
        .descendants()
        .filter(|node| node.tag_name() == names::STATUS);
    for status in statuses {
        let status = status.text().ok_or(WebDavError::InvalidResponse(
            "missing text inside 'DAV:status'".into(),
        ))?;
        check_status(parse_statusline(status)?)?;
    }

    Ok(())
}

/// Parses a status line string into a [`StatusCode`].
///
/// Example input string: `HTTP/1.1 200 OK`.
///
/// # See also
///
/// - The [status element](https://www.rfc-editor.org/rfc/rfc2518#section-12.9.1.2)
/// - [Status-Line](https://www.rfc-editor.org/rfc/rfc2068#section-6.1)
///
/// # Errors
///
/// If the input string does not match a status line.
pub fn parse_statusline(status_line: impl AsRef<str>) -> Result<StatusCode, InvalidStatusCode> {
    let mut iter = status_line.as_ref().splitn(3, ' ');
    iter.next();
    let code = iter.next().unwrap_or("");
    StatusCode::from_str(code)
}

/// Render an empty XML node.
pub(crate) fn render_xml(name: &PropertyName) -> String {
    if let Some(ns) = name.namespace() {
        format!("<{0} xmlns=\"{1}\"/>", name.name(), ns)
    } else {
        format!("<{0}/>", name.name())
    }
}

/// Render an XML node with optional text.
pub fn render_xml_with_text(name: &PropertyName, text: Option<impl AsRef<str>>) -> String {
    match (name.namespace(), text) {
        (None, None) => format!("<{}/>", name.name()),
        (None, Some(t)) => format!("<{0}>{1}</{0}>", name.name(), escape_text(t.as_ref())),
        (Some(ns), None) => format!("<{0} xmlns=\"{ns}\"/>", name.name()),
        (Some(ns), Some(t)) => format!(
            "<{0} xmlns=\"{ns}\">{1}</{0}>",
            name.name(),
            escape_text(t.as_ref())
        ),
    }
}

/// Replaces characters that need to be escaped in texts.
///
/// `<` --> `&lt;`
/// `>` --> `&gt;`
/// `&` --> `&amp;`
///
/// This IS NOT usable in other contexts of XML encoding.
#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn escape_text(raw: &str) -> Cow<str> {
    // This function is strongly based on `escape_partial` from `quick-xml`:
    {
        // The MIT License (MIT)
        //
        // Copyright (c) 2016 Johann Tuffe
        //
        // Permission is hereby granted, free of charge, to any person obtaining a copy
        // of this software and associated documentation files (the "Software"), to deal
        // in the Software without restriction, including without limitation the rights
        // to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
        // copies of the Software, and to permit persons to whom the Software is
        // furnished to do so, subject to the following conditions:
        //
        //
        // The above copyright notice and this permission notice shall be included in
        // all copies or substantial portions of the Software.
        //
        //
        // THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
        // IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
        // FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL THE
        // AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
        // LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
        // OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
        // THE SOFTWARE.
        let bytes = raw.as_bytes();
        let mut escaped = None;
        let mut iter = bytes.iter();
        let mut pos = 0;
        while let Some(i) = iter.position(|&b| matches!(b, b'<' | b'>' | b'&')) {
            let escaped = escaped.get_or_insert_with(|| Vec::with_capacity(raw.len()));
            let new_pos = pos + i;
            escaped.extend_from_slice(&bytes[pos..new_pos]);
            match bytes[new_pos] {
                b'<' => escaped.extend_from_slice(b"&lt;"),
                b'>' => escaped.extend_from_slice(b"&gt;"),
                b'&' => escaped.extend_from_slice(b"&amp;"),
                _ => unreachable!("Only '<', '>' and '&', are escaped"),
            }
            pos = new_pos + 1;
        }

        if let Some(mut escaped) = escaped {
            if let Some(raw) = bytes.get(pos..) {
                escaped.extend_from_slice(raw);
            }
            // SAFETY: we operate on UTF-8 input and search for an one byte chars only,
            // so all slices that was put to the `escaped` is a valid UTF-8 encoded strings
            // TODO: Can be replaced with the following unsafe snippet:
            // Cow::Owned(unsafe { String::from_utf8_unchecked(escaped) })
            Cow::Owned(
                String::from_utf8(escaped).expect("manually escaped string must be valid utf-8"),
            )
        } else {
            Cow::Borrowed(raw)
        }
    }
    // End copied code.
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use crate::xmlutils::escape_text;

    #[test]
    fn test_escape_text() {
        match escape_text("HELLO THERE") {
            Cow::Borrowed(s) => assert_eq!(s, "HELLO THERE"),
            Cow::Owned(_) => panic!("expected Borrowed, got Owned"),
        }
        match escape_text("HELLO <") {
            Cow::Borrowed(_) => panic!("expected Owned, got Borrowed"),
            Cow::Owned(s) => assert_eq!(s, "HELLO &lt;"),
        }
        match escape_text("HELLO &lt;") {
            Cow::Borrowed(_) => panic!("expected Owned, got Borrowed"),
            Cow::Owned(s) => assert_eq!(s, "HELLO &amp;lt;"),
        }
        match escape_text("你吃过了吗？") {
            Cow::Borrowed(s) => assert_eq!(s, "你吃过了吗？"),
            Cow::Owned(_) => panic!("expected Borrowed, got Owned"),
        }
    }
}

/// Find an `href` node and return its unescaped text value.
pub(crate) fn get_unquoted_href<'a>(node: &'a Node) -> Result<Cow<'a, str>, WebDavError> {
    Ok(node
        .descendants()
        .find(|node| node.tag_name() == crate::names::HREF)
        .ok_or(WebDavError::InvalidResponse(
            "missing href in response".into(),
        ))?
        .text()
        .map(percent_decode_str)
        .ok_or(WebDavError::InvalidResponse("missing text in href".into()))?
        .decode_utf8()?)
}

// URL-encodes an href.
//
// Obviously the input parameter MUST NOT be url-encoded.
pub(crate) fn quote_href(href: &[u8]) -> Cow<'_, str> {
    Cow::from(percent_encode(href, DISALLOWED_FOR_HREF))
}

#[inline]
pub(crate) fn get_newline_corrected_text(
    node: &Node,
    property: &PropertyName<'_, '_>,
) -> Result<String, WebDavError> {
    let raw_data = node
        .descendants()
        .find(|node| node.tag_name() == *property)
        .ok_or(WebDavError::InvalidResponse(
            format!("missing {} in response", property.name()).into(),
        ))?
        .text()
        .ok_or(WebDavError::InvalidResponse(
            "missing text in property".into(),
        ))?;

    // "\r\n" is usually converted into "\n" during. This needs to be undone.
    //
    // See: https://github.com/RazrFalcon/roxmltree/issues/102
    // See: https://www.w3.org/TR/xml/#sec-line-ends
    // See: https://www.rfc-editor.org/rfc/rfc4791#section-9.6

    let mut result = String::new();
    let mut last_end = 0;
    for (start, part) in raw_data.match_indices('\n') {
        result.push_str(&raw_data[last_end..start]);
        result.push_str("\r\n");
        last_end = start + part.len();
    }
    result.push_str(&raw_data[last_end..raw_data.len()]);
    Ok(result)
}

#[cfg(test)]
mod test {
    use crate::{names, xmlutils::get_newline_corrected_text};

    #[test]
    fn test_get_newline_corrected_text_without_returns() {
        let without_returns ="<ns0:multistatus xmlns:ns0=\"DAV:\" xmlns:ns1=\"urn:ietf:params:xml:ns:caldav\"><ns0:response><ns0:href>/user/calendars/qdBEnN9jwjQFLry4/1ehsci7nhH31.ics</ns0:href><ns0:propstat><ns0:status>HTTP/1.1 200 OK</ns0:status><ns0:prop><ns0:getetag>\"2d2c827debd802fb3844309b53254b90dd7fd900\"</ns0:getetag><ns1:calendar-data>BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:-//hacksw/handcal//NONSGML v1.0//EN\nBEGIN:VEVENT\nSUMMARY:hello\\, testing\nDTSTART:19970714T170000Z\nDTSTAMP:19970610T172345Z\nUID:92gDWceCowpO\nEND:VEVENT\nEND:VCALENDAR\n</ns1:calendar-data></ns0:prop></ns0:propstat></ns0:response></ns0:multistatus>";
        let expected = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//hacksw/handcal//NONSGML v1.0//EN\r\nBEGIN:VEVENT\r\nSUMMARY:hello\\, testing\r\nDTSTART:19970714T170000Z\r\nDTSTAMP:19970610T172345Z\r\nUID:92gDWceCowpO\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let doc = roxmltree::Document::parse(without_returns).unwrap();
        let responses = doc
            .root_element()
            .descendants()
            .find(|node| node.tag_name() == names::RESPONSE)
            .unwrap();
        assert_eq!(
            get_newline_corrected_text(&responses, &names::CALENDAR_DATA).unwrap(),
            expected
        );
    }

    #[test]
    fn test_get_newline_corrected_text_with_returns() {
        let with_returns= "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<multistatus xmlns=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\n  <response>\n    <href>/dav/calendars/user/vdirsyncer@fastmail.com/UvrlExcG9Jp0gEzQ/2H8kQfNQj8GP.ics</href>\n    <propstat>\n      <prop>\n        <getetag>\"4d92fc1c8bdc18bbf83caf34eeab7e7167eb292e\"</getetag>\n        <C:calendar-data><![CDATA[BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//hacksw/handcal//NONSGML v1.0//EN\r\nBEGIN:VEVENT\r\nUID:jSayX7OSdp3V\r\nDTSTAMP:19970610T172345Z\r\nDTSTART:19970714T170000Z\r\nSUMMARY:hello\\, testing\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n]]></C:calendar-data>\n      </prop>\n      <status>HTTP/1.1 200 OK</status>\n    </propstat>\n  </response>\n</multistatus>\n";
        let expected = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//hacksw/handcal//NONSGML v1.0//EN\r\nBEGIN:VEVENT\r\nUID:jSayX7OSdp3V\r\nDTSTAMP:19970610T172345Z\r\nDTSTART:19970714T170000Z\r\nSUMMARY:hello\\, testing\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

        let doc = roxmltree::Document::parse(with_returns).unwrap();
        let responses = doc
            .root_element()
            .descendants()
            .find(|node| node.tag_name() == names::RESPONSE)
            .unwrap();
        assert_eq!(
            get_newline_corrected_text(&responses, &names::CALENDAR_DATA).unwrap(),
            expected
        );
    }
}
