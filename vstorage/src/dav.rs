//! Common bits and pieces shared between CalDav and CardDav

use http::Uri;

use crate::{Error, ErrorKind, Result};

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
