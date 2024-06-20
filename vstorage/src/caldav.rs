// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! A [`CalDavStorage`] is a single caldav repository, as specified in rfc4791.

use async_trait::async_trait;
use http::Uri;
use hyper::client::connect::Connect;
use libdav::auth::Auth;
use libdav::dav::{mime_types, WebDavClient};
use libdav::sd::BootstrapError;
use libdav::CalDavClient;

use crate::base::{
    CalendarProperty, Collection, FetchedItem, IcsItem, Item, ItemRef, ListedProperty,
    PropertyTarget, Storage,
};
use crate::dav::{
    collection_href_for_item, collection_id_for_href, path_for_collection_in_home_set,
};
use crate::disco::{DiscoveredCollection, Discovery};
use crate::vdir::PropertyWithFilename;
use crate::{CollectionId, Error, ErrorKind, Etag, Href, Result};

impl<C> CalDavStorage<C>
where
    C: Connect + Send + Sync + Clone + std::fmt::Debug,
{
    /// Build a new `Storage` instance.
    ///
    /// # Errors
    ///
    /// If there are errors discovering the CalDav server.
    pub async fn new(url: Uri, auth: Auth, connector: C) -> Result<CalDavStorage<C>> {
        let webdav = WebDavClient::new(url, auth, connector);
        let client = CalDavClient::new_via_bootstrap(webdav).await?;
        let principal = client
            .find_current_user_principal()
            .await
            .map_err(|e| Error::new(ErrorKind::Io, e))?
            .ok_or_else(|| Error::new(ErrorKind::Unavailable, "no user principal found"))?;
        let calendar_home_set = client
            .find_calendar_home_set(&principal)
            .await
            .map_err(|e| Error::new(ErrorKind::Io, e))?;

        Ok(CalDavStorage {
            client,
            calendar_home_set,
        })
    }
}

impl From<BootstrapError> for Error {
    fn from(value: BootstrapError) -> Self {
        // TODO: not implemented
        Error::new(ErrorKind::Uncategorised, value)
    }
}

impl From<libdav::dav::WebDavError> for Error {
    fn from(value: libdav::dav::WebDavError) -> Self {
        // TODO: not implemented
        Error::new(ErrorKind::Uncategorised, value)
    }
}

/// A storage backed by a caldav server.
///
/// A single storage represents a single server with a specific set of credentials.
pub struct CalDavStorage<C: Connect + Clone + Sync + Send + 'static> {
    client: CalDavClient<C>,
    calendar_home_set: Option<Uri>,
}

#[async_trait]
impl<C> Storage<IcsItem> for CalDavStorage<C>
where
    C: Connect + Sync + Send + Clone,
{
    async fn check(&self) -> Result<()> {
        let url = self
            .calendar_home_set
            .as_ref()
            .unwrap_or(&self.client.base_url);
        self.client
            .check_support(url)
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
        let Some(home_set) = &self.calendar_home_set else {
            return Err(Error::new(
                ErrorKind::PreconditionFailed,
                "calendar home set was not found",
            ));
        };

        self.client
            .find_calendars(home_set)
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

        if results.len() > 1 {
            return Err(ErrorKind::InvalidData.into());
        }

        let item = results
            .pop()
            .ok_or_else(|| Error::from(ErrorKind::InvalidData))?;

        if item.href != href {
            return Err(Error::new(
                ErrorKind::InvalidData,
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
        let raw_etag = self
            .client
            .update_resource(
                href,
                item.as_str().as_bytes().to_vec(),
                etag,
                mime_types::CALENDAR,
            )
            .await?;
        if let Some(etag) = raw_etag {
            return Ok(Etag::from(etag));
        }
        let (new_item, etag) = self.get_item(href).await?;
        if new_item.hash() == item.hash() {
            return Ok(etag);
        }
        return Err(ErrorKind::Io.error("Item was overwritten replaced before reading Etag"));
    }

    /// # Errors
    ///
    /// Only `DisplayName` and `Colour` are implemented.
    async fn set_property(&self, href: &str, prop: CalendarProperty, value: &str) -> Result<()> {
        self.client
            .set_property(href, prop.dav_propname(), Some(value))
            .await
            .map_err(Error::from)
    }

    /// # Errors
    ///
    /// Only `DisplayName` and `Colour` are implemented.
    async fn unset_property(&self, href: &str, prop: CalendarProperty) -> Result<()> {
        self.client
            .set_property(href, prop.dav_propname(), None)
            .await
            .map_err(Error::from)
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
    async fn get_property(&self, href: &str, prop: CalendarProperty) -> Result<Option<String>> {
        self.client
            .get_property(href, prop.dav_propname())
            .await
            .map_err(Error::from)
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

    /// # Errors
    ///
    /// Returns [`ErrorKind::PreconditionFailed`] if a home set was not found in the carddav
    /// server.
    fn href_for_collection_id(&self, id: &CollectionId) -> Result<Href> {
        if let Some(home_set) = &self.calendar_home_set {
            Ok(path_for_collection_in_home_set(home_set, id.as_ref()))
        } else {
            Err(Error::new(
                ErrorKind::PreconditionFailed,
                "calendar home set not found in caldav server",
            ))
        }
    }

    async fn list_properties(&self, collection_href: &str) -> Result<Vec<ListedProperty<IcsItem>>> {
        let mut props = Vec::new();
        for property in CalendarProperty::known_properties() {
            let prop_value = self.get_property(collection_href, property.clone()).await?;
            if let Some(value) = prop_value {
                props.push(ListedProperty {
                    resource: PropertyTarget::Collection(collection_href.to_owned()),
                    property: property.clone(),
                    value,
                });
            };
        }
        return Ok(props);
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

#[cfg(test)]
mod test {
    use hyper_rustls::HttpsConnectorBuilder;
    use libdav::{auth::Auth, dav::WebDavClient, CalDavClient};

    use crate::{base::Storage, caldav::CalDavStorage};

    #[test]
    fn test_collection_id() {
        let test_client = {
            let https = HttpsConnectorBuilder::new()
                .with_native_roots()
                .unwrap()
                .https_or_http()
                .enable_http1()
                .build();
            let base_url = "https://example.com".parse().unwrap();
            let webdav = WebDavClient::new(base_url, Auth::None, https);
            let client = CalDavClient::new(webdav);

            CalDavStorage {
                client,
                calendar_home_set: None,
            }
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
