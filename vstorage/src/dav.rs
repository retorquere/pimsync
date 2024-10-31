//! Common bits and pieces shared between CalDav and CardDav

use http::{StatusCode, Uri};
use libdav::dav::ListedResource;
use log::warn;

use crate::{base::ItemRef, CollectionId, CollectionIdError, Error, ErrorKind, Result};

pub(crate) fn path_for_collection_in_home_set(home_set: &Uri, name: &str) -> String {
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
    path.push_str(name);
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
    let mut items = Vec::with_capacity(response.len());
    // TODO: should actually check that href's path matches the requested path.
    if response.len() == 1 && response[0].status == Some(StatusCode::NOT_FOUND) {
        return Err(ErrorKind::DoesNotExist.into());
    }
    for r in response {
        match r.status {
            Some(StatusCode::OK) | None => {}
            Some(status) => {
                warn!("Item in response is not OK: {status}.");
                continue;
            }
        };
        items.push(ItemRef {
            href: r.href,
            etag: r
                .details
                .etag
                .ok_or(ErrorKind::InvalidData.error("missing Etag"))?
                .into(),
        });
    }
    Ok(items)
}
