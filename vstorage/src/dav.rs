//! Common bits and pieces shared between CalDav and CardDav

use http::Uri;

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
