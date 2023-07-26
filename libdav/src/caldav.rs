// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::ops::Deref;

use http::{Method, Request};
use hyper::{Body, Uri};
use log::debug;

use crate::builder::{ClientBuilder, NeedsUri};
use crate::common::{common_bootstrap, parse_find_multiple_collections};
use crate::dav::{check_status, DavError, FoundCollection};
use crate::dns::DiscoverableService;
use crate::names::{
    self, CALENDAR, CALENDAR_COLOUR, CALENDAR_DATA, CALENDAR_HOME_SET, GETETAG, RESOURCETYPE,
    SUPPORTED_REPORT_SET,
};
use crate::xmlutils::{check_multistatus, quote_href};
use crate::{dav::WebDavClient, BootstrapError, FindHomeSetError};
use crate::{CheckSupportError, FetchedResource};

/// A client to communicate with a caldav server.
///
/// Instances are created via a builder:
///
/// ```rust,no_run
/// # use libdav::CalDavClient;
/// use http::Uri;
/// use libdav::auth::{Auth, Password};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let uri = Uri::try_from("https://example.com").unwrap();
/// let auth = Auth::Basic {
///     username: String::from("user"),
///     password: Some(Password::from("secret")),
/// };
///
/// let client = CalDavClient::builder()
///     .with_uri(uri)
///     .with_auth(auth)
///     .build()
///     .auto_bootstrap()
///     .await
///     .unwrap();
/// # })
/// ```
///
/// For common cases, [`auto_bootstrap`](Self::auto_bootstrap) should be called on the client to
/// bootstrap it automatically.
#[derive(Debug, Clone)]
pub struct CalDavClient {
    /// The `base_url` may be (due to bootstrapping discovery) different to the one provided as input.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6764#section-1>
    dav_client: WebDavClient,
    /// URL of collections that are either calendar collections or ordinary collections
    /// that have child or descendant calendar collections owned by the principal.
    /// See: <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
    ///
    /// This field is automatically populated by [`auto_bootstrap`][Self::auto_bootstrap].
    pub calendar_home_set: Option<Uri>, // TODO: timeouts
}

impl Deref for CalDavClient {
    type Target = WebDavClient;

    fn deref(&self) -> &Self::Target {
        &self.dav_client
    }
}

impl ClientBuilder<CalDavClient, crate::builder::Ready> {
    /// Return a built client.
    pub fn build(self) -> CalDavClient {
        CalDavClient {
            dav_client: WebDavClient::new(self.state.uri, self.state.auth),
            calendar_home_set: None,
        }
    }
}

impl CalDavClient {
    /// Creates a new builder. See [`CalDavClient`] and [`ClientBuilder`] for details.
    #[must_use]
    pub fn builder() -> ClientBuilder<Self, NeedsUri> {
        ClientBuilder::new()
    }

    // TODO: methods to serialise and deserialise (mostly to cache all discovery data).

    /// Auto-bootstrap a new client.
    ///
    /// Determines the caldav server's real host and the context path of the resources for a
    /// server, following the discovery mechanism described in [rfc6764].
    ///
    /// [rfc6764]: https://www.rfc-editor.org/rfc/rfc6764
    ///
    /// # Errors
    ///
    /// If any of the underlying DNS or HTTP requests fail, or if any of the responses fail to
    /// parse.
    ///
    /// Does not return an error if DNS records as missing, only if they contain invalid data.
    pub async fn auto_bootstrap(mut self) -> Result<Self, BootstrapError> {
        let port = self.default_port()?;
        let service = self.service()?;
        common_bootstrap(&mut self.dav_client, port, service).await?;

        // If obtaining a principal fails, the specification says we should query the user. This
        // tries to use the `base_url` first, since the user might have provided it for a reason.
        let principal_url = self.principal.as_ref().unwrap_or(&self.base_url);
        self.calendar_home_set = self.find_calendar_home_set(principal_url).await?;

        Ok(self)
    }

    /// Queries a server for the calendar home set.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc4791#section-6.2.1>
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    async fn find_calendar_home_set(&self, url: &Uri) -> Result<Option<Uri>, FindHomeSetError> {
        self.find_href_prop_as_uri(url, &CALENDAR_HOME_SET)
            .await
            .map_err(FindHomeSetError)
    }

    /// Find calendars collections under the given `url`.
    ///
    /// It `url` is not specified, this client's calendar home set is used instead. If no calendar
    /// home set has been found, then the server's context path will be used. When using a client
    /// bootstrapped via automatic discovery, passing `None` will usually yield the expected
    /// results.
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn find_calendars(
        &self,
        url: Option<&Uri>,
    ) -> Result<Vec<FoundCollection>, DavError> {
        let url = url.unwrap_or(self.calendar_home_set.as_ref().unwrap_or(&self.base_url));
        let (head, body) = self
            .propfind(url, &[&RESOURCETYPE, &GETETAG, &SUPPORTED_REPORT_SET], 1)
            .await?;
        check_status(head.status)?;

        parse_find_multiple_collections(body, &CALENDAR)
    }

