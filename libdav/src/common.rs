// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Common bits shared between caldav and carddav clients.

use crate::{
    dav::{check_status, DavError, FoundCollection, WebDavClient},
    dns::{find_context_path_via_txt_records, resolve_srv_record, DiscoverableService},
    names,
    xmlutils::get_unquoted_href,
    BootstrapError, CheckSupportError, InvalidUrl, Property,
};

use domain::base::Dname;
use http::{Method, Request};
use hyper::{client::connect::Connect, Body, Uri};
use log::debug;

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

/// Find a CalDav or CardDav context path via client bootstrap sequence.
///
/// Determines the server's real host and the context path of the resources for a server,
/// following the discovery mechanism described in [rfc6764].
///
/// [rfc6764]: https://www.rfc-editor.org/rfc/rfc6764
///
/// This resolves from "user friendly" URLs to the real URL where the CalDav or CardDav server is
/// advertised as running. For example, a user may understand their CalDav server as being
/// `https://example.com` but bootstrapping would reveal it to actually run under
/// `https://instance31.example.com/users/john@example.com/calendars`.
///
/// # Errors
///
/// If any of the underlying DNS or HTTP requests fail, or if any of the responses fail to
/// parse.
///
/// Does not return an error if DNS records are missing, only if they contain invalid data.
pub async fn find_context_path_via_bootstrap<C>(
    client: &WebDavClient<C>,
    service: DiscoverableService,
) -> Result<Option<Uri>, BootstrapError>
where
    C: Connect + Clone + Sync + Send,
{
    let domain = client.base_url.host().ok_or(InvalidUrl::MissingHost)?;
    let port = client.base_url.port_u16().unwrap_or(service.default_port());

    let dname = Dname::bytes_from_str(domain).map_err(InvalidUrl::InvalidDomain)?;
    let host_candidates = resolve_srv_record(service, &dname, port)
        .await?
        .ok_or(BootstrapError::NotAvailable)?;

    let mut context_path = None;
    if let Some(path) = find_context_path_via_txt_records(service, &dname).await? {
        for candidate in &host_candidates {
            let test_uri = Uri::builder()
                .scheme(service.scheme())
                .authority(format!("{}:{}", candidate.0, candidate.1))
                .path_and_query(&path)
                .build()
                .map_err(BootstrapError::UnusableSrv)?;

            match check_support(client, &test_uri, service.access_field()).await {
                Ok(()) | Err(CheckSupportError::NotAdvertised) => {
                    // NotAdvertised implies that the server does not advertise support for this
                    // protocol. We ignore this because NextCloud reports a lack of support for
                    // CalDav and CardDav. See https://github.com/nextcloud/server/issues/37374
                    context_path = Some(test_uri);
                    break;
                }
                Err(_) => continue,
            };
        }
    }
    if context_path.is_none() {
        for candidate in host_candidates {
            if let Ok(Some(url)) = client
                .find_context_path(service, &candidate.0, candidate.1)
                .await
            {
                context_path = Some(url);
                break;
            }
        }
    };

    Ok(context_path)
}
