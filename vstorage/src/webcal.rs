// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Implements reading entries from a remote webcal resource.
//!
//! Webcal is a de-facto standard, and is basically a single icalendar file hosted via http(s).
//!
//! See the [Webcal wikipedia page](https://en.wikipedia.org/wiki/Webcal).

use async_trait::async_trait;
use http::{uri::Scheme, StatusCode, Uri};
use hyper::{client::HttpConnector, Client};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};

use crate::{
    base::{
        CalendarProperty, Collection, FetchedItem, IcsItem, Item, ItemRef, ListedProperty, Storage,
    },
    disco::{DiscoveredCollection, Discovery},
    simple_component::Component,
    CollectionId, Error, ErrorKind, Etag, Href, Result,
};

/// A storage which exposes items in remote icalendar resource.
///
/// A webcal storage contains exactly one collection, which contains all the entires found in the
/// remote resource. The name of this single collection is specified via the `collection_name`
/// argument.
///
/// This storage is a bit of an odd one (since in reality, there's no concept of collections in
/// webcal). The extra abstraction layer is here merely to match the format of other storages.
///
/// # Href
///
/// The `href` for this meaningless. A string matching the `collection_name` property is used to
/// describe the only available collection.
// TODO: If an alternative href is provided, it should be used as a path on the same host.
//       Note that discovery will only support the one matching the input URL.
pub struct WebCalStorage {
    /// The URL of the remote icalendar resource. Must be HTTP or HTTPS.
    url: Uri,
    /// The href and id to be given to the single collection available.
    collection_name: CollectionId,
    http_client: Client<HttpsConnector<HttpConnector>>,
}

impl WebCalStorage {
    /// Build a new `Storage` instance.
    ///
    /// # Errors
    ///
    /// If there are errors discovering the CardDav server.
    pub fn new(url: Uri, collection_name: CollectionId) -> Result<WebCalStorage> {
        let proto = match &url.scheme().map(Scheme::as_str) {
            Some("http") => HttpsConnectorBuilder::new()
                .with_native_roots()?
                .https_or_http()
                .enable_http1()
                .build(),
            Some("https") => HttpsConnectorBuilder::new()
                .with_native_roots()?
                .https_only()
                .enable_http1()
                .build(),
            // TODO: support webcal and webcals
            Some(_) => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "URL scheme must be http or https",
                ));
            }
            None => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "webcal URL requires a scheme/protocol",
                ));
            }
        };
        Ok(WebCalStorage {
            url,
            collection_name,
            http_client: Client::builder().build(proto),
        })
    }
}

