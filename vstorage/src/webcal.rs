// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Implements reading entries from a remote webcal resource.
//!
//! Webcal is a de-facto standard, and is basically a single icalendar file hosted via http(s).
//!
//! See the [Webcal wikipedia page](https://en.wikipedia.org/wiki/Webcal).
#![allow(clippy::module_name_repetitions)]

use std::sync::Arc;

use async_trait::async_trait;
use http::{uri::Scheme, StatusCode, Uri};
use hyper::{client::HttpConnector, Client};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};

use crate::{
    base::{
        CalendarProperty, Collection, Definition, FetchedItem, IcsItem, Item, ItemRef, Storage,
    },
    disco::Discovery,
    simple_component::Component,
    CollectionId, Error, ErrorKind, Etag, Result,
};

/// A storage which exposes items in remote icalendar resource.
///
/// A webcal storage contains exactly one collection, which contains all the entires found in the
/// remote resource. The name of this single collection is specified via the
/// [`WebCalDefinition::collection_name`] property.
///
/// This storage is a bit of an odd one (since in reality, there's no concept of collections in
/// webcal). The extra abstraction layer is here merely to match the format of other storages.
///
/// # Href
///
/// The `href` for this meaningless. A string matching the [`WebCalDefinition::collection_name`]
/// property is used to describe the only available collection.
pub struct WebCalStorage {
    definition: WebCalDefinition,
    http_client: Client<HttpsConnector<HttpConnector>>,
}

/// Definition for a [`WebCalStorage`].
#[derive(Debug, PartialEq)]
pub struct WebCalDefinition {
    /// The URL of the remote icalendar resource. Must be HTTP or HTTPS.
    pub url: Uri,
    /// The href and id to be given to the single collection available.
    pub collection_name: CollectionId,
}

impl WebCalDefinition {
    /// Build a new `Storage` instance.
    ///
    /// # Errors
    ///
    /// If there are errors discovering the CardDav server.
    pub fn build(self) -> Result<WebCalStorage> {
        let proto = match &self.url.scheme().map(Scheme::as_str) {
            Some("http") => HttpsConnectorBuilder::new()
                .with_native_roots()
                .https_or_http()
                .enable_http1()
                .build(),
            Some("https") => HttpsConnectorBuilder::new()
                .with_native_roots()
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
            None => todo!(),
        };
        Ok(WebCalStorage {
            definition: self,
            http_client: Client::builder().build(proto),
        })
    }
}

#[async_trait]
impl Definition<IcsItem> for WebCalDefinition {
    /// Create a new storage instance.
    ///
    /// Unlike other [`Storage`] implementations, this one allows only a single collection.
    async fn into_storage(self) -> Result<Arc<dyn Storage<IcsItem>>> {
        Ok(Arc::new(self.build()?))
    }
}

#[async_trait]
impl Storage<IcsItem> for WebCalStorage {
    /// Checks that the remove resource exists and whether it looks like an icalendar resource.
    async fn check(&self) -> Result<()> {
        // TODO: Should map status codes to io::Error. if 404 -> NotFound, etc.
        let raw = fetch_raw(&self.http_client, &self.definition.url).await?;

        if !raw.starts_with("BEGIN:VCALENDAR") {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "response for URL doesn't look like a calendar",
            ));
        }
        Ok(())
    }

    /// Returns a single collection with the name specified in the definition.
    async fn discover_collections(&self) -> Result<Discovery> {
        // TODO: shouldn't I check that the collection actually exists?
        Ok(vec![Collection::new(
            self.definition.collection_name.clone().into(),
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
    async fn create_collection_with_id(&self, _id: &CollectionId) -> Result<Collection> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn destroy_collection(&self, _: &str) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webcal is not supported",
        ))
    }

    /// Usable only with the collection name specified in the definition. Any other name will
    /// return [`ErrorKind::DoesNotExist`]
    fn open_collection(&self, href: &str) -> Result<Collection> {
        if href != self.definition.collection_name.as_ref() {
            return Err(Error::new(
                ErrorKind::DoesNotExist,
                format!("this storage only contains the '{href}' collection"),
            ));
        }
        Ok(Collection::new(
            self.definition.collection_name.clone().into(),
        ))
    }

    /// Enumerates items in this collection.
    ///
    /// Note that, due to the nature of webcal, the whole collection needs to be retrieved. If some
    /// items need to be read as well, it is generally best to use
    /// [`WebCalStorage::get_all_items`] instead.
    async fn list_items(&self, _collection: &Collection) -> Result<Vec<ItemRef>> {
        let raw = fetch_raw(&self.http_client, &self.definition.url).await?;

        // TODO: it would be best if the parser could operate on a stream, although that might
        //       complicate inlining VTIMEZONEs that are at the end.
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
        let raw = fetch_raw(&self.http_client, &self.definition.url).await?;

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
        let raw = fetch_raw(&self.http_client, &self.definition.url).await?;

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
    async fn get_all_items(&self, _collection: &Collection) -> Result<Vec<FetchedItem<IcsItem>>> {
        let raw = fetch_raw(&self.http_client, &self.definition.url).await?;

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
    async fn add_item(&self, _collection: &Collection, _: &IcsItem) -> Result<ItemRef> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "creating collections via webcal is not supported",
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
    async fn set_collection_property(
        &self,
        _collection: &Collection,
        _: CalendarProperty,
        _: &str,
    ) -> Result<()> {
        Err(Error::new(
            ErrorKind::Unsupported,
            "setting metadata via webcal is not supported",
        ))
    }

    /// Unsupported for this storage type.
    async fn get_collection_property(
        &self,
        _collection: &Collection,
        _: CalendarProperty,
    ) -> Result<Option<String>> {
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

    fn collection_id(&self, collection: &Collection) -> Result<CollectionId> {
        if collection.href() == self.definition.collection_name.as_ref() {
            Ok(self.definition.collection_name.clone())
        } else {
            Err(ErrorKind::DoesNotExist.into())
        }
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

    use crate::base::Definition;

    // FIXME: only run this test with a dedicated flag for networked test.
    // FIXME: use a webcal link hosted by me.
    // TODO: these are just validation tests and not suitable as a keeper.
    #[tokio::test]
    #[ignore = "uses internet resource"]
    async fn test_dummy() {
        use crate::webcal::WebCalDefinition;

        let definition = WebCalDefinition {
            url: Uri::try_from("https://www.officeholidays.com/ics/netherlands").unwrap(),
            collection_name: "holidays".parse().unwrap(),
        };
        let storage = definition.into_storage().await.unwrap();
        storage.check().await.unwrap();
        let collection = &storage.open_collection("holidays").unwrap();
        let discovery = &storage.discover_collections().await.unwrap();

        assert_eq!(
            &collection.href(),
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
