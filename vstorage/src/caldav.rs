// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! A [`CalDavStorage`] is a single caldav repository, as specified in rfc4791.

use std::sync::Arc;

use async_trait::async_trait;
use http::Uri;
use hyper::client::connect::Connect;
use libdav::auth::Auth;
use libdav::dav::mime_types;
use libdav::CalDavClient;

use crate::base::{
    CalendarProperty, Collection, Definition, FetchedItem, IcsItem, Item, ItemRef, Storage,
};
use crate::dav::{
    collection_href_for_item, collection_id_for_href, path_for_collection_in_home_set,
};
use crate::disco::{DiscoveredCollection, Discovery};
use crate::{CollectionId, Error, ErrorKind, Etag, Result};

#[derive(Debug)]
pub struct CalDavDefinition<C>
where
    C: Connect + Send + Sync + Clone + 'static,
{
    pub url: Uri,
    pub auth: Auth,
    pub connector: C,
}

impl<C> CalDavDefinition<C>
where
    C: Connect + Send + Sync + Clone + std::fmt::Debug,
{
    /// Build a new `Storage` instance.
    ///
    /// # Errors
    ///
    /// If there are errors discovering the CalDav server.
    pub async fn build(self) -> Result<CalDavStorage<C>> {
        let client = CalDavClient::builder()
            .with_uri(self.url)
            .with_auth(self.auth)
            .bootstrap(self.connector)
            .await?
            .build();

        Ok(CalDavStorage { client })
    }
}

impl From<libdav::BootstrapError> for Error {
    fn from(value: libdav::BootstrapError) -> Self {
        // TODO: not implemented
        Error::new(ErrorKind::Uncategorised, value)
    }
}

impl From<libdav::dav::DavError> for Error {
    fn from(value: libdav::dav::DavError) -> Self {
        // TODO: not implemented
        Error::new(ErrorKind::Uncategorised, value)
    }
}

#[async_trait]
impl<C> Definition<IcsItem> for CalDavDefinition<C>
where
    C: Connect + Send + Sync + Clone + std::fmt::Debug,
{
    async fn into_storage(self) -> Result<Arc<dyn Storage<IcsItem>>> {
        Ok(Arc::from(self.build().await?))
    }
}

/// A storage backed by a caldav server.
///
/// A single storage represents a single server with a specific set of credentials.
pub struct CalDavStorage<C>
where
    C: Connect + Sync + Send + Clone + 'static,
{
    client: CalDavClient<C>,
}

