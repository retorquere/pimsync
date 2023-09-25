// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! A [`CardDavStorage`] is a single carddav repository, as specified in rfc6352.

use async_trait::async_trait;
use http::Uri;
use hyper::client::connect::Connect;
use libdav::auth::Auth;
use libdav::dav::mime_types;
use libdav::CardDavClient;

use crate::base::{AddressBookProperty, Collection, Definition, Item, ItemRef, Storage, VcardItem};
use crate::dav::path_for_collection_in_home_set;
use crate::{CollectionId, Error, ErrorKind, Etag, Href, Result};

#[derive(Debug)]
pub struct CardDavDefinition<C>
where
    C: Connect + Send + Sync + Clone + 'static,
{
    pub url: Uri,
    pub auth: Auth,
    pub connector: C,
}
impl<C> CardDavDefinition<C>
where
    C: Connect + Send + Sync + Clone + std::fmt::Debug,
{
    pub async fn build(self) -> Result<CardDavStorage<C>> {
        let client = CardDavClient::builder()
            .with_uri(self.url)
            .with_auth(self.auth)
            .build(self.connector)
            .await?;

        Ok(CardDavStorage { client })
    }
}

#[async_trait]
impl<C> Definition<VcardItem> for CardDavDefinition<C>
where
    C: Connect + Send + Sync + Clone + 'static + std::fmt::Debug,
{
    async fn build_boxed(self) -> Result<Box<dyn Storage<VcardItem>>> {
        Ok(Box::from(self.build().await?))
    }
}

/// A storage backed by a carddav server.
///
/// A single storage represents a single server with a specific set of credentials.
pub struct CardDavStorage<C: Connect + Clone + Sync + Send + 'static> {
    client: CardDavClient<C>,
}

