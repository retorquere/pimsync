//! Common bits and pieces shared between CalDAV and CardDAV

use http::{StatusCode, Uri};
use libdav::{
    dav::{ListedResource, WebDavError},
    sd::BootstrapError,
};
use log::warn;

use crate::{base::ItemRef, CollectionId, CollectionIdError, Error, ErrorKind, Result};

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

#[inline]
pub(crate) fn collection_id_for_href(href: &str) -> Result<CollectionId, CollectionIdError> {
    href.trim_matches('/') // Remove any trailing slashes.
        .rsplit('/')
        .next()
        .expect("rsplit always returns at least one item")
        .parse()
}

pub(crate) fn parse_list_items(response: Vec<ListedResource>) -> Result<Vec<ItemRef>> {
    // TODO: should actually check that href's path matches the requested path.
    if response.len() == 1 && response[0].status == Some(StatusCode::NOT_FOUND) {
        return Err(ErrorKind::DoesNotExist.into());
    }

    response
        .into_iter()
        .filter_map(|r| match (r.status, r.details.etag) {
            (Some(StatusCode::OK) | None, Some(etag)) => Some(Ok(ItemRef {
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