#[async_trait]
impl<C> Storage<IcsItem> for CalDavStorage<C>
where
    C: Connect + Sync + Send + Clone,
{
    async fn check(&self) -> Result<()> {
        let uri = &self
            .client
            .calendar_home_set()
            .unwrap_or(self.client.base_url());
        self.client
            .check_support(uri)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))
    }

    /// Finds existing collections for this storage.
    ///
    /// Will only return collections stored under the principal's home. In most common scenarios,
    /// this implies that only collections owned by the current user are found and not other
    /// collections.
    ///
    /// Collections outside the principal's home can be referenced by using an absolute path.
    async fn discover_collections(&self) -> Result<Discovery> {
        self.client
            .find_calendars(None)
            .await?
            .into_iter()
            .map(|collection| {
                collection_id_for_href(&collection.href)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e))
                    .map(|id| DiscoveredCollection::new(collection.href, id))
            })
            .collect::<Result<Vec<_>>>()
            .map(Discovery::from)
    }

    async fn create_collection(&self, href: &str) -> Result<Collection> {
        self.client
            .create_calendar(href)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(Collection::new(href.to_string()))
    }

    /// Create a new calendar such that its `collection_id` matches the given input.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::PreconditionFailed`] if a home set was not found in the carddav
    /// server.
    async fn create_collection_with_id(&self, id: &CollectionId) -> Result<Collection> {
        let home_set = self.client.calendar_home_set().ok_or_else(|| {
            Error::new(
                ErrorKind::PreconditionFailed,
                "calendar home set not found in caldav server",
            )
        })?;

        let path = path_for_collection_in_home_set(home_set, id.as_ref());

        self.client
            .create_calendar(path.clone())
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(Collection::new(path))
    }

    /// Deletes a caldav collection.
    ///
    /// This method does multiple network calls to ensure that the collection is empty. If the
    /// server property supports `Etag` (it MUST as per the spec), this method guarantees that the
    /// collection is empty when deleting it.
    ///
    /// If the server is not compliant and does not support Etags, possible race conditions could
    /// occur and if calendar components are added to the collection at the same time, they may be
    /// deleted.
    async fn destroy_collection(&self, href: &str) -> Result<()> {
        let mut results = self
            .client
            .get_calendar_resources(href, &[href])
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;

        if results.len() != 1 {
            return Err(ErrorKind::InvalidData.into());
        }

        let item = results.pop().expect("results has exactly one item");
        if item.href != href {
            return Err(Error::new(
                ErrorKind::Uncategorised,
                format!("Requested href: {}, got: {}", href, item.href,),
            ));
        }

        let etag = item
            .content
            .map_err(|e| Error::new(ErrorKind::Uncategorised, format!("Got status code: {e}")))?
            .etag;
        // TODO: specific error kind type for MissingEtag?

        // TODO: if no etag -> use force deletion (and warn)

        // TODO: verify that the collection is actually a calendar collection?
        // This could be done by using discover above.
        let items = self.list_items(href).await?;
        if !items.is_empty() {
            return Err(ErrorKind::CollectionNotEmpty.into());
        }

        self.client
            .delete(href, etag)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(())
    }

    async fn list_items(&self, collection_href: &str) -> Result<Vec<ItemRef>> {
        let response = self.client.list_resources(collection_href).await?;
        let mut items = Vec::with_capacity(response.len());
        for r in response {
            items.push(ItemRef {
                href: r.href,
                etag: r
                    .details
                    .etag
                    .ok_or(Error::from(ErrorKind::InvalidData))?
                    .into(),
            });
        }
        Ok(items)
    }

    async fn get_item(&self, href: &str) -> Result<(IcsItem, Etag)> {
        let collection_href = collection_href_for_item(href)?;
        let mut results = self
            .client
            .get_calendar_resources(collection_href, &[href])
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;

        if results.len() != 1 {
            return Err(ErrorKind::InvalidData.into());
        }

        let item = results.pop().expect("results has exactly one item");
        if item.href != href {
            return Err(Error::new(
                ErrorKind::Uncategorised,
                format!("Requested href: {}, got: {}", href, item.href,),
            ));
        }

        let content = item
            .content
            .map_err(|e| Error::new(ErrorKind::Uncategorised, format!("Got status code: {e}")))?;

        Ok((IcsItem::from(content.data), content.etag.into()))
    }

    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem<IcsItem>>> {
        // TODO: use generics for CalDavClient+CardDavClient and make this method generic too.
        if hrefs.is_empty() {
            return Ok(Vec::new());
        }
        let collection_href = collection_href_for_item(hrefs[0])?;
        self.client
            .get_calendar_resources(collection_href, hrefs)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?
            .into_iter()
            .map(|resource| {
                resource
                    .content
                    .map_err(|e| {
                        ErrorKind::Io.error(format!("Got status code {} for {}", e, resource.href))
                    })
                    .map(|content| FetchedItem {
                        href: resource.href,
                        item: IcsItem::from(content.data),
                        etag: content.etag.into(),
                    })
            })
            .collect()
    }

    async fn get_all_items(&self, collection: &str) -> Result<Vec<FetchedItem<IcsItem>>> {
        let list = self.list_items(collection).await?;
        let hrefs = list.iter().map(|i| i.href.as_str()).collect::<Vec<_>>();
        self.get_many_items(&hrefs).await
    }

    async fn add_item(&self, collection_href: &str, item: &IcsItem) -> Result<ItemRef> {
        let href = join_hrefs(collection_href, &item.ident());
        // TODO: ident: .chars().filter(char::is_ascii_alphanumeric)

        let response = self
            .client
            .create_resource(
                &href,
                item.as_str().as_bytes().to_vec(),
                mime_types::CALENDAR,
            )
            .await?;
        let etag = match response {
            Some(e) => e,
            // TODO: we should only perform a HEAD request here; we don't need actual data.
            None => self.get_item(&href).await?.1.to_string(),
        };
        Ok(ItemRef {
            href,
            etag: Etag::from(etag),
        })
    }

    async fn update_item(&self, href: &str, etag: &Etag, item: &IcsItem) -> Result<Etag> {
        // TODO: check that href is a sub-path of collection.href?
        self.client
            .update_resource(
                href,
                item.as_str().as_bytes().to_vec(),
                etag,
                mime_types::CALENDAR,
            )
            .await
            // FIXME: etag may be missing. In such case, we should fetch it.
            .map(|opt| opt.ok_or(Error::new(ErrorKind::InvalidData, "No Etag in response")))?
            .map(Etag::from)
    }

    /// # Errors
    ///
    /// Only `DisplayName` and `Colour` are implemented.
    async fn set_collection_property(
        &self,
        collection_href: &str,
        meta: CalendarProperty,
        value: &str,
    ) -> Result<()> {
        match meta {
            CalendarProperty::DisplayName => self
                .client
                .set_collection_displayname(collection_href, Some(value))
                .await
                .map_err(Error::from),
            CalendarProperty::Colour => self
                .client
                .set_calendar_colour(collection_href, Some(value))
                .await
                .map_err(Error::from),
            _ => Err(Error::from(ErrorKind::Unsupported)),
        }
    }

    /// Read metadata from a collection.
    ///
    /// Metadata is fetched using the `PROPFIND` method under the hood. Some servers may not
    /// support some properties.
    ///
    /// # Errors
    ///
    /// If the underlying HTTP connection fails or if the server returns invalid data.
    ///
    /// Only `DisplayName` and `Colour` are implemented.
    async fn get_collection_property(
        &self,
        collection_href: &str,
        meta: CalendarProperty,
    ) -> Result<Option<String>> {
        match meta {
            CalendarProperty::DisplayName => self
                .client
                .get_collection_displayname(collection_href)
                .await
                .map_err(Error::from),
            CalendarProperty::Colour => self
                .client
                .get_calendar_colour(collection_href)
                .await
                .map_err(Error::from),
            _ => Err(Error::from(ErrorKind::Unsupported)),
        }
    }

    async fn delete_item(&self, href: &str, etag: &Etag) -> Result<()> {
        // TODO: check that href is a sub-path of this storage?
        self.client.delete(href, etag).await?;

        Ok(())
    }

    /// The id of a caldav collection is the last component of the path.
    fn collection_id(&self, collection_href: &str) -> Result<CollectionId> {
        // TODO: this will need to be different for Google's WebDav.
        collection_id_for_href(collection_href).map_err(|e| Error::new(ErrorKind::InvalidInput, e))
    }
}

#[cfg(test)]
mod test {
    use hyper_rustls::HttpsConnectorBuilder;
    use libdav::{auth::Auth, CalDavClient};

    use crate::{base::Storage, caldav::CalDavStorage};

    #[test]
    fn test_collection_id() {
        let test_client = {
            let https = HttpsConnectorBuilder::new()
                .with_native_roots()
                .https_or_http()
                .enable_http1()
                .build();
            let client = CalDavClient::builder()
                .with_uri("https://example.com".parse().unwrap())
                .with_auth(Auth::None)
                .without_discovery(https)
                .build();

            CalDavStorage { client }
        };

        let samples = &[
            ("/path/to/collection/", "collection"),
            ("/path/to/collection", "collection"),
            ("/path/to//collection/", "collection"),
            ("/path/to/collection//", "collection"),
            ("path/to/collection", "collection"),
            ("/", ""),
        ];
        for (input, output) in samples {
            let collection_id = test_client.collection_id(input).unwrap();

            assert_eq!(collection_id, output.parse().unwrap());
        }
    }
}

fn join_hrefs(collection_href: &str, item_href: &str) -> String {
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
