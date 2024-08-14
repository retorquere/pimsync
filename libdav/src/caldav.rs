// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::ops::Deref;

use hyper::client::connect::Connect;
use hyper::Uri;

use crate::common::{check_support, parse_find_multiple_collections};
use crate::dav::WebDavClient;
use crate::dav::{check_status, FoundCollection, WebDavError};
use crate::sd::{find_context_url, BootstrapError, DiscoverableService};
use crate::xmlutils::quote_href;
use crate::{names, FindHomeSetError, InvalidUrl};
use crate::{CheckSupportError, FetchedResource};

/// Client to communicate with a caldav server.
///
/// Instances are usually created via [`CalDavClient::new`].
///
/// ```rust
/// # use libdav::CalDavClient;
/// # use libdav::dav::WebDavClient;
/// use http::Uri;
/// use libdav::auth::{Auth, Password};
/// use hyper_rustls::HttpsConnectorBuilder;
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let uri = Uri::try_from("https://example.com").unwrap();
/// let auth = Auth::Basic {
///     username: String::from("user"),
///     password: Some(Password::from("secret")),
/// };
///
/// let https = HttpsConnectorBuilder::new()
///     .with_native_roots()
///     .unwrap()
///     .https_or_http()
///     .enable_http1()
///     .build();
/// let webdav = WebDavClient::new(uri, auth, https);
/// // Optionally, perform bootstrap sequence here.
/// let client = CalDavClient::new(webdav);
/// # })
/// ```
///
/// If the real CalDav server needs to be resolved via bootstrapping, see
/// [`find_context_url`].
#[derive(Debug, Clone)]
pub struct CalDavClient<C>
where
    C: Connect + Clone + Sync + Send + 'static,
{
    /// A WebDav client used to send requests.
    pub webdav_client: WebDavClient<C>,
}

impl<C> Deref for CalDavClient<C>
where
    C: Connect + Clone + Sync + Send,
{
    type Target = WebDavClient<C>;

    fn deref(&self) -> &Self::Target {
        &self.webdav_client
    }
}

impl<C> CalDavClient<C>
where
    C: Connect + Clone + Sync + Send,
{
    /// Create a new client instance.
    pub fn new(webdav_client: WebDavClient<C>) -> CalDavClient<C> {
        CalDavClient { webdav_client }
    }

    /// Create a new client instance.
    ///
    /// Creates a new client, with its `base_url` set to the context path automatically discovered
    /// via [`find_context_url`].
    ///
    /// # Errors
    ///
    /// Returns an error if:
    ///
    /// - The URL has an invalid schema.
    /// - The underlying call to [`find_context_url`] returns an error.
    pub async fn new_via_bootstrap(
        mut webdav_client: WebDavClient<C>,
    ) -> Result<CalDavClient<C>, BootstrapError> {
        let service = service_for_url(&webdav_client.base_url)?;
        if let Some(context_path) = find_context_url(&webdav_client, service).await? {
            webdav_client.base_url = context_path;
        }
        Ok(CalDavClient { webdav_client })
    }

    /// Queries the server for the calendar home set.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn find_calendar_home_set(
        &self,
        principal: &Uri,
    ) -> Result<Vec<Uri>, FindHomeSetError>
    where
        C: Connect + Clone + Sync + Send,
    {
        // If obtaining a principal fails, the specification says we should query the user. This
        // tries to use the `base_url` first, since the user might have provided it for a reason.
        self.find_hrefs_prop_as_uri(principal, &names::CALENDAR_HOME_SET)
            .await
            .map_err(FindHomeSetError)
    }

    // TODO: methods to serialise and deserialise (mostly to cache all discovery data).

    /// Find calendars collections under the given `url`.
    ///
    /// If `url` is not specified, this client's calendar home set is used instead. If no calendar
    /// home set has been found, then the server's context path will be used. When using a client
    /// bootstrapped via automatic discovery, passing `None` will usually yield the expected
    /// results.
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn find_calendars(
        &self,
        calendar_home_set: &Uri,
    ) -> Result<Vec<FoundCollection>, WebDavError> {
        let props = [
            &names::RESOURCETYPE,
            &names::GETETAG,
            &names::SUPPORTED_REPORT_SET,
        ];
        let (head, body) = self.propfind(calendar_home_set, &props, 1).await?;
        check_status(head.status)?;

        parse_find_multiple_collections(body, &names::CALENDAR)
    }

    // TODO: check link in doc:
    // TODO: same note on carddav.
    /// Fetches existing icalendar resources.
    ///
    /// If the `getetag` property is missing for an item, it will be reported as
    /// [`http::StatusCode::NOT_FOUND`]. This should not be an actual issue with in practice, since
    /// support for `getetag` is mandatory for CalDav implementations.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn get_calendar_resources(
        &self,
        calendar_href: impl AsRef<str>,
        hrefs: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Vec<FetchedResource>, WebDavError> {
        let mut body = String::from(
            r#"
            <C:calendar-multiget xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
                <prop>
                    <getetag/>
                    <C:calendar-data/>
                </prop>"#,
        );
        for href in hrefs {
            let href = quote_href(href.as_ref().as_bytes());
            body.push_str("<href>");
            body.push_str(&href);
            body.push_str("</href>");
        }
        body.push_str("</C:calendar-multiget>");

        self.multi_get(calendar_href.as_ref(), body, &names::CALENDAR_DATA)
            .await
    }

    /// Checks that the given URI advertises caldav support.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc4791#section-5.1>
    ///
    /// # Known Issues
    ///
    /// - This is currently broken on Nextcloud. [Bug report][nextcloud].
    ///
    /// [nextcloud]: https://github.com/nextcloud/server/issues/37374
    ///
    /// # Errors
    ///
    /// If there are any network issues or if the server does not explicitly advertise caldav
    /// support.
    pub async fn check_support(&self, url: &Uri) -> Result<(), CheckSupportError> {
        check_support(&self.webdav_client, url, "calendar-access").await
    }

    /// Create an calendar collection.
    ///
    /// # Errors
    ///
    /// Returns an error in case of network errors or if the server returns a failure status code.
    pub async fn create_calendar(&self, href: impl AsRef<str>) -> Result<(), WebDavError> {
        self.webdav_client
            .create_collection(href, &[&names::CALENDAR])
            .await
    }
}

/// Return the service type based on a URL's scheme.
///
/// # Errors
///
/// If `url` is missing a scheme or has a scheme invalid for CalDav usage.
pub fn service_for_url(url: &Uri) -> Result<DiscoverableService, InvalidUrl> {
    match url.scheme().ok_or(InvalidUrl::MissingScheme)?.as_ref() {
        "https" | "caldavs" => Ok(DiscoverableService::CalDavs),
        "http" | "caldav" => Ok(DiscoverableService::CalDav),
        _ => Err(InvalidUrl::InvalidScheme),
    }
}
