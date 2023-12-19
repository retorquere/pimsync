// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Common bits shared between caldav and carddav clients.

use crate::{
    dav::{DavError, FoundCollection, WebDavClient},
    dns::DiscoverableService,
    names,
    xmlutils::get_unquoted_href,
    FindHomeSetError, InvalidUrl, Property,
};

use hyper::{client::connect::Connect, Uri};

pub(crate) fn parse_find_multiple_collections(
    body: impl AsRef<[u8]>,
    only: &Property<'_, '_>,
) -> Result<Vec<FoundCollection>, DavError> {
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

/// Queries a server for a calendar or address book home set.
///
/// See: <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
///
/// # Errors
///
/// If there are any network errors or the response could not be parsed.
pub(crate) async fn find_home_set<C>(
    client: &WebDavClient<C>,
    property: &Property<'_, '_>,
) -> Result<Option<Uri>, FindHomeSetError>
where
    C: Connect + Clone + Sync + Send,
{
    // If obtaining a principal fails, the specification says we should query the user. This
    // tries to use the `base_url` first, since the user might have provided it for a reason.
    let principal_url = client.principal.as_ref().unwrap_or(&client.base_url);
    client
        .find_href_prop_as_uri(principal_url, property)
        .await
        .map_err(FindHomeSetError)
}

/// Helper trait for implementing [rfc6764](https://www.rfc-editor.org/rfc/rfc6764) discovery.
pub trait Rfc6764Protocol {
    /// Returns the service type based on the provided Uri.
    fn service(uri: &Uri) -> Result<DiscoverableService, InvalidUrl>;
    /// Name of the property that describes this protocol's home set.
    fn home_set_property() -> &'static Property<'static, 'static>;
}
