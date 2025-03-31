// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::sync::Arc;

use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::{client::legacy::Client as HyperClient, rt::TokioExecutor};
use libdav::{dav::WebDavClient, CalDavClient};
use tower_http::auth::AddAuthorization;
use vstorage::{
    base::{CreateItemOptions, FetchedItem, Storage},
    caldav::CalDavStorage,
    vdir::VdirStorage,
    ItemKind,
};

async fn create_caldav_from_env() -> Arc<dyn Storage> {
    let server = std::env::var("CALDAV_SERVER").unwrap();
    let username = std::env::var("CALDAV_USERNAME").unwrap();
    let password = std::env::var("CALDAV_PASSWORD").unwrap();

    let connector = HttpsConnectorBuilder::new()
        .with_native_roots()
        .unwrap()
        .https_or_http()
        .enable_http1()
        .build();
    let raw_client = HyperClient::builder(TokioExecutor::new()).build(connector);
    let auth_client = AddAuthorization::basic(raw_client, &username, &password);
    let webdav = WebDavClient::new(server.parse().unwrap(), auth_client);
    let caldav = CalDavClient::bootstrap_via_service_discovery(webdav)
        .await
        .unwrap();
    let storage = CalDavStorage::new(caldav).await.unwrap();
    Arc::from(storage)
}

async fn create_vdir_from_env() -> Arc<dyn Storage> {
    let path = std::env::var("VDIR_PATH").unwrap();
    let storage = VdirStorage::new(path.into(), "ics".to_string(), ItemKind::Calendar);
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
        let collection_id = discovered_collection
            .href()
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .expect("collection has at least one path segument");
        let new_collection = vdir_storage.create_collection(collection_id).await.unwrap();

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
    source_storage: &dyn Storage,
    source_collection_href: &str,
    target_storage: &dyn Storage,
    target_collection_href: &str,
) -> usize {
    let mut count = 0;
    for FetchedItem { item, .. } in source_storage
        .get_all_items(source_collection_href)
        .await
        .expect("webcal remote has items")
    {
        let opts = CreateItemOptions::default();
        count += 1;
        target_storage
            .create_item(target_collection_href, &item, opts)
            .await
            .expect("write to local filesystem collection");
    }

    count
}
