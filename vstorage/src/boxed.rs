use crate::base::Collection;
use crate::base::FetchedItem;
use crate::base::ItemRef;
use crate::base::{Item, Storage};
use crate::disco::Discovery;
use crate::{CollectionId, Etag, Result};

use async_trait::async_trait;

#[async_trait]
// impl<I: Item> Storage<I> for Box<dyn Storage<I> + '_> {
impl<I: Item> Storage<I> for Box<dyn Storage<I>> {
    async fn check(&self) -> Result<()> {
        AsRef::<dyn Storage<I>>::as_ref(self).check().await
    }

    async fn discover_collections(&self) -> Result<Discovery> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .discover_collections()
            .await
    }

    async fn create_collection(&self, href: &str) -> Result<Collection> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .create_collection(href)
            .await
    }

    async fn create_collection_with_id(&self, id: &CollectionId) -> Result<Collection> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .create_collection_with_id(id)
            .await
    }

    async fn destroy_collection(&self, href: &str) -> Result<()> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .destroy_collection(href)
            .await
    }

    async fn get_collection_property(
        &self,
        collection_href: &str,
        property: I::CollectionProperty,
    ) -> Result<Option<String>> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .get_collection_property(collection_href, property)
            .await
    }

    async fn set_collection_property(
        &self,
        collection_href: &str,
        property: I::CollectionProperty,
        value: &str,
    ) -> Result<()> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .set_collection_property(collection_href, property, value)
            .await
    }

    async fn list_items(&self, collection_href: &str) -> Result<Vec<ItemRef>> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .list_items(collection_href)
            .await
    }

    async fn get_item(&self, href: &str) -> Result<(I, Etag)> {
        AsRef::<dyn Storage<I>>::as_ref(self).get_item(href).await
    }

    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem<I>>> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .get_many_items(hrefs)
            .await
    }

    async fn get_all_items(&self, collection_href: &str) -> Result<Vec<FetchedItem<I>>> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .get_all_items(collection_href)
            .await
    }

    async fn add_item(&self, collection_href: &str, item: &I) -> Result<ItemRef> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .add_item(collection_href, item)
            .await
    }

    async fn update_item(&self, href: &str, etag: &Etag, item: &I) -> Result<Etag> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .update_item(href, etag, item)
            .await
    }

    async fn delete_item(&self, href: &str, etag: &Etag) -> Result<()> {
        AsRef::<dyn Storage<I>>::as_ref(self)
            .delete_item(href, etag)
            .await
    }

    fn collection_id(&self, collection_href: &str) -> Result<CollectionId> {
        AsRef::<dyn Storage<I>>::as_ref(self).collection_id(collection_href)
    }
}
