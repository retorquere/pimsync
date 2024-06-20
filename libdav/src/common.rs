// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Common bits shared between caldav and carddav clients.

use crate::{
    dav::{check_status, FoundCollection, WebDavClient, WebDavError},
    names,
    xmlutils::get_unquoted_href,
    CheckSupportError, PropertyName,
};

use http::{Method, Request};
use hyper::{client::connect::Connect, Body, Uri};
use log::debug;

pub(crate) fn parse_find_multiple_collections(
    body: impl AsRef<[u8]>,
    only: &PropertyName<'_, '_>,
) -> Result<Vec<FoundCollection>, WebDavError> {
    let body = std::str::from_utf8(body.as_ref())?;
    let doc = roxmltree::Document::parse(body)?;
    let root = doc.root_element();

    let responses = root
        .descendants()
        .filter(|node| node.tag_name() == names::RESPONSE);

    let mut items = Vec::new();
    for response in responses {
        if !response
            .descendants()
            .find(|node| node.tag_name() == names::RESOURCETYPE)
            .map_or(false, |node| {
                node.descendants().any(|node| node.tag_name() == *only)
            })
        {
            continue;
        }

        let href = get_unquoted_href(&response)?.to_string();
        let etag = response
            .descendants()
            .find(|node| node.tag_name() == names::GETETAG)
            .and_then(|node| node.text().map(str::to_string));
        let supports_sync = response
            .descendants()
            .find(|node| node.tag_name() == names::SUPPORTED_REPORT_SET)
            .map_or(false, |node| {
                node.descendants()
                    .any(|node| node.tag_name() == names::SYNC_COLLECTION)
            });

        items.push(FoundCollection {
            href,
            etag,
            supports_sync,
        });
    }

    Ok(items)
}

pub(crate) async fn check_support<C>(
    client: &WebDavClient<C>,
    uri: &Uri,
    expectation: &str,
) -> Result<(), CheckSupportError>
where
    C: Connect + Clone + Sync + Send + 'static,
{
    let request = Request::builder()
        .method(Method::OPTIONS)
        .uri(uri)
        .body(Body::empty())?;

    let (head, _body) = client.request(request).await?;
    check_status(head.status)?;

    let header = head
        .headers
        .get("DAV")
        .ok_or(CheckSupportError::MissingHeader)?
        .to_str()?;

    debug!("DAV header: '{}'", header);
    if header
        .split(|c| c == ',')
        .any(|part| part.trim() == expectation)
    {
        Ok(())
    } else {
        Err(CheckSupportError::NotAdvertised)
    }
}
