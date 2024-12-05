// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! This example copies all entries from a remote webcal storage into a local filesystem storage.
//! It DOES NOT synchronise items; it does a blind one-way copy.
//!
//! This is mostly a proof of concept of the basic storage implementations.
//!
//! Usage:
//!
//! ```
//! cargo run --example=webcal_to_vdir https://www.officeholidays.com/ics/netherlands /tmp/holidays
//! ```

use camino::Utf8PathBuf;
use http::Uri;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::{client::legacy::Client as HyperClient, rt::TokioExecutor};
use std::sync::Arc;
use vstorage::base::FetchedItem;
use vstorage::base::Item;
use vstorage::base::Storage;
use vstorage::vdir::VdirStorage;
use vstorage::webcal::WebCalStorage;

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args();
    arguments
        .next()
        .expect("binary has been called with a name");
    let raw_url = arguments.next().expect("$1 is a valid URL");
    let raw_path = arguments.next().expect("$2 is a valid path");

    let url = Uri::try_from(raw_url.as_str()).expect("provided URL must be valid");
    let path = Utf8PathBuf::from(raw_path);

    let connector = HttpsConnectorBuilder::new()
        .with_native_roots()
        .unwrap()
        .https_or_http()
        .enable_http1()
        .build();
    let http_client = HyperClient::builder(TokioExecutor::new()).build(connector);
    let webcal = WebCalStorage::new(http_client, url, "holidays_nl".parse().unwrap())
        .expect("can create webcal storage");
    let webcal = Arc::from(webcal);
    let fs = Arc::new(VdirStorage::new(path, String::from("ics")));

    let webcal_collection = "holidays_nl";
    let fs_collection = fs
        .create_collection("holidays_nl")
        .await
        .expect("can create fs collection");

    let copied = copy_collection(webcal, webcal_collection, fs, fs_collection.href()).await;

    println!("Copied {copied} items");
}

/// Copies from `source` to `target` and returns the amount of items copied.
async fn copy_collection<I: Item>(
    source_storage: Arc<dyn Storage<I>>,
    source_collection_id: &str,
    target_storage: Arc<dyn Storage<I>>,
    target_collection_href: &str,
) -> usize {
    let mut count = 0;
    for FetchedItem { item, .. } in source_storage
        .get_all_items(source_collection_id)
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
