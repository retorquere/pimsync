// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::Context;
use http::Uri;
use hyper::client::HttpConnector;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use libdav::{
    auth::Auth, caldav_service_for_url, carddav_service_for_url, dav::WebDavClient,
    sd::find_context_url, CalDavClient, CardDavClient,
};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use std::fs::read_to_string;

mod caldav;
mod carddav;

/// A profile for a test server
///
/// Profiles are expected to be defined in files which specify details for connecting
/// to the server and exceptions to rules for tests (e.g.: expected failures).
#[derive(serde::Deserialize, Debug, Clone)]
struct Profile {
    host: String,
    username: String,
    password: String,
    /// The name of the server implementation.
    server: String,
    /// Whether to perform rfc6764 bootstrap sequence.
    #[serde(default)]
    bootstrap: bool,
}

struct TestData {
    caldav: CalDavClient<HttpsConnector<HttpConnector>>,
    carddav: CardDavClient<HttpsConnector<HttpConnector>>,
    // FIXME: assumes that home set is a single href.
    calendar_home_set: Uri,
    // FIXME: assumes that home set is a single href.
    address_home_set: Uri,
    profile: Profile,
}

impl TestData {
    async fn from_profile(profile: Profile) -> anyhow::Result<Self> {
        let https = HttpsConnectorBuilder::new()
            .with_native_roots()?
            .https_or_http()
            .enable_http1()
            .build();
        let base_url = profile.host.parse::<Uri>()?;

        let auth = Auth::Basic {
            username: profile.username.clone(),
            password: Some(profile.password.clone().into()),
        };

        let (caldav, user_principal) = {
            let mut webdav = WebDavClient::new(base_url.clone(), auth.clone(), https.clone());
            if profile.bootstrap {
                let service = caldav_service_for_url(&base_url)?;
                webdav.base_url = find_context_url(&webdav, service)
                    .await?
                    .context("determining context path via bootstrap sequence")?;
            }
            let user_principal = webdav
                .find_current_user_principal()
                .await?
                .context("finding current user pricinpal")?;

            (CalDavClient::new(webdav), user_principal)
        };
        let calendar_home_set = caldav
            .find_calendar_home_set(&user_principal)
            .await?
            .first()
            .context("no calendar home set found")?
            .clone();

        let (carddav, user_principal) = {
            let mut webdav = WebDavClient::new(base_url.clone(), auth.clone(), https.clone());
            if profile.bootstrap {
                let service = carddav_service_for_url(&base_url)?;
                webdav.base_url = find_context_url(&webdav, service)
                    .await?
                    .context("determining context path via bootstrap sequence")?;
            }
            let user_principal = webdav
                .find_current_user_principal()
                .await?
                .context("finding current user pricinpal")?;

            (CardDavClient::new(webdav), user_principal)
        };
        let address_home_set = carddav
            .find_address_book_home_set(&user_principal)
            .await?
            .first()
            .context("no calendar home set found")?
            .clone();

        Ok(TestData {
            caldav,
            carddav,
            calendar_home_set,
            address_home_set,
            profile,
        })
    }

    async fn calendar_count(&self) -> anyhow::Result<usize> {
        self.caldav
            .find_calendars(&self.calendar_home_set)
            .await
            .map(|calendars| calendars.len())
            .context("fetch calendar count")
    }

    async fn addressbook_count(&self) -> anyhow::Result<usize> {
        self.carddav
            .find_addressbooks(&self.address_home_set)
            .await
            .map(|a| a.len())
            .context("fetching addressbook count")
    }
}

fn process_result(
    test_data: &TestData,
    test_name: &str,
    result: &anyhow::Result<()>,
    total: &mut u32,
    passed: &mut u32,
) {
    print!("- {test_name}: ");
    if let Some(expected_failure) = EXPECTED_FAILURES
        .iter()
        .find(|x| x.server == test_data.profile.server.as_str() && x.test == test_name)
    {
        if result.is_ok() {
            println!("⛔ expected failure but passed");
        } else {
            println!("⚠️ expected failure: {}", expected_failure.reason);
            *passed += 1;
        }
    } else if let Err(err) = &result {
        println!("⛔ failed: {err:?}");
    } else {
        println!("✅ passed");
        *passed += 1;
    };
    *total += 1;
}

