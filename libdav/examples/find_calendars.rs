// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! An example of some basic usage of the `CalDavClient` type.
//!
//! Usage:
//!
//!     cargo run --example=find_calendars https://example.com user@example.com MYPASSWORD
//!     cargo run --example=find_calendars $SERVER_URL         $USERNAME        $PASSWORD
//!
//! Example output (with $1 = "https://fastmail.com"):
//!
//! ```
//! Resolved server URL to: https://d277161.caldav.fastmail.com/dav/calendars
//! found 1 calendars...
//! 📅 name: Some("Calendar"), colour: Some("#3a429c"), path: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/"
//! Href and Etag for components in calendar:
//! - /dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/395b00a0-eebc-40fd-a98e-176a06367c82.ics, "e7577ff2b0924fe8e9a91d3fb2eb9072598bf9fb"
//! ```
use http::Uri;
use hyper_rustls::HttpsConnectorBuilder;
use libdav::auth::Auth;
use libdav::CalDavClient;

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
    let caldav_client = CalDavClient::builder()
        .with_uri(base_url)
        .with_auth(Auth::Basic {
            username,
            password: Some(password),
        })
        .build(https)
        .auto_bootstrap()
        .await
        .unwrap();

    println!("Resolved server URL to: {}", caldav_client.context_path());

    let calendars = caldav_client.find_calendars(None).await.unwrap();

    println!("found {} calendars...", calendars.len());

    for calendar in calendars {
        let name = caldav_client
            .get_collection_displayname(&calendar.href)
            .await
            .unwrap();
        let color = caldav_client
            .get_calendar_colour(&calendar.href)
            .await
            .unwrap();
        println!(
            "📅 name: {name:?}, colour: {color:?}, path: {:?}, etag: {:?}",
            &calendar.href, &calendar.etag
        );
        let items = caldav_client
            .list_resources(&calendar.href)
            .await
            .unwrap()
            .into_iter()
            .filter(|i| !i.details.resource_type.is_collection);
        for item in items {
            println!("   {}, {}", item.href, item.details.etag.unwrap());
        }
    }
}
