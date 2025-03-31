// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Common bits and pieces shared between CalDAV and CardDAV

use http::{StatusCode, Uri};
use libdav::{
    dav::{ListedResource, WebDavError},
    sd::BootstrapError,
};
use log::warn;

use crate::{
    base::{CreateItemOptions, Item, ItemVersion},
    CollectionId, CollectionIdError, Error, ErrorKind, Result,
};

/// Generate a path for a collection expected to have id `id`.
pub(crate) fn path_for_collection_in_home_set(home_set: &Uri, id: &str) -> String {
    // TODO: can be simplified with: https://github.com/hyperium/http/pull/623
    let mut path = match home_set.clone().into_parts().path_and_query {
        Some(ref pq) => pq.path(),
        None => "/",
    }
    .to_owned();

    if let Some(index) = path.find('?') {
        path.truncate(index + 1);
    }

    if !path.ends_with('/') {
        path.push('/');
    }
    path.push_str(id);
    path.push('/');
    path
}

pub(crate) fn collection_href_for_item(item_href: &str) -> Result<&str> {
    let mut parts = item_href.rsplitn(2, '/');
    let _resource = parts.next();
    let collection_href = parts
        .next()
        .ok_or_else(|| Error::from(ErrorKind::InvalidInput))?;
    Ok(collection_href)
}

/// Returns the last path component.
#[inline]
pub(crate) fn collection_id_for_href(href: &str) -> Result<CollectionId, CollectionIdError> {
    href.trim_matches('/') // Remove any trailing slashes.
        .rsplit('/')
        .next()
        .expect("rsplit always returns at least one item")
        .parse()
}

pub(crate) fn parse_list_items(response: Vec<ListedResource>) -> Result<Vec<ItemVersion>> {
    // TODO: should actually check that href's path matches the requested path.
    if response.len() == 1 && response[0].status == Some(StatusCode::NOT_FOUND) {
        return Err(ErrorKind::DoesNotExist.into());
    }

    response
        .into_iter()
        .filter_map(|r| match (r.status, r.etag) {
            (Some(StatusCode::OK) | None, Some(etag)) => Some(Ok(ItemVersion {
                href: r.href,
                etag: etag.into(),
            })),
            (Some(status), _) => {
                warn!("Got status code {status} for item: {}.", r.href);
                None
            }
            (_, None) => Some(Err(ErrorKind::InvalidData.error("missing Etag"))),
        })
        .collect()
}

impl From<BootstrapError> for Error {
    fn from(value: BootstrapError) -> Self {
        // TODO: not implemented
        Error::new(ErrorKind::Uncategorised, value)
    }
}

impl From<libdav::dav::WebDavError> for Error {
    fn from(value: libdav::dav::WebDavError) -> Self {
        match value {
            WebDavError::BadStatusCode(StatusCode::NOT_FOUND) => {
                Error::new(ErrorKind::DoesNotExist, value)
            }
            WebDavError::BadStatusCode(StatusCode::FORBIDDEN) => {
                Error::new(ErrorKind::AccessDenied, value)
            }
            err => Error::new(ErrorKind::Uncategorised, err), // TODO
        }
    }
}

pub(crate) fn join_hrefs(collection_href: &str, item_href: &str) -> String {
    if item_href.starts_with('/') {
        return item_href.to_string();
    }

    let mut href = collection_href
        .strip_suffix('/')
        .unwrap_or(collection_href)
        .to_string();
    href.push('/');
    href.push_str(item_href);
    href
}

/// Returns false if a resource name has any reserved characters.
///
/// ```txt
/// reserved    = gen-delims / sub-delims
/// gen-delims  = ":" / "/" / "?" / "#" / "[" / "]" / "@"
/// sub-delims  = "!" / "$" / "&" / "'" / "(" / ")"
/// ```
fn is_valid_resource_name(name: &str) -> bool {
    static INVALID_CHARS: &str = ":/?#[]@!$&'()";
    if name
        .chars()
        .any(|c| INVALID_CHARS.contains(c) || c.is_control())
    {
        return false;
    }
    if name == ".." || name.is_empty() {
        return false;
    }
    true
}

pub(crate) fn name_for_creation(collection: &str, item: &Item, opts: CreateItemOptions) -> String {
    if let Some(name) = opts.href {
        if is_valid_resource_name(&name) {
            return join_hrefs(collection, &name);
        }
    }
    if let Some(name) = item.uid() {
        if is_valid_resource_name(&name) {
            return join_hrefs(collection, &name);
        }
    }
    join_hrefs(collection, &item.hash().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_names() {
        assert!(is_valid_resource_name("example"));
        assert!(is_valid_resource_name("example.ics"));
        assert!(is_valid_resource_name("resource-1"));
        assert!(is_valid_resource_name("resource_1"));
        assert!(is_valid_resource_name("contact.vcf"));
        assert!(is_valid_resource_name("doc.pdf"));
        assert!(is_valid_resource_name(".hidden"));
        assert!(is_valid_resource_name("-party.ics"));
        assert!(is_valid_resource_name("my event.ics"));
        assert!(is_valid_resource_name("file\\name")); // TODO: review
        assert!(is_valid_resource_name("file*")); // TODO: review
    }

    #[test]
    fn test_invalid_names() {
        assert!(!is_valid_resource_name(""));
        assert!(!is_valid_resource_name("folder/file.txt"));
        assert!(!is_valid_resource_name("path/../file"));
        assert!(!is_valid_resource_name("path//file"));
        assert!(!is_valid_resource_name("file\tname"));
        assert!(!is_valid_resource_name("file\nname"));
        assert!(!is_valid_resource_name("file?query"));
        assert!(!is_valid_resource_name("file#fragment"));
    }
}
