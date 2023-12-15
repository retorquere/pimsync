// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::sync::Arc;

use hyper_rustls::HttpsConnectorBuilder;
use libdav::auth::Auth;
use vstorage::{
    base::{Collection, Definition, FetchedItem, IcsItem, Storage},
    caldav::CalDavDefinition,
    filesystem::FilesystemDefinition,
};

async fn create_caldav_from_env() -> Arc<dyn Storage<IcsItem>> {
    let server = std::env::var("CALDAV_SERVER").unwrap();
    let username = std::env::var("CALDAV_USERNAME").unwrap();
    let password = std::env::var("CALDAV_PASSWORD").unwrap().into();

    let connector = HttpsConnectorBuilder::new()
        .with_native_roots()
        .https_or_http()
        .enable_http1()
        .build();
    CalDavDefinition {
        url: server.parse().unwrap(),
        auth: Auth::Basic {
            username,
            password: Some(password),
        },
        connector,
    }
    .into_storage()
    .await
    .unwrap()
}

async fn create_vdir_from_env() -> Arc<dyn Storage<IcsItem>> {
    let path = std::env::var("VDIR_PATH").unwrap();
    FilesystemDefinition::new(path.try_into().unwrap(), "ics".to_string())
        .into_storage()
        .await
        .unwrap()
}
#[tokio::main]
async fn main() {
    let caldav_storage = create_caldav_from_env().await;
    let vdir_storage = create_vdir_from_env().await;

    let discovery = caldav_storage.discover_collections().await.unwrap();

    println!("Found {} collections", discovery.collection_count());
    for collection in discovery.collections() {
        println!("Creating {}", collection.href());
        let collection_name = collection
            .href()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .expect("collection has at least one path segument");
        let new_collection = vdir_storage
            .create_collection(collection_name)
            .await
            .unwrap();

        copy_collection(&caldav_storage, collection, &vdir_storage, &new_collection).await;
    }
}

/// Copies from `source` to `target` and returns the amount of items copied.
async fn copy_collection(
    source_storage: &Arc<dyn Storage<IcsItem>>,
    source_collection: &Collection,
    target_storage: &Arc<dyn Storage<IcsItem>>,
    target_collection: &Collection,
) -> usize {
    let mut count = 0;
    for FetchedItem { item, .. } in source_storage
        .get_all_items(&source_collection)
        .await
        .expect("webcal remote has items")
    {
        count += 1;
        target_storage
            .add_item(&target_collection, &item)
            .await
            .expect("write to local filesystem collection");
    }

    count
}
