// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::sync::Arc;

use hyper_rustls::HttpsConnectorBuilder;
use libdav::auth::Auth;
use vstorage::{
    base::{FetchedItem, IcsItem, Storage},
    caldav::CalDavStorage,
    vdir::VdirStorage,
};

async fn create_caldav_from_env() -> Arc<dyn Storage<IcsItem>> {
    let server = std::env::var("CALDAV_SERVER").unwrap();
    let username = std::env::var("CALDAV_USERNAME").unwrap();
    let password = std::env::var("CALDAV_PASSWORD").unwrap().into();

    let connector = HttpsConnectorBuilder::new()
        .with_native_roots()
        .unwrap()
        .https_or_http()
        .enable_http1()
        .build();
    let storage = CalDavStorage::new(
        server.parse().unwrap(),
        Auth::Basic {
            username,
            password: Some(password),
        },
        connector,
    )
    .await
    .unwrap();
    Arc::from(storage)
}

async fn create_vdir_from_env() -> Arc<dyn Storage<IcsItem>> {
    let path = std::env::var("VDIR_PATH").unwrap();
    let storage = VdirStorage::new(path.into(), "ics".to_string());
    Arc::new(storage)
}
#[tokio::main]
async fn main() {
    let caldav_storage = create_caldav_from_env().await;
    let vdir_storage = create_vdir_from_env().await;

    let discovery = caldav_storage.discover_collections().await.unwrap();

    println!("Found {} collections", discovery.collection_count());
    for discovered_collection in discovery.collections() {
        println!("Creating {}", discovered_collection.href());
        let collection_name = discovered_collection
            .href()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .expect("collection has at least one path segument");
        let new_collection = vdir_storage
            .create_collection(collection_name)
            .await
            .unwrap();

        copy_collection(
            caldav_storage.as_ref(),
            discovered_collection.href(),
            vdir_storage.as_ref(),
            new_collection.href(),
        )
        .await;
    }
}

/// Copies from `source` to `target` and returns the amount of items copied.
async fn copy_collection(
    source_storage: &dyn Storage<IcsItem>,
    source_collection_href: &str,
    target_storage: &dyn Storage<IcsItem>,
    target_collection_href: &str,
) -> usize {
    let mut count = 0;
    for FetchedItem { item, .. } in source_storage
        .get_all_items(source_collection_href)
        .await
        .expect("webcal remote has items")
    {
        count += 1;
        target_storage
            .add_item(target_collection_href, &item)
            .await
            .expect("write to local filesystem collection");
    }

    count
}
