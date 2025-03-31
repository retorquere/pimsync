// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Implements reading entries from a remote webcal resource.
//!
//! Webcal is a de-facto standard, and is basically a single icalendar file hosted via http(s).
//!
//! See the [Webcal wikipedia page](https://en.wikipedia.org/wiki/Webcal).

use async_trait::async_trait;
use http::{Method, Request, Response, StatusCode, Uri};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use tower::Service;

use crate::{
    base::{
        Collection, CreateItemOptions, FetchedItem, FetchedProperty, Item, ItemVersion, Property,
        Storage,
    },
    disco::{DiscoveredCollection, Discovery},
    simple_component::Component,
    CollectionId, Error, ErrorKind, Etag, Href, Result,
};

/// A storage which exposes items in remote icalendar resource.
///
/// A webcal storage contains exactly one collection, which contains all the entires found in the
/// remote resource. The name of this single collection is specified via the `collection_id`
/// argument.
///
/// This storage is a bit of an odd one (since in reality, there's no concept of collections in
/// webcal). The extra abstraction layer is here merely to match the format of other storages.
///
/// # Href
///
/// The `href` for this meaningless. A string matching the `collection_id` property is used to
/// describe the only available collection.
// TODO: If an alternative href is provided, it should be used as a path on the same host.
//       Note that discovery will only support the one matching the input URL.
pub struct WebCalStorage<C>
where
    // XXX: Clone can be dropped after tower drops it `mut` for `tower_service::Service::call`
    //      See: https://github.com/hyperium/hyper/issues/3784#issuecomment-2491667302
    //      See: https://github.com/tower-rs/tower/issues/753
    C: Service<Request<String>, Response = Response<Incoming>> + Send + Sync + Clone + 'static,
    C::Error: std::error::Error + Send + Sync,
    C::Future: Send + Sync,
{
    /// The URL of the remote icalendar resource. Must be HTTP or HTTPS.
    url: Uri,
    /// The href and id to be given to the single collection available.
    collection_id: CollectionId,
    http_client: C,
}

impl<C> WebCalStorage<C>
where
    C: Service<Request<String>, Response = Response<Incoming>> + Send + Sync + Clone + 'static,
    C::Error: std::error::Error + Send + Sync,
    C::Future: Send + Sync,
{
    /// Build a new `Storage` instance.
    ///
    /// # Errors
    ///
    /// If there are errors discovering the CardDAV server.
    // TODO: take custom client as input.
    pub fn new(http_client: C, url: Uri, collection_id: CollectionId) -> Result<WebCalStorage<C>> {
        Ok(WebCalStorage {
            url,
            collection_id,
            http_client,
        })
    }

    /// Helper method to fetch a URL and return its body as a String.
    ///
    /// Be warned! This swallows headers (including `Etag`!).
    async fn fetch_raw(&self, url: &Uri) -> Result<String> {
        let req = Request::builder()
            .method(Method::GET)
            .uri(url)
            .body(String::new())
            .map_err(|e| Error::new(ErrorKind::InvalidInput, e))?;
        let response = self
            .http_client
            .clone()
            .call(req)
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

        let (_head, body) = response.into_parts();
        let data = body
            .collect()
            .await
            .map_err(|e| Error::new(ErrorKind::Io, e))?
            .to_bytes();

        // TODO: handle non-UTF-8 data (e.g.: Content-Type/charset).
        // TODO: can I avoid making a copy of the entire response here?
        String::from_utf8(data.to_vec()).map_err(|e| Error::new(ErrorKind::InvalidData, e))
    }
}