#[async_trait]
impl Storage<IcsItem> for WebCalStorage {
    /// Checks that the remove resource exists and whether it looks like an icalendar resource.
    async fn check(&self) -> Result<()> {
        // TODO: Should map status codes to io::Error. if 404 -> NotFound, etc.
        let raw = fetch_raw(&self.http_client, &self.url).await?;

        if !raw.starts_with("BEGIN:VCALENDAR") {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "response for URL doesn't look like a calendar",
            ));
        }
        Ok(())
    }

    /// Returns a single collection with the name originally specified.
    async fn discover_collections(&self) -> Result<Discovery> {
        // TODO: shouldn't I check that the collection actually exists?
        Ok(vec![DiscoveredCollection::new(
            self.url.path().to_string(),
            self.collection_name.clone(),
        )]
        .into())
    }

    /// Unsupported for this storage type.
    async fn create_collection(&self, _: &str) -> Result<Collection> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn destroy_collection(&self, _: &str) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "destroying collections via webcal is not supported",
        ))
    }

    /// Enumerates items in this collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. If some
    /// items need to be read as well, it is generally best to use
    /// [`WebCalStorage::get_all_items`] instead.
    async fn list_items(&self, _collection: &str) -> Result<Vec<ItemRef>> {
        let raw = fetch_raw(&self.http_client, &self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate copying VTIMEZONEs inline if they are at the end of the stream.
        let refs = Component::parse(&raw)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .into_split_collection()
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .iter()
            .map(|c| {
                let item = IcsItem::from(c.to_string());
                let hash = item.hash();

                ItemRef {
                    href: item.ident(),
                    etag: hash.into(),
                }
            })
            .collect();

        Ok(refs)
    }

    /// Returns a single item from the collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. It is
    /// strongly recommended to use [`WebCalStorage::get_all_items`] instead.
    async fn get_item(&self, href: &str) -> Result<(IcsItem, Etag)> {
        let raw = fetch_raw(&self.http_client, &self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let item = Component::parse(&raw)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .into_split_collection()
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .iter()
            .find_map(|c| {
                let item = IcsItem::from(c.to_string());
                if item.ident() == href {
                    Some(item)
                } else {
                    None
                }
            })
            .ok_or_else(|| Error::from(ErrorKind::DoesNotExist))?;

        let hash = item.hash();
        Ok((item, hash.into()))
    }

    /// Returns multiple items from the collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. It is
    /// generally best to use [`WebCalStorage::get_all_items`] instead.
    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem<IcsItem>>> {
        let raw = fetch_raw(&self.http_client, &self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.

        Component::parse(&raw)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .into_split_collection()
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .iter()
            .filter_map(|c| {
                let item = IcsItem::from(c.to_string());
                if hrefs.contains(&(item.ident().as_ref())) {
                    Some(Ok(FetchedItem {
                        href: item.ident(),
                        etag: item.hash().into(),
                        item,
                    }))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Fetch all items in the collection.
    ///
    /// Performs a single HTTP(s) request to fetch all items.
    async fn get_all_items(&self, _collection: &str) -> Result<Vec<FetchedItem<IcsItem>>> {
        let raw = fetch_raw(&self.http_client, &self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let components = Component::parse(&raw)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .into_split_collection()
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;

        components
            .iter()
            .map(|c| {
                let item = IcsItem::from(c.to_string());
                Ok(FetchedItem {
                    href: item.ident(),
                    etag: item.hash().into(),
                    item,
                })
            })
            .collect()
    }

    /// Unsupported for this storage type.
    async fn add_item(&self, _collection: &str, _: &IcsItem) -> Result<ItemRef> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "adding items via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn update_item(&self, _: &str, _: &Etag, _: &IcsItem) -> Result<Etag> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "updating items via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn set_property(&self, _: &str, _: CalendarProperty, _: &str) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "setting metadata via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn unset_property(&self, _: &str, _: CalendarProperty) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "unsetting metadata via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn get_property(&self, _: &str, _: CalendarProperty) -> Result<Option<String>> {
        // TODO: return None?
        Err(Error::new(
            ErrorKind::Unsupported,
            "getting metadata via webcal is not supported",
        ))
    }

    async fn delete_item(&self, _: &str, _: &Etag) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "deleting items via webcal is not supported",
        ))
    }

    fn collection_id(&self, collection_href: &str) -> Result<CollectionId> {
        if collection_href == self.url.path() {
            Ok(self.collection_name.clone())
        } else {
            Err(ErrorKind::DoesNotExist.into())
        }
    }

    fn href_for_collection_id(&self, id: &CollectionId) -> Result<Href> {
        if id == &self.collection_name {
            Ok(self.url.path().to_string())
        } else {
            Err(Error::new(
                ErrorKind::Unsupported,
                "discovery of arbitrary collections is not supported",
            ))
        }
    }

    async fn list_properties(&self, _: &str) -> Result<Vec<ListedProperty<CalendarProperty>>> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "webcal does not support properties",
        ))
    }
}

/// Helper method to fetch a URL and return its body as a String.
///
/// Be warned! This swallows headers (including `Etag`!).
#[inline]
async fn fetch_raw(client: &Client<HttpsConnector<HttpConnector>>, url: &Uri) -> Result<String> {
    let response = client
        // TODO: upstream should impl IntoURL for &Uri
        .get(url.clone())
        .await
        .map_err(|e| Error::new(ErrorKind::Io, e))?;

    match response.status() {
        StatusCode::NOT_FOUND | StatusCode::GONE => {
            return Err(Error::new(
                ErrorKind::DoesNotExist,
                "The remote resource does not exist.",
            ))
        }
        StatusCode::OK => {}
        code => {
            return Err(Error::new(
                ErrorKind::Io,
                format!("request returned {code}"),
            ))
        }
    }

    // TODO: handle non-UTF-8 data (e.g.: Content-Type/charset).
    hyper::body::to_bytes(response)
        .await
        .map_err(|e| Error::new(ErrorKind::Io, e))
        .map(|bytes| String::from_utf8(bytes.into()))?
        .map_err(|e| Error::new(ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod test {
    use http::Uri;

    use crate::{base::Storage, webcal::WebCalStorage};

    // FIXME: only run this test with a dedicated flag for networked test.
    // FIXME: use a webcal link hosted by me.
    // TODO: these are just validation tests and not suitable as a keeper.
    #[tokio::test]
    #[ignore = "uses internet resource"]
    async fn test_dummy() {
        let storage = WebCalStorage::new(
            Uri::try_from("https://www.officeholidays.com/ics/netherlands").unwrap(),
            "holidays".parse().unwrap(),
        )
        .unwrap();
        storage.check().await.unwrap();
        let collection = "holidays";
        let discovery = &storage.discover_collections().await.unwrap();

        assert_eq!(
            &collection,
            &discovery.collections().first().unwrap().href()
        );

        let item_refs = storage.list_items(collection).await.unwrap();

        for item_ref in &item_refs {
            let (_item, etag) = storage.get_item(&item_ref.href).await.unwrap();
            // Might file if upstream file mutates between requests.
            assert_eq!(etag, item_ref.etag);
        }

        let hrefs: Vec<&str> = item_refs.iter().map(|r| r.href.as_ref()).collect();
        let many = storage.get_many_items(&hrefs.clone()).await.unwrap();

        assert_eq!(many.len(), hrefs.len());
        assert_eq!(many.len(), item_refs.len());
        // TODO: compare their contents and etags, though these should all match.
    }
}
