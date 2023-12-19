// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Common bits shared between caldav and carddav clients.

use crate::{
    auth::Auth,
    dav::{DavError, FoundCollection, WebDavClient},
    dns::{find_context_path_via_txt_records, resolve_srv_record, DiscoverableService},
    names,
    xmlutils::get_unquoted_href,
    BootstrapError, FindHomeSetError, Property,
};
use domain::base::Dname;

use hyper::{client::connect::Connect, Uri};

/// Crate a client bootstrapped with discovery data.
pub(crate) async fn bootstrap_client<C>(
    base_uri: Uri,
    auth: Auth,
    connector: C,
    service: DiscoverableService,
) -> Result<WebDavClient<C>, BootstrapError>
where
    C: Connect + Clone + Send + Sync,
{
    let domain = base_uri
        .host()
        .ok_or(BootstrapError::InvalidUrl("a host is required"))?;
    let port = base_uri.port_u16().unwrap_or(service.default_port());

    let dname = Dname::bytes_from_str(domain)
        .map_err(|_| BootstrapError::InvalidUrl("invalid domain name"))?;
    let host_candidates = resolve_srv_record(service, &dname, port)
        .await?
        .ok_or(BootstrapError::NotAvailable)?;

    let mut client = WebDavClient::new(base_uri, auth, connector);

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

    Ok(client)
}

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

pub trait Rfc6764Protocol {
    /// Returns the service type based on the provided Uri.
    fn service(uri: &Uri) -> Result<DiscoverableService, BootstrapError>;
    /// Name of the property that describes this protocol's home set.
    fn home_set_property() -> &'static Property<'static, 'static>;
}
