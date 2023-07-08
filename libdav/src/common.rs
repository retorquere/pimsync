// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Common bits shared between caldav and carddav clients.

use crate::{
    dav::{DavError, FoundCollection, WebDavClient},
    dns::{find_context_path_via_txt_records, resolve_srv_record, DiscoverableService},
    names,
    xmlutils::get_unquoted_href,
    BootstrapError, Property,
};
use domain::base::Dname;

use hyper::Uri;

/// A big chunk of the bootstrap logic that's shared between both types.
///
/// Mutates the `base_url` for the client to the discovered one.
pub(crate) async fn common_bootstrap(
    client: &mut WebDavClient,
    port: u16,
    service: DiscoverableService,
) -> Result<(), BootstrapError> {
    let domain = client
        .base_url
        .host()
        .ok_or(BootstrapError::InvalidUrl("a host is required"))?;

    let dname = Dname::bytes_from_str(domain)
        .map_err(|_| BootstrapError::InvalidUrl("invalid domain name"))?;
    let host_candidates = {
        let candidates = resolve_srv_record(service, &dname, port)
            .await
            .map_err(BootstrapError::DnsError)?;

        // If there are no SRV records, try the domain/port in the provided URI.
        if candidates.is_empty() {
            vec![(domain.to_string(), port)]
        } else {
            candidates
        }
    };

    if let Some(path) = find_context_path_via_txt_records(service, &dname).await? {
        let candidate = &host_candidates[0];

        // TODO: check `DAV:` capabilities here.
        client.base_url = Uri::builder()
            .scheme(service.scheme())
            .authority(format!("{}:{}", candidate.0, candidate.1))
            .path_and_query(path)
            .build()
            .map_err(BootstrapError::UnusableSrv)?;
    } else {
        for candidate in host_candidates {
            if let Ok(Some(url)) = client
                .find_context_path(service, &candidate.0, candidate.1)
                .await
            {
                client.base_url = url;
                break;
            }
        }
    }

    client.principal = client.find_current_user_principal().await?;

    Ok(())
}

pub(crate) fn parse_find_multiple_collections<B: AsRef<[u8]>>(
    body: B,
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
