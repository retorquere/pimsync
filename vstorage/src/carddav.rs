// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! A [`CardDavStorage`] is a single carddav repository, as specified in rfc6352.

use async_trait::async_trait;
use http::{Request, Response, StatusCode, Uri};
use hyper::body::Incoming;
use libdav::dav::mime_types;
use libdav::CardDavClient;
use tower::Service;

use crate::addressbook::{AddressBookProperty, VcardItem};
use crate::base::{
    Collection, FetchedItem, FetchedProperty, Item, ItemVersion, Property as _, Storage,
};
use crate::dav::{
    collection_href_for_item, collection_id_for_href, join_hrefs, parse_list_items,
    path_for_collection_in_home_set,
};
use crate::disco::{DiscoveredCollection, Discovery};
use crate::{CollectionId, Error, ErrorKind, Etag, Href, Result};

impl<C> CardDavStorage<C>
where
    C: Service<Request<String>, Response = Response<Incoming>> + Send + Sync + 'static,
    C::Error: std::error::Error + Send + Sync,
    C::Future: Send + Sync,
{
    /// Build a new `Storage` instance.
    ///
    /// # Errors
    ///
    /// If there are errors discovering the CardDAV server.
    pub async fn new(client: CardDavClient<C>) -> Result<CardDavStorage<C>> {
        let principal = client
            .find_current_user_principal()
            .await
            .map_err(|e| Error::new(ErrorKind::Io, e))?
            .ok_or_else(|| Error::new(ErrorKind::Unavailable, "no user principal found"))?;
        let address_book_home_set = client
            .find_address_book_home_set(&principal)
            .await
            .map_err(|e| Error::new(ErrorKind::Io, e))?;

        Ok(CardDavStorage {
            client,
            address_book_home_set,
        })
    }
}

/// A storage backed by a carddav server.
///
/// A single storage represents a single server with a specific set of credentials.
pub struct CardDavStorage<C>
where
    C: Service<Request<String>, Response = Response<Incoming>> + Send + Sync + 'static,
    C::Error: std::error::Error + Send + Sync,
    C::Future: Send + Sync,
{
    client: CardDavClient<C>,
    address_book_home_set: Vec<Uri>,
}