#[async_trait]
impl<C> Storage<VcardItem> for CardDavStorage<C>
where
    C: Connect + Clone + Sync + Send,
{
    async fn check(&self) -> Result<()> {
        let uri = &self
            .client
            .addressbook_home_set()
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
    /// Collections outside the principal's home can still be found by providing an absolute path
    /// to [`CardDavStorage::open_collection`].
    async fn discover_collections(&self) -> Result<Vec<Collection>> {
        let x = self
            .client
            .find_addresbooks(None)
            .await?
            .into_iter()
            .map(|collection| Collection::new(collection.href))
            .collect::<Vec<_>>();
        Ok(x)
    }

    async fn create_collection(&self, href: &str) -> Result<Collection> {
        self.client
            .create_addressbook(href)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(Collection::new(href.to_string()))
    }

    /// Create a new address book such that its `collection_id` matches the given input.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::PreconditionFailed`] if a home set was not found in the carddav
    /// server.
    async fn create_collection_with_id(&self, id: &CollectionId) -> Result<Collection> {
        let home_set = self.client.addressbook_home_set().ok_or_else(|| {
            Error::new(
                ErrorKind::PreconditionFailed,
                "address book home set not found in carddav server",
            )
        })?;

        let path = path_for_collection_in_home_set(home_set, id.as_ref());

        self.client
            .create_addressbook(path.clone())
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(Collection::new(path))
    }

    /// Deletes a carddav collection.
    ///
    /// This method does multiple network calls to ensure that the collection is empty. If the
    /// server property supports `Etag` (it MUST as per the spec), this method guarantees that the
    /// collection is empty when deleting it.
    ///
    /// If the server is not compliant and does not support Etags, possible race conditions could
    /// occur and if address book components are added to the collection at the same time, they may
    /// be deleted.
    async fn destroy_collection(&self, href: &str) -> Result<()> {
        let mut results = self
            .client
            .get_resources(href, &[href])
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
        let collection = Collection::new(href.to_string());

        // TODO: verify that the collection is actually an address book collection?
        // This could be done by using discover above.
        let items = self.list_items(&collection).await?;
        if !items.is_empty() {
            return Err(ErrorKind::CollectionNotEmpty.into());
        }

        self.client
            .delete(href, etag)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?;
        Ok(())
    }

    fn open_collection(&self, href: &str) -> Result<Collection> {
        Ok(Collection::new(href.to_string()))
    }

    async fn list_items(&self, collection: &Collection) -> Result<Vec<ItemRef>> {
        let response = self.client.list_resources(collection.href()).await?;
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

    async fn get_item(&self, collection: &Collection, href: &str) -> Result<(VcardItem, Etag)> {
        let mut results = self
            .client
            .get_resources(&collection.href(), &[href])
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

        Ok((VcardItem::from(content.data), content.etag.into()))
    }

    async fn get_many_items(
        &self,
        collection: &Collection,
        hrefs: &[&str],
    ) -> Result<Vec<(Href, VcardItem, Etag)>> {
        Ok(self
            .client
            .get_resources(&collection.href(), hrefs)
            .await
            .map_err(|e| Error::new(ErrorKind::Uncategorised, e))?
            .into_iter()
            .map(|r| {
                let content = r.content.unwrap();
                (r.href, VcardItem::from(content.data), content.etag.into())
            })
            .collect())
    }

    async fn get_all_items(&self, collection: &Collection) -> Result<Vec<(Href, VcardItem, Etag)>> {
        let list = self.list_items(collection).await?;
        let hrefs = list.iter().map(|i| i.href.as_str()).collect::<Vec<_>>();
        self.get_many_items(collection, &hrefs).await
    }

    async fn add_item(&self, collection: &Collection, item: &VcardItem) -> Result<ItemRef> {
        let href = join_hrefs(collection.href(), &item.ident());
        // TODO: ident: .chars().filter(char::is_ascii_alphanumeric)

        self.client
            // FIXME: should not copy data here?
            .create_resource(
                &href,
                item.as_str().as_bytes().to_vec(),
                mime_types::ADDRESSBOOK,
            )
            .await
            // FIXME: etag may be missing. In such case, we should fetch it.
            .map(|opt| opt.ok_or(Error::new(ErrorKind::InvalidData, "No Etag in response")))?
            .map(|etag| ItemRef {
                href,
                etag: etag.into(),
            })
    }

    async fn update_item(
        &self,
        _collection: &Collection,
        href: &str,
        etag: &Etag,
        item: &VcardItem,
    ) -> Result<Etag> {
        // TODO: check that href is a sub-path of collection.href?
        self.client
            .update_resource(
                href,
                item.as_str().as_bytes().to_vec(),
                etag,
                mime_types::ADDRESSBOOK,
            )
            .await
            // FIXME: etag may be missing. In such case, we should fetch it.
            .map(|opt| opt.ok_or(Error::new(ErrorKind::InvalidData, "No Etag in response")))?
            .map(Etag::from)
    }

    /// # Panics
    ///
    /// Only `DisplayName` is implemented.
    async fn set_collection_property(
        &self,
        collection: &Collection,
        meta: AddressBookProperty,
        value: &str,
    ) -> Result<()> {
        // TODO: make MetaKind paramatrezed on the ItemKind
        match meta {
            AddressBookProperty::DisplayName => {
                self.client
                    .set_collection_displayname(collection.href(), Some(value))
                    .await
            }
            AddressBookProperty::Description => {
                todo!(); // TODO FIXME
            }
        }
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
    /// # Panics
    ///
    /// Only `DisplayName` is implemented.
    async fn get_collection_property(
        &self,
        collection: &Collection,
        meta: AddressBookProperty,
    ) -> Result<Option<String>> {
        let result = match meta {
            AddressBookProperty::DisplayName => {
                self.client
                    .get_collection_displayname(collection.href())
                    .await
            }
            AddressBookProperty::Description => {
                todo!(); // TODO FIXME
            }
        };

        result.map_err(Error::from)
    }

    async fn delete_item(&self, _collection: &Collection, href: &str, etag: &Etag) -> Result<()> {
        // TODO: check that href is a sub-path of collection.href?
        self.client.delete(href, etag).await?;

        Ok(())
    }

    /// The `collection_id` of a carddav collection is the last component of the path.
    fn collection_id(&self, collection: &Collection) -> Result<CollectionId> {
        // TODO: this will need to be different for Google's WebDav.
        collection
            .href()
            .rsplit('/')
            .next()
            .expect("rsplit always returns at least one item")
            .parse()
            .map_err(|e| Error::new(ErrorKind::InvalidInput, e))
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