#[async_trait]
impl<C> Storage for WebCalStorage<C>
where
    C: Service<Request<String>, Response = Response<Incoming>> + Send + Sync + Clone + 'static,
    C::Error: std::error::Error + Send + Sync,
    C::Future: Send + Sync,
{
    /// Checks that the remove resource exists and whether it looks like an icalendar resource.
    async fn check(&self) -> Result<()> {
        // TODO: Should map status codes to io::Error. if 404 -> NotFound, etc.
        let raw = self.fetch_raw(&self.url).await?;

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
        vec![DiscoveredCollection::new(
            self.url.path().to_string(),
            self.collection_id.clone(),
        )]
        .try_into()
        .map_err(|e| ErrorKind::InvalidData.error(e))
    }

    /// Unsupported for this storage type.
    async fn create_collection(&self, _: &str) -> Result<Collection> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn delete_collection(&self, _: &str) -> Result<()> {
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
    async fn list_items(&self, _collection: &str) -> Result<Vec<ItemVersion>> {
        let raw = self.fetch_raw(&self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate copying VTIMEZONEs inline if they are at the end of the stream.
        let refs = Component::parse(&raw)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .into_split_collection()
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .iter()
            .map(|c| {
                let item = Item::from(c.to_string());
                let hash = item.hash();

                ItemVersion::new(item.ident(), Etag::from(hash.to_string()))
            })
            .collect();

        Ok(refs)
    }

    /// Returns a single item from the collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. It is
    /// strongly recommended to use [`WebCalStorage::get_all_items`] instead.
    async fn get_item(&self, href: &str) -> Result<(Item, Etag)> {
        let raw = self.fetch_raw(&self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let item = Component::parse(&raw)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .into_split_collection()
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .iter()
            .find_map(|c| {
                let item = Item::from(c.to_string());
                if item.ident() == href {
                    Some(item)
                } else {
                    None
                }
            })
            .ok_or_else(|| Error::from(ErrorKind::DoesNotExist))?;

        let hash = item.hash();
        Ok((item, hash.to_string().into()))
    }

    /// Returns multiple items from the collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. It is
    /// generally best to use [`WebCalStorage::get_all_items`] instead.
    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem>> {
        let raw = self.fetch_raw(&self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.

        Component::parse(&raw)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .into_split_collection()
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .iter()
            .filter_map(|c| {
                let item = Item::from(c.to_string());
                if hrefs.contains(&(item.ident().as_ref())) {
                    Some(Ok(FetchedItem {
                        href: item.ident(),
                        etag: item.hash().to_string().into(),
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
    async fn get_all_items(&self, _collection: &str) -> Result<Vec<FetchedItem>> {
        let raw = self.fetch_raw(&self.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
        let components = Component::parse(&raw)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
            .into_split_collection()
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;

        components
            .iter()
            .map(|c| {
                let item = Item::from(c.to_string());
                Ok(FetchedItem {
                    href: item.ident(),
                    etag: item.hash().to_string().into(),
                    item,
                })
            })
            .collect()
    }

    /// Unsupported for this storage type.
    async fn create_item(
        &self,
        _collection: &str,
        _: &Item,
        _: CreateItemOptions,
    ) -> Result<ItemVersion> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "adding items via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn update_item(&self, _: &str, _: &Etag, _: &Item) -> Result<Etag> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "updating items via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn set_property(&self, _: &str, _: Property, _: &str) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "setting metadata via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn unset_property(&self, _: &str, _: Property) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "unsetting metadata via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn get_property(&self, _: &str, _: Property) -> Result<Option<String>> {
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

    fn href_for_collection_id(&self, id: &CollectionId) -> Result<Href> {
        if id == &self.collection_id {
            Ok(self.url.path().to_string())
        } else {
            Err(Error::new(
                ErrorKind::Unsupported,
                "discovery of arbitrary collections is not supported",
            ))
        }
    }

    async fn list_properties(&self, _: &str) -> Result<Vec<FetchedProperty>> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "webcal does not support properties",
        ))
    }
}

#[cfg(test)]
mod test {
    use http::Uri;
    use hyper_rustls::HttpsConnectorBuilder;
    use hyper_util::{client::legacy::Client, rt::TokioExecutor};

    use crate::{base::Storage, webcal::WebCalStorage};

    // FIXME: only run this test with a dedicated flag for networked test.
    // FIXME: use a webcal link hosted by me.
    // TODO: these are just validation tests and not suitable as a keeper.
    #[tokio::test]
    #[ignore = "uses internet resource"]
    async fn test_dummy() {
        let connector = HttpsConnectorBuilder::new()
            .with_native_roots()
            .unwrap()
            .https_or_http()
            .enable_http1()
            .build();
        let client = Client::builder(TokioExecutor::new()).build(connector);
        let storage = WebCalStorage::new(
            client,
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

        let item_vers = storage.list_items(collection).await.unwrap();

        for item_ver in &item_vers {
            let (_item, etag) = storage.get_item(&item_ver.href).await.unwrap();
            // Might file if upstream file mutates between requests.
            assert_eq!(etag, item_ver.etag);
        }

        let hrefs: Vec<&str> = item_vers.iter().map(|r| r.href.as_ref()).collect();
        let many = storage.get_many_items(&hrefs.clone()).await.unwrap();

        assert_eq!(many.len(), hrefs.len());
        assert_eq!(many.len(), item_vers.len());
        // TODO: compare their contents and etags, though these should all match.
    }
}