#[async_trait]
impl<C> Storage<VcardItem> for CardDavStorage<C>
where
    C: Service<Request<String>, Response = Response<Incoming>> + Send + Sync + 'static,
    C::Error: std::error::Error + Send + Sync,
    C::Future: Send + Sync,
{
    async fn check(&self) -> Result<()> {
        self.client
            .check_support(&self.client.base_url)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))
    }

    /// Finds existing collections for this storage.
    ///
    /// Will only return collections stored under the principal's home set. In most common
    /// scenarios, this implies that only collections owned by the current user are found and not
    /// other collections.
    ///
    /// Collections outside the principal's home set can be referenced by using an absolute path.
    async fn discover_collections(&self) -> Result<Discovery> {
        let mut collections = Vec::new();
        for home in &self.address_book_home_set {
            collections.append(&mut self.client.find_addressbooks(home).await?);
        }

        collections
            .into_iter()
            .map(|collection| {
                collection_id_for_href(&collection.href)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e))
                    .map(|id| DiscoveredCollection::new(collection.href, id))
            })
            .collect::<Result<Vec<_>>>()
            .map(Discovery::try_from)?
            .map_err(|e| ErrorKind::InvalidData.error(e))
    }

    async fn create_collection(&self, href: &str) -> Result<Collection> {
        self.client
            .create_addressbook(href)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(Collection::new(href.to_string()))
    }

    /// Deletes a carddav collection.
    ///
    /// This method does multiple network calls to ensure that the collection is empty. If the
    /// server property supports `Etag` (it MUST as per the spec), this method guarantees that the
    /// collection is empty when deleting it.
    ///
    /// If the server is not compliant and does not support Etags, possible race conditions could
    /// occur and if contacts components are added to the collection concurrently, they may be
    /// deleted.
    async fn destroy_collection(&self, href: &str) -> Result<()> {
        let mut results = self
            .client
            .get_address_book_resources(href, &[href])
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;

        // We requested the collection only; only that should be returned.
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

        // TODO: verify that the collection is actually an address book collection?
        // This could be done by using discover above.
        let items = self.list_items(href).await?;
        if !items.is_empty() {
            return Err(ErrorKind::CollectionNotEmpty.into());
        }

        self.client
            .delete(href, &etag)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(())
    }

    async fn list_items(&self, collection_href: &str) -> Result<Vec<ItemVersion>> {
        let response = self.client.list_resources(collection_href).await?;
        parse_list_items(response)
    }

    async fn get_item(&self, href: &str) -> Result<(Item, Etag)> {
        let collection_href = collection_href_for_item(href)?;
        let mut results = self
            .client
            .get_address_book_resources(collection_href, &[href])
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

        Ok((Item::from(content.data), content.etag.into()))
    }

    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem>> {
        // TODO: use generics for CalDavClient+CardDavClient and make this method generic too.
        if hrefs.is_empty() {
            return Ok(Vec::new());
        }
        let collection_href = collection_href_for_item(hrefs[0])?;
        self.client
            .get_address_book_resources(collection_href, hrefs)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?
            .into_iter()
            .filter_map(|resource| match resource.content {
                Ok(content) => Some(Ok(FetchedItem {
                    href: resource.href,
                    item: Item::from(content.data),
                    etag: content.etag.into(),
                })),
                Err(StatusCode::NOT_FOUND) => None,
                Err(e) => Some(Err(
                    ErrorKind::Io.error(format!("Got status code {} for {}", e, resource.href))
                )),
            })
            .collect()
    }

    async fn get_all_items(&self, collection_href: &str) -> Result<Vec<FetchedItem>> {
        let list = self.list_items(collection_href).await?;
        let hrefs = list.iter().map(|i| i.href.as_str()).collect::<Vec<_>>();
        self.get_many_items(&hrefs).await
    }

    async fn add_item(&self, collection_href: &str, item: &Item) -> Result<ItemVersion> {
        let href = join_hrefs(collection_href, &item.ident());
        // TODO: ident: .chars().filter(char::is_ascii_alphanumeric)

        let response = self
            .client
            // FIXME: should not copy data here?
            .create_resource(
                &href,
                item.as_str().as_bytes().to_vec(),
                mime_types::ADDRESSBOOK,
            )
            .await?;
        let etag = match response {
            Some(e) => e,
            // TODO: we should only perform a HEAD request here; we don't need actual data.
            None => self.get_item(&href).await?.1.to_string(),
        };
        Ok(ItemVersion {
            href,
            etag: Etag::from(etag),
        })
    }

    async fn update_item(&self, href: &str, etag: &Etag, item: &Item) -> Result<Etag> {
        // TODO: check that href is a sub-path of collection_href
        let raw_etag = self
            .client
            .update_resource(
                href,
                item.as_str().as_bytes().to_vec(),
                etag.as_str(),
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
    /// Only `DisplayName` is implemented.
    async fn set_property(&self, href: &str, prop: AddressBookProperty, value: &str) -> Result<()> {
        self.client
            .set_property(href, prop.dav_propname(), Some(value))
            .await
            .map(|_| ())
            .map_err(Error::from)
    }

    /// # Errors
    ///
    /// Only `DisplayName` is implemented.
    async fn unset_property(&self, href: &str, prop: AddressBookProperty) -> Result<()> {
        self.client
            .set_property(href, prop.dav_propname(), None)
            .await
            .map(|_| ())
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
    /// Only `DisplayName` is implemented.
    async fn get_property(&self, href: &str, prop: AddressBookProperty) -> Result<Option<String>> {
        self.client
            .get_property(href, prop.dav_propname())
            .await
            .map_err(Error::from)
    }

    async fn delete_item(&self, href: &str, etag: &Etag) -> Result<()> {
        // TODO: check that href is a sub-path of this storage?
        self.client.delete(href, etag.as_str()).await?;

        Ok(())
    }

    /// # Errors
    ///
    /// Returns [`ErrorKind::PreconditionFailed`] if a home set was not found in the carddav
    /// server.
    fn href_for_collection_id(&self, id: &CollectionId) -> Result<Href> {
        if let Some(home_set) = &self.address_book_home_set.first() {
            Ok(path_for_collection_in_home_set(home_set, id.as_ref()))
        } else {
            Err(Error::new(
                ErrorKind::PreconditionFailed,
                "calendar home set not found in caldav server",
            ))
        }
    }

    async fn list_properties(
        &self,
        collection_href: &str,
    ) -> Result<Vec<FetchedProperty<AddressBookProperty>>> {
        let prop_names = AddressBookProperty::known_properties()
            .iter()
            .map(|p| p.dav_propname())
            .collect::<Vec<_>>();
        let result = self
            .client
            .get_properties(collection_href, &prop_names)
            .await?
            .into_iter()
            .zip(AddressBookProperty::known_properties())
            .filter_map(|((_, v), p)| {
                v.map(|value| FetchedProperty {
                    property: *p,
                    value,
                })
            })
            .collect::<Vec<_>>();

        return Ok(result);
    }
}