macro_rules! run_tests {
    ($test_data:expr, $($test:expr,)*) => {
        {
            let mut total = 0;
            let mut passed = 0;
            $(
                let name = stringify!($test);
                let result = $test($test_data).await;
                process_result($test_data, name, &result, &mut total, &mut passed);
            )*
            (total, passed)
        }
    };
}

struct ExpectedFailure {
    server: &'static str,
    test: &'static str,
    reason: &'static str,
}

/// A list of tests that are known to fail on specific servers.
///
/// An `xfail` proc macro would be nice, but it seems like an overkill for just a single project.
const EXPECTED_FAILURES: &[ExpectedFailure] = &[
    // Baikal
    ExpectedFailure {
        server: "baikal",
        test: "caldav::test_create_and_delete_collection",
        reason: "https://github.com/sabre-io/Baikal/issues/1182",
    },
    ExpectedFailure {
        server: "baikal",
        test: "carddav::test_create_and_delete_addressbook",
        reason: "https://github.com/sabre-io/Baikal/issues/1182",
    },
    // Cyrus-IMAP
    ExpectedFailure {
        server: "cyrus-imap",
        test: "caldav::test_create_and_delete_collection",
        reason: "precondition failed (unreported)",
    },
    ExpectedFailure {
        server: "cyrus-imap",
        test: "carddav::test_create_and_delete_addressbook",
        reason: "precondition failed (unreported)",
    },
    ExpectedFailure {
        server: "cyrus-imap",
        test: "caldav::test_check_caldav_support",
        reason: "server does not adviertise caldav support (unreported)",
    },
    ExpectedFailure {
        server: "cyrus-imap",
        test: "carddav::test_check_carddav_support",
        reason: "server does not adviertise caldav support (unreported)",
    },
    // Nextcloud
    ExpectedFailure {
        server: "nextcloud",
        test: "caldav::test_create_and_delete_collection",
        reason: "server does not return etags (unreported)",
    },
    ExpectedFailure {
        server: "nextcloud",
        test: "carddav::test_create_and_delete_addressbook",
        reason: "server does not return etags (unreported)",
    },
    ExpectedFailure {
        server: "nextcloud",
        test: "caldav::test_check_caldav_support",
        reason: "https://github.com/nextcloud/server/issues/37374",
    },
    ExpectedFailure {
        server: "nextcloud",
        test: "carddav::test_check_carddav_support",
        reason: "server does not adviertise caldav support (unreported)",
    },
    // Xandikos
    ExpectedFailure {
        server: "xandikos",
        test: "caldav::test_create_and_fetch_resource_with_weird_characters",
        reason: "https://github.com/jelmer/xandikos/issues/253",
    },
];

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    simple_logger::init_with_level(log::Level::Error).expect("logger configuration is valid");

    let mut args = std::env::args_os();
    let cmd = args.next().expect("Argument zero must be defined");
    let profile_path = args
        .next()
        .context(format!("Usage: {} PROFILE", cmd.to_string_lossy()))?;

    println!("🗓️ Running tests for: {}", profile_path.to_string_lossy());
    let raw_profile = read_to_string(profile_path).context("reading config profile")?;
    let profile = toml::de::from_str::<Profile>(&raw_profile)?;
    let test_data = TestData::from_profile(profile).await?;

    let (total, passed) = run_tests!(
        &test_data,
        caldav::test_get_properties,
        caldav::test_create_and_delete_collection,
        caldav::test_create_and_force_delete_collection,
        caldav::test_setting_and_getting_displayname,
        caldav::test_setting_and_getting_colour,
        caldav::test_create_and_delete_resource,
        caldav::test_create_and_fetch_resource,
        caldav::test_create_and_fetch_resource_with_weird_characters,
        caldav::test_create_and_fetch_resource_with_non_ascii_data,
        caldav::test_fetch_missing,
        caldav::test_check_caldav_support,
        carddav::test_setting_and_getting_addressbook_displayname,
        carddav::test_check_carddav_support,
        carddav::test_create_and_delete_addressbook,
        carddav::test_create_and_delete_resource,
    );

    if passed < total {
        println!("⛔ {passed}/{total} tests passed.\n");
        std::process::exit(1);
    } else {
        println!("✅ {total} tests passed.\n");
    }

    Ok(())
}

fn random_string(len: usize) -> String {
    thread_rng()
        .sample_iter(Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}
