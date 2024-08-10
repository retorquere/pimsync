// Copyright 2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! An example of some basic usage of the `CalDavClient` type.
//!
//! This example uses no authentication.
//!
//! Usage:
//!
//!     cargo run --example=find_calendars_noauth https://example.com
//!     cargo run --example=find_calendars_noauth $SERVER_URL
use http::Uri;
use hyper_rustls::HttpsConnectorBuilder;
use libdav::auth::Auth;
use libdav::dav::WebDavClient;
use libdav::{names, CalDavClient};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut arguments = std::env::args();
    arguments.next().expect("arg0 must be defined");
    let base_url: Uri = arguments
        .next()
        .expect("Must specify a $1")
        .parse()
        .expect("$1 must be valid URL");

    let https = HttpsConnectorBuilder::new()
        .with_native_roots()
        .expect("native TLS roots should be available")
        .https_or_http()
        .enable_http1()
        .build();
    let auth = Auth::None;
    let webdav = WebDavClient::new(base_url, auth, https);
    let caldav_client = CalDavClient::new_via_bootstrap(webdav).await.unwrap();

    let urls = match caldav_client.find_current_user_principal().await.unwrap() {
        Some(principal) => {
            let home_set = caldav_client
                .find_calendar_home_set(&principal)
                .await
                .unwrap();
            if home_set.is_empty() {
                vec![caldav_client.base_url().clone()]
            } else {
                home_set
            }
        }
        None => vec![caldav_client.base_url().clone()],
    };

    for url in urls {
        let calendars = caldav_client.find_calendars(&url).await.unwrap();

        println!("found {} calendars...", calendars.len());

        for calendar in calendars {
            let name = caldav_client
                .get_property(&calendar.href, &names::DISPLAY_NAME)
                .await
                .unwrap();
            let color = caldav_client
                .get_property(&calendar.href, &names::CALENDAR_COLOUR)
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
}