    /// Returns the colour for the calendar at path `href`.
    ///
    /// This is not a formally standardised property, but is relatively widespread.
    ///
    /// # Quirks
    ///
    /// The namespace of the value in the response from the server is ignored. This is a workaround
    /// for an [issue in the `cyrus-imapd` implemenetation][cyrus-issue].
    ///
    /// [cyrus-issue]: https://github.com/cyrusimap/cyrus-imapd/issues/4489
    ///
    /// # Errors
    ///
    /// If the network request fails, or if the response cannot be parsed.
    pub async fn get_calendar_colour(&self, href: &str) -> Result<Option<String>, DavError> {
        let url = self.relative_uri(href)?;

        let (head, body) = self.propfind(&url, &[&CALENDAR_COLOUR], 0).await?;
        check_status(head.status)?;

        let body = std::str::from_utf8(body.as_ref())?;
        let doc = roxmltree::Document::parse(body)?;
        let root = doc.root_element();

        let props = root
            .descendants()
            .filter(|node| {
                // Ignoring namespace as workaround for https://github.com/cyrusimap/cyrus-imapd/issues/4489
                node.tag_name().name() == "calendar-color"
            })
            .collect::<Vec<_>>();

        if props.len() == 1 {
            return Ok(props[0].text().map(str::to_string));
        }

        check_multistatus(root)?;

        Err(DavError::InvalidResponse(
            "missing property in response with no error".into(),
        ))
    }

    /// Sets the `colour` for a collection
    ///
    /// The `colour` string should be an unescaped hex value with a leading pound sign (e.g.
    /// `#ff0000`).
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn set_calendar_colour(
        &self,
        href: &str,
        colour: Option<&str>,
    ) -> Result<(), DavError> {
        let url = self.relative_uri(href)?;
        self.propupdate(&url, &CALENDAR_COLOUR, colour).await
    }

    // TODO: get_calendar_description ("calendar-description", "urn:ietf:params:xml:ns:caldav")
    // TODO: get_calendar_order ("calendar-order", "http://apple.com/ns/ical/")
    // TODO: DRY: the above methods are super repetitive.
    //       Maybe all these props impl a single trait, so the API could be `get_prop<T>(url)`?

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
    pub async fn get_resources<S1, S2>(
        &self,
        calendar_href: S1,
        hrefs: &[S2],
    ) -> Result<Vec<FetchedResource>, DavError>
    where
        S1: AsRef<str>,
        S2: AsRef<str>,
    {
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

        self.multi_get(calendar_href.as_ref(), body, &CALENDAR_DATA)
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
        let request = Request::builder()
            .method(Method::OPTIONS)
            .uri(url)
            .body(Body::empty())?;

        let (head, _body) = self.request(request).await?;
        check_status(head.status)?;

        let header = head
            .headers
            .get("DAV")
            .ok_or(CheckSupportError::MissingHeader)?
            .to_str()?;

        debug!("DAV header: '{}'", header);
        if header
            .split(|c| c == ',')
            .any(|part| part.trim() == "calendar-access")
        {
            Ok(())
        } else {
            Err(CheckSupportError::NotAdvertised)
        }
    }

    /// Returns the default port to try and use.
    ///
    /// If the `base_url` has an explicit port, that value is returned. Otherwise,
    /// returns `443` for https, `80` for http, and `443` as a fallback for
    /// anything else.
    #[inline]
    fn default_port(&self) -> Result<u16, BootstrapError> {
        if let Some(port) = self.base_url.port_u16() {
            Ok(port)
        } else {
            match self.base_url.scheme() {
                Some(scheme) if scheme == "https" => Ok(443),
                Some(scheme) if scheme == "http" => Ok(80),
                Some(scheme) if scheme == "caldavs" => Ok(443),
                Some(scheme) if scheme == "caldav" => Ok(80),
                _ => Err(BootstrapError::InvalidUrl("invalid scheme (and no port)")),
            }
        }
    }

    fn service(&self) -> Result<DiscoverableService, BootstrapError> {
        let scheme = self
            .base_url
            .scheme()
            .ok_or(BootstrapError::InvalidUrl("missing scheme"))?;
        match scheme.as_ref() {
            "https" | "caldavs" => Ok(DiscoverableService::CalDavs),
            "http" | "caldav" => Ok(DiscoverableService::CalDav),
            _ => Err(BootstrapError::InvalidUrl("scheme is invalid")),
        }
    }

    /// Create an calendar collection.
    ///
    /// # Errors
    ///
    /// Returns an error in case of network errors or if the server returns a failure status code.
    pub async fn create_calendar<Href: AsRef<str>>(&self, href: Href) -> Result<(), DavError> {
        // TODO: Can I somehow delegate to this async method without introducing a new await point?
        self.dav_client
            .create_collection(href, &[&names::CALENDAR])
            .await
    }
}
