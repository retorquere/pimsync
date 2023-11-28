// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::ops::Deref;

use http::{Method, Request};
use hyper::client::connect::Connect;
use hyper::{Body, Uri};
use log::debug;

use crate::builder::{ClientBuilder, NeedsUri};
use crate::common::{bootstrap_client, find_home_set, parse_find_multiple_collections};
use crate::dav::{check_status, DavError, FoundCollection};
use crate::dns::DiscoverableService;
use crate::names;
use crate::xmlutils::quote_href;
use crate::{dav::WebDavClient, BootstrapError};
use crate::{CheckSupportError, FetchedResource};

/// Client to communicate with a carddav server.
///
/// Instances are usually created via a builder, which does discovery of the exact host and context
/// path.
///
/// ```rust,no_run
/// # use libdav::CardDavClient;
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
///     .https_or_http()
///     .enable_http1()
///     .build();
/// let client = CardDavClient::builder()
///     .with_uri(uri)
///     .with_auth(auth)
///     .build(https)
///     .await
///     .unwrap();
/// # })
/// ```
#[derive(Debug)]
pub struct CardDavClient<C>
where
    C: Connect + Clone + Sync + Send + 'static,
{
    /// The `base_url` may be (due to bootstrapping discovery) different to the one provided as input.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6764#section-1>
    dav_client: WebDavClient<C>,
    /// URL of collections that are either address book collections or ordinary collections
    /// that have child or descendant address book collections owned by the principal.
    /// See: <https://www.rfc-editor.org/rfc/rfc6352#section-7.1.1>
    addressbook_home_set: Option<Uri>,
}

impl<C> Deref for CardDavClient<C>
where
    C: Connect + Clone + Sync + Send,
{
    type Target = WebDavClient<C>;

    fn deref(&self) -> &Self::Target {
        &self.dav_client
    }
}

impl<C> ClientBuilder<CardDavClient<C>, crate::builder::Ready>
where
    C: Connect + Clone + Sync + Send,
{
    /// Builds a carddav client
    ///
    /// Determines the carddav server's real host and the context path of the resources for a
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
    pub async fn build(self, connector: C) -> Result<CardDavClient<C>, BootstrapError> {
        let service = CardDavClient::<C>::service(&self.state.uri)?;

        let dav_client =
            bootstrap_client(self.state.uri, self.state.auth, connector, service).await?;
        let addressbook_home_set = find_home_set(&dav_client, &names::ADDRESSBOOK_HOME_SET).await?;

        Ok(CardDavClient {
            dav_client,
            addressbook_home_set,
        })
    }

    /// Create a client without any discovery.
    ///
    /// This constructor is recommended only for situations where DNS-based discovery is
    /// unavailable or undesirable.
    ///
    /// When in doubt, use [`ClientBuilder<CardDavClient>::build`].
    pub fn build_without_discovery(self, connector: C) -> CardDavClient<C> {
        CardDavClient {
            // TODO: it is not possible to override the user pricinpal
            dav_client: WebDavClient::new(self.state.uri, self.state.auth, connector),
            // TODO: Allow specifying this value
            addressbook_home_set: None,
        }
    }
}

impl<C> CardDavClient<C>
where
    C: Connect + Clone + Sync + Send,
{
    /// Creates a new builder. See [`CardDavClient`] and [`ClientBuilder`] for details.
    #[must_use]
    pub fn builder() -> ClientBuilder<Self, NeedsUri> {
        ClientBuilder::new()
    }

    pub fn addressbook_home_set(&self) -> Option<&Uri> {
        self.addressbook_home_set.as_ref()
    }

    // TODO: methods to serialise and deserialise (mostly to cache all discovery data).

    /// Find address book collections under the given `url`.
    ///
    /// It `url` is not specified, this client's address book home set is used instead. If no
    /// address book home set has been found, then the server's context path will be used. When
    /// using a client bootstrapped via automatic discovery, passing `None` will usually yield the
    /// expected results.
    ///
    /// # Errors
    ///
    /// If the HTTP call fails or parsing the XML response fails.
    pub async fn find_addresbooks(
        &self,
        url: Option<&Uri>,
    ) -> Result<Vec<FoundCollection>, DavError> {
        let url = url.unwrap_or(self.addressbook_home_set.as_ref().unwrap_or(&self.base_url));
        // FIXME: DRY: This is almost a copy-paste of the same method from CalDavClient
        let (head, body) = self
            .propfind(
                url,
                &[
                    &names::RESOURCETYPE,
                    &names::GETETAG,
                    &names::SUPPORTED_REPORT_SET,
                ],
                1,
            )
            .await?;
        check_status(head.status)?;

        parse_find_multiple_collections(body, &names::ADDRESSBOOK)
    }

    // TODO: get_addressbook_description ("addressbook-description", "urn:ietf:params:xml:ns:carddav")
    // TODO: DRY: the above methods are super repetitive.
    //       Maybe all these props impl a single trait, so the API could be `get_prop<T>(url)`?

    /// Fetches existing vcard resources.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn get_resources(
        &self,
        addressbook_href: impl AsRef<str>,
        hrefs: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Vec<FetchedResource>, DavError> {
        let mut body = String::from(
            r#"
            <C:addressbook-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav">
                <D:prop>
                    <D:getetag/>
                    <C:address-data/>
                </D:prop>"#,
        );
        for href in hrefs {
            let href = quote_href(href.as_ref().as_bytes());
            body.push_str("<href>");
            body.push_str(&href);
            body.push_str("</href>");
        }
        body.push_str("</C:addressbook-multiget>");

        self.multi_get(addressbook_href.as_ref(), body, &names::ADDRESS_DATA)
            .await
    }

    /// Checks that the given URI advertises carddav support.
    ///
    /// See: <https://www.rfc-editor.org/rfc/rfc6352#section-6.1>
    ///
    /// # Errors
    ///
    /// If there are any network issues or if the server does not explicitly advertise carddav
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
            .any(|part| part.trim() == "addressbook")
        {
            Ok(())
        } else {
            Err(CheckSupportError::NotAdvertised)
        }
    }

    fn service(uri: &Uri) -> Result<DiscoverableService, BootstrapError> {
        let scheme = uri
            .scheme()
            .ok_or(BootstrapError::InvalidUrl("missing scheme"))?;
        match scheme.as_ref() {
            "https" | "carddavs" => Ok(DiscoverableService::CardDavs),
            "http" | "carddav" => Ok(DiscoverableService::CardDav),
            _ => Err(BootstrapError::InvalidUrl("scheme is invalid")),
        }
    }

    /// Create an address book collection.
    ///
    /// # Errors
    ///
    /// Returns an error in case of network errors or if the server returns a failure status code.
    pub async fn create_addressbook(&self, href: impl AsRef<str>) -> Result<(), DavError> {
        // TODO: Can I somehow delegate to this async method without introducing a new await point?
        self.dav_client
            .create_collection(href, &[&names::ADDRESSBOOK])
            .await
    }
}
