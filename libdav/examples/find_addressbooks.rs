// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! An example of some basic usage of the `CardDavClient` type.
//!
//! Usage:
//!
//!     cargo run --example=find_addressbooks https://example.com user@example.com MYPASSWORD
//!     cargo run --example=find_addressbooks $SERVER_URL         $USERNAME        $PASSWORD
//!
//! Example output (with $1 = "https://fastmail.com"):
//!
//! ```
//! Resolved server URL to: https://d277161.carddav.fastmail.com/dav/addressbooks
//! found 2 addressbooks...
//! 📇 name: Some("Personal"), path: "/dav/addressbooks/user/vdirsyncer@fastmail.com/Default/"
//! Href and Etag for components in addressbook:
//! 📇 name: Some("test-vdirsyncer-ci-bedd62c5-ede3-4e07-87c0-163c259c634f"), path: "/dav/addressbooks/user/vdirsyncer@fastmail.com/test-vdirsyncer-ci-bedd62c5-ede3-4e07-87c0-163c259c634f/"
//! Href and Etag for components in addressbook:
//! ```
use http::Uri;
use hyper_rustls::HttpsConnectorBuilder;
use libdav::auth::Auth;
use libdav::CardDavClient;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut arguments = std::env::args();
    arguments
        .next()
        .expect("binary has been called with a name");
    let base_url: Uri = arguments
        .next()
        .expect("$1 is defined")
        .parse()
        .expect("$1 is a valid URL");
    let username = arguments.next().expect("$2 is a valid username");
    let password = arguments.next().expect("$3 is a valid password").into();

    let https = HttpsConnectorBuilder::new()
        .with_native_roots()
        .https_or_http()
        .enable_http1()
        .build();
    let carddav_client = CardDavClient::builder()
        .with_uri(base_url)
        .with_auth(Auth::Basic {
            username,
            password: Some(password),
        })
        .bootstrap(https)
        .await
        .unwrap()
        .build();

    println!("Resolved server URL to: {}", carddav_client.base_url());

    let addressbooks = carddav_client.find_addressbooks(None).await.unwrap();

    println!("found {} addressbooks...", addressbooks.len());

    for ref addressbook in addressbooks {
        let name = carddav_client
            .get_collection_displayname(&addressbook.href)
            .await
            .unwrap();
        println!(
            "📇 name: {name:?}, path: {:?}, etag: {:?}",
            &addressbook.href, &addressbook.etag
        );
        let items = carddav_client
            .list_resources(&addressbook.href)
            .await
            .unwrap()
            .into_iter()
            .filter(|i| !i.details.resource_type.is_collection);
        for item in items {
            println!("   {}, {}", item.href, item.details.etag.unwrap());
        }
    }
}
