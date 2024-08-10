// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Generic webdav implementation.
//!
//! This mostly implements the necessary bits for the caldav and carddav implementations. It should
//! not be considered a general purpose webdav implementation.
use std::{str::FromStr, string::FromUtf8Error};

use http::{
    response::Parts, status::InvalidStatusCode, uri::PathAndQuery, Method, Request, StatusCode, Uri,
};
use hyper::{body::Bytes, client::connect::Connect, Body, Client};
use percent_encoding::percent_decode_str;

use crate::{
    names,
    sd::DiscoverableService,
    xmlutils::{
        check_multistatus, get_newline_corrected_text, get_unquoted_href, quote_href, render_xml,
        render_xml_with_text,
    },
    Auth, FetchedResource, FetchedResourceContent, ItemDetails, PropertyName, ResourceType,
};

#[derive(thiserror::Error, Debug)]
pub enum RequestError {
    #[error("error executing http request: {0}")]
    Http(#[from] hyper::Error),

    #[error("error resolving authentication: {0}")]
    BadAuth(#[from] std::io::Error),
}

/// A generic error for WebDav operations.
#[derive(thiserror::Error, Debug)]
#[allow(clippy::module_name_repetitions)]
pub enum WebDavError {
    #[error("error executing http request: {0}")]
    Http(#[from] hyper::Error),

    #[error("error resolving authentication: {0}")]
    BadAuth(#[from] std::io::Error),

    #[error("missing field '{0}' in response XML")]
    MissingData(&'static str),

    #[error("invalid status code in response: {0}")]
    InvalidStatusCode(#[from] InvalidStatusCode),

    #[error("could not parse XML response: {0}")]
    Xml(#[from] roxmltree::Error),

    #[error("http request returned {0}")]
    BadStatusCode(http::StatusCode),

    #[error("failed to build URL with the given input: {0}")]
    InvalidInput(#[from] http::Error),

    #[error("the server returned an response with an invalid etag header: {0}")]
    InvalidEtag(#[from] FromUtf8Error),

    #[error("the server returned an invalid response: {0}")]
    InvalidResponse(Box<dyn std::error::Error + Send + Sync>),

    #[error("could not decode response as utf-8: {0}")]
    NotUtf8(#[from] std::str::Utf8Error),
}

impl From<StatusCode> for WebDavError {
    fn from(status: StatusCode) -> Self {
        WebDavError::BadStatusCode(status)
    }
}

impl From<RequestError> for WebDavError {
    fn from(value: RequestError) -> Self {
        match value {
            RequestError::Http(err) => WebDavError::Http(err),
            RequestError::BadAuth(err) => WebDavError::BadAuth(err),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ResolveContextPathError {
    #[error("failed to create uri and request with given parameters: {0}")]
    BadInput(#[from] http::Error),

    #[error("error performing http request: {0}")]
    Request(#[from] RequestError),

    #[error("missing Location header in response")]
    MissingLocation,

    #[error("error building new Uri with Location from response: {0}")]
    BadLocation(#[from] http::uri::InvalidUri),
}

#[derive(thiserror::Error, Debug)]
pub enum FindCurrentUserPrincipalError {
    #[error("error performing http request: {0}")]
    RequestError(#[from] WebDavError),

    // XXX: This should not really happen, but the API for `http` won't let us validate this
    // earlier with a clear approach.
    #[error("cannot use base_url to build request uri: {0}")]
    InvalidInput(#[from] http::Error),
}

/// A generic webdav client.
#[derive(Debug, Clone)]
pub struct WebDavClient<C>
where
    C: Connect + Clone + Sync + Send + 'static,
{
    /// Base URL to be used for all requests.
    ///
    /// This is composed of the domain+port used for the server, plus the context path where Dav
    /// requests are served.
    pub base_url: Uri,
    auth: Auth,
    http_client: Client<C>,
}

impl<C> WebDavClient<C>
where
    C: Connect + Clone + Sync + Send,
{
    /// Builds a new webdav client.
    ///
    /// Only `https` is enabled by default. Plain-text `http` is only enabled if the
    /// input uri has a scheme of `http` or `caldav`.
    pub fn new(base_url: Uri, auth: Auth, connector: C) -> WebDavClient<C> {
        WebDavClient {
            base_url,
            auth,
            http_client: Client::builder().build(connector),
        }
    }

    /// Returns a URL pointing to the server's context path.
    pub fn base_url(&self) -> &Uri {
        &self.base_url
    }

    /// Returns a new URI relative to the server's root.
    ///
    /// # Errors
    ///
    /// If this client's `base_url` is invalid or the provided `path` is not an acceptable path.
    // TODO: document the exact error variants in each situation.
    pub fn relative_uri(&self, path: impl AsRef<str>) -> Result<Uri, http::Error> {
        let href = quote_href(path.as_ref().as_bytes());
        let mut parts = self.base_url.clone().into_parts();
        parts.path_and_query = Some(PathAndQuery::try_from(href.as_ref())?);
        Uri::from_parts(parts).map_err(http::Error::from)
    }

    /// Resolves the current user's principal resource.
    ///
    /// Returns `None` if the response's status code is 404 or if no principal was found.
    ///
    /// # Errors
    ///
    /// - If the underlying HTTP request fails.
    /// - If the response status code is neither success nor 404.
    /// - If parsing the XML response fails.
    /// - If the `href` cannot be parsed into a valid [`Uri`]
    ///
    /// # See also
    ///
    /// - <https://www.rfc-editor.org/rfc/rfc5397#section-3>
    pub async fn find_current_user_principal(
        &self,
    ) -> Result<Option<Uri>, FindCurrentUserPrincipalError> {
        // Try querying the provided base url...
        let maybe_principal = self
            .find_href_prop_as_uri(&self.base_url, &names::CURRENT_USER_PRINCIPAL)
            .await;

        match maybe_principal {
            Err(WebDavError::BadStatusCode(StatusCode::NOT_FOUND)) | Ok(None) => {}
            Err(err) => return Err(FindCurrentUserPrincipalError::RequestError(err)),
            Ok(Some(p)) => return Ok(Some(p)),
        };

        // ... Otherwise, try querying the root path.
        let root = self.relative_uri("/")?;
        self.find_href_prop_as_uri(&root, &names::CURRENT_USER_PRINCIPAL)
            .await
            .map_err(FindCurrentUserPrincipalError::RequestError)

        // NOTE: If no principal is resolved, it needs to be provided interactively
        //       by the user. We use `base_url` as a fallback.
    }

    /// Internal helper to find an `href` property
    ///
    /// Very specific, but de-duplicates a few identical functions.
    pub(crate) async fn find_href_prop_as_uri(
        &self,
        url: &Uri,
        property: &PropertyName<'_, '_>,
    ) -> Result<Option<Uri>, WebDavError> {
        let (head, body) = self.propfind(url, &[property], 0).await?;
        check_status(head.status)?;

        parse_prop_href(body, url, property)
    }

    /// Internal helper to find multiple `href` properties.
    ///
    /// Very specific, but de-duplicates a few identical functions.
    pub(crate) async fn find_hrefs_prop_as_uri(
        &self,
        url: &Uri,
        property: &PropertyName<'_, '_>,
    ) -> Result<Vec<Uri>, WebDavError> {
        let (head, body) = self.propfind(url, &[property], 0).await?;
        check_status(head.status)?;

        let body = body;
        let body = std::str::from_utf8(body.as_ref())?;
        let doc = roxmltree::Document::parse(body)?;
        let root = doc.root_element();

        let props = root
            .descendants()
            .filter(|node| node.tag_name() == *property)
            .collect::<Vec<_>>();

        if props.len() == 1 {
            let mut hrefs = Vec::new();

            let href_nodes = props[0]
                .children()
                .filter(|node| node.tag_name() == names::HREF);

            for href_node in href_nodes {
                let maybe_href = href_node
                    .text()
                    .map(|raw| percent_decode_str(raw).decode_utf8())
                    .transpose()?;
                let Some(href) = maybe_href else {
                    continue;
                };
                let path = PathAndQuery::from_str(&href)
                    .map_err(|e| WebDavError::InvalidResponse(Box::from(e)))?;

                let mut parts = url.clone().into_parts();
                parts.path_and_query = Some(path);
                let href = (Uri::from_parts(parts))
                    .map_err(|e| WebDavError::InvalidResponse(Box::from(e)))?;
                hrefs.push(href);
            }

            return Ok(hrefs);
        }

        check_multistatus(root)?;

        Err(WebDavError::InvalidResponse(
            "missing property in response but no error".into(),
        ))
    }

    /// Sends a `PROPFIND` request.
    ///
    /// This is a shortcut for simple `PROPFIND` requests.
    ///
    /// # Errors
    ///
    /// If there are any network errors.
    pub async fn propfind(
        &self,
        url: &Uri,
        properties: &[&PropertyName<'_, '_>],
        depth: u8,
    ) -> Result<(Parts, Bytes), WebDavError> {
        let mut body = String::from(r#"<propfind xmlns="DAV:"><prop>"#);
        for prop in properties {
            body.push_str(&render_xml(prop));
        }
        body.push_str("</prop></propfind>");

        let request = Request::builder()
            .method("PROPFIND")
            .uri(url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("Depth", depth.to_string())
            .body(Body::from(body))?;

        self.request(request).await.map_err(WebDavError::from)
    }

    /// Send a request to the server.
    ///
    /// Sends a request, applying any necessary authentication and logging the response.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying http request fails or if streaming the response fails.
    pub async fn request(&self, request: Request<Body>) -> Result<(Parts, Bytes), RequestError> {
        // QUIRK: When trying to fetch a resource on a URL that is a collection, iCloud
        // will terminate the connection (which returns "unexpected end of file").

        let response = self.http_client.request(self.auth.apply(request)?).await?;
        let (head, body) = response.into_parts();
        let body = hyper::body::to_bytes(body).await?;

        log::trace!("Response ({}): {:?}", head.status, body);
        Ok((head, body))
    }

    /// Fetch a single property.
    ///
    /// # Common properties
    ///
    /// - [`names::ADDRESSBOOK_DESCRIPTION`]
    /// - [`names::CALENDAR_COLOUR`]
    /// - [`names::CALENDAR_DESCRIPTION`]
    /// - [`names::CALENDAR_ORDER`]
    /// - [`names::DISPLAY_NAME`]
    ///
    /// # Quirks
    ///
    /// The namespace of the value in the response from the server is ignored. This is a workaround
    /// for an [issue in `cyrus-imapd`][cyrus-issue].
    ///
    /// [cyrus-issue]: https://github.com/cyrusimap/cyrus-imapd/issues/4489
    ///
    /// # Errors
    ///
    /// - If there are any network errors or the response could not be parsed.
    /// - If the requested property is missing in the response.
    ///
    /// # See also
    ///
    /// - [`WebDavClient::set_property`]
    pub async fn get_property(
        &self,
        href: &str,
        property: &PropertyName<'_, '_>,
    ) -> Result<Option<String>, WebDavError> {
        let url = self.relative_uri(href)?;

        let (head, body) = self.propfind(&url, &[property], 0).await?;
        check_status(head.status)?;

        parse_prop(body, property)
    }

    /// Fetch multiple properties for a single resource.
    ///
    /// Values in the returned `Vec` are in the same order as the `properties` parameter.
    ///
    /// # Quirks
    ///
    /// Same as [`WebDavClient::get_property`].
    ///
    /// # Errors
    ///
    /// - If there are any network errors or the response could not be parsed.
    /// - If the requested property is missing in the response.
    ///
    /// # See also
    ///
    /// - [`WebDavClient::get_property`]
    /// - [`WebDavClient::set_property`]
    pub async fn get_properties<'p>(
        &self,
        href: &str,
        properties: &[&PropertyName<'p, 'p>],
    ) -> Result<Vec<(PropertyName<'p, 'p>, Option<String>)>, WebDavError> {
        let url = self.relative_uri(href)?;

        let (head, body) = self.propfind(&url, properties, 0).await?;
        check_status(head.status)?;

        let body = std::str::from_utf8(body.as_ref())?;
        let doc = roxmltree::Document::parse(body)?;
        let root = doc.root_element();

        let mut results = Vec::with_capacity(properties.len());
        for property in properties {
            let prop = root
                .descendants()
                .find(|node| node.tag_name() == **property)
                // Hack to work around: https://github.com/cyrusimap/cyrus-imapd/issues/4489
                .or_else(|| {
                    root.descendants()
                        .find(|node| node.tag_name().name() == property.name())
                })
                // End hack
                .and_then(|p| p.text())
                .map(str::to_owned);

            results.push((**property, prop));
        }
        Ok(results)
    }

    /// Sends a `PROPUPDATE` query to the server.
    ///
    /// Setting the value to `None` will remove the property. Returns the new value as returned by
    /// the server.
    ///
    /// # Quirks
    ///
    /// Same as [`WebDavClient::get_property`].
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    ///
    /// # See also
    ///
    /// - [`WebDavClient::get_property`] (contains a list of some included well-known properties)
    // TODO: document whether the value needs to be escaped or not.
    pub async fn set_property(
        &self,
        href: &str,
        property: &PropertyName<'_, '_>,
        value: Option<&str>,
    ) -> Result<Option<String>, WebDavError> {
        let url = self.relative_uri(href)?;
        let action = match value {
            Some(_) => "set",
            None => "remove",
        };
        let inner = render_xml_with_text(property, value);
        let request = Request::builder()
            .method("PROPPATCH")
            .uri(url)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(Body::from(format!(
                r#"<propertyupdate xmlns="DAV:">
                <{action}>
                    <prop>
                        {inner}
                    </prop>
                </{action}>
            </propertyupdate>"#
            )))?;

        let (head, body) = self.request(request).await?;
        check_status(head.status)?;

        parse_prop(body, property)
    }

    /// Resolve the default context path using a well-known path.
    ///
    /// This only applies for servers supporting webdav extensions like caldav or carddav. Returns
    /// `Ok(None)` if the well-known path does not redirect to another location.
    ///
    /// # Errors
    ///
    /// - If the provided scheme, host and port cannot be used to construct a valid URL.
    /// - If there are any network errors.
    /// - If the response is not an HTTP redirection.
    /// - If the `Location` header in the response is missing or invalid.
    ///
    /// # See also
    ///
    /// - <https://www.rfc-editor.org/rfc/rfc6764#section-5>
    /// - [`ResolveContextPathError`]
    #[allow(clippy::missing_panics_doc)] // panic condition is unreachable.
    pub async fn find_context_path(
        &self,
        service: DiscoverableService,
        host: &str,
        port: u16,
    ) -> Result<Option<Uri>, ResolveContextPathError> {
        let uri = Uri::builder()
            .scheme(service.scheme())
            .authority(format!("{host}:{port}"))
            .path_and_query(service.well_known_path())
            .build()?;

        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::default())?;

        // From https://www.rfc-editor.org/rfc/rfc6764#section-5:
        // > [...] the server MAY require authentication when a client tries to
        // > access the ".well-known" URI
        let (head, _body) = self.request(request).await?;
        log::debug!("Response finding context path: {}", head.status);

        if !head.status.is_redirection() {
            return Ok(None);
        }

        // TODO: multiple redirections...?
        let location = head
            .headers
            .get(hyper::header::LOCATION)
            .ok_or(ResolveContextPathError::MissingLocation)?
            .as_bytes();
        let uri = Uri::try_from(location)?;

        if uri.host().is_some() {
            return Ok(Some(uri)); // Uri is absolute.
        }

        let mut parts = uri.into_parts();
        if parts.scheme.is_none() {
            parts.scheme = Some(service.scheme());
        }
        if parts.authority.is_none() {
            parts.authority = Some(format!("{host}:{port}").try_into()?);
        }

        let uri = Uri::from_parts(parts).expect("uri parts are already validated");
        Ok(Some(uri))
    }

    /// Enumerates resources in a collection
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn list_resources(
        &self,
        collection_href: &str,
    ) -> Result<Vec<ListedResource>, WebDavError> {
        let url = self.relative_uri(collection_href)?;

        let (head, body) = self
            .propfind(
                &url,
                &[
                    &names::RESOURCETYPE,
                    &names::GETCONTENTTYPE,
                    &names::GETETAG,
                ],
                1,
            )
            .await?;
        check_status(head.status)?;

        list_resources_parse(body, collection_href)
    }

    /// Inner helper with common logic between `create` and `update`.
    async fn put(
        &self,
        href: impl AsRef<str>,
        data: Vec<u8>,
        etag: Option<impl AsRef<str>>,
        mime_type: impl AsRef<[u8]>,
    ) -> Result<Option<String>, WebDavError> {
        let mut builder = Request::builder()
            .method(Method::PUT)
            .uri(self.relative_uri(href)?)
            .header("Content-Type", mime_type.as_ref());

        builder = match etag {
            Some(etag) => builder.header("If-Match", etag.as_ref()),
            None => builder.header("If-None-Match", "*"),
        };

        let request = builder.body(Body::from(data))?;

        let (head, _body) = self.request(request).await?;
        check_status(head.status)?;

        // TODO: check multi-response

        let new_etag = head
            .headers
            .get("etag")
            .map(|hv| String::from_utf8(hv.as_bytes().to_vec()))
            .transpose()?;
        Ok(new_etag)
    }

    /// Creates a new resource
    ///
    /// Returns an `Etag` if present in the response. If the `Etag` is not included, it must be
    /// requested in a follow-up request, and cannot be obtained race-free.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn create_resource(
        &self,
        href: impl AsRef<str>,
        data: Vec<u8>,
        mime_type: impl AsRef<[u8]>,
    ) -> Result<Option<String>, WebDavError> {
        self.put(href, data, Option::<&str>::None, mime_type).await
    }

    /// Updates an existing resource
    ///
    /// Returns an `Etag` if present in the response. If the `Etag` is not included, it must be
    /// requested in a follow-up request, and cannot be obtained race-free.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn update_resource(
        &self,
        href: impl AsRef<str>,
        data: Vec<u8>,
        etag: impl AsRef<str>,
        mime_type: impl AsRef<[u8]>,
    ) -> Result<Option<String>, WebDavError> {
        self.put(href, data, Some(etag.as_ref()), mime_type).await
    }

    /// Creates a collection under path `href`.
    ///
    /// This function executes an [Extended MKCOL](https://www.rfc-editor.org/rfc/rfc5689).
    ///
    /// Additional resource types may be specified via the `resourcetypes` argument. The
    /// `DAV:collection` resource type is implied and MUST NOT be specified.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn create_collection(
        &self,
        href: impl AsRef<str>,
        resourcetypes: &[&PropertyName<'_, '_>],
    ) -> Result<(), WebDavError> {
        let mut rendered_resource_types = String::new();
        for resource_type in resourcetypes {
            rendered_resource_types.push_str(&render_xml(resource_type));
        }

        let body = format!(
            r#"
            <mkcol xmlns="DAV:">
                <set>
                    <prop>
                        <resourcetype>
                            <collection/>
                            {rendered_resource_types}
                        </resourcetype>
                    </prop>
                </set>
            </mkcol>"#
        );

        let request = Request::builder()
            .method("MKCOL")
            .uri(self.relative_uri(href.as_ref())?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(Body::from(body))?;

        let (head, _body) = self.request(request).await?;
        // TODO: we should check the response body here, if present.
        // Some servers (e.g.: Fastmail) return an empty body.
        check_status(head.status)?;

        Ok(())
    }

    /// Deletes the resource at `href`.
    ///
    /// The resource MAY be a collection. Because the implementation for deleting resources and
    /// collections is identical, this same function is used for both cases.
    ///
    /// If the Etag does not match (i.e.: if the resource has been altered), the operation will
    /// fail and return an Error.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    // TODO: document WHICH error is returned on Etag mismatch.
    pub async fn delete(
        &self,
        href: impl AsRef<str>,
        etag: impl AsRef<str>,
    ) -> Result<(), WebDavError> {
        let request = Request::builder()
            .method(Method::DELETE)
            .uri(self.relative_uri(href.as_ref())?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .header("If-Match", etag.as_ref())
            .body(Body::empty())?;

        let (head, _body) = self.request(request).await?;

        check_status(head.status).map_err(WebDavError::BadStatusCode)
    }

    /// Force deletion of the resource at `href`.
    ///
    /// This function does not guarantee that a resource or collection has not been modified since
    /// it was last read. **Use this function with care**.
    ///
    /// The resource MAY be a collection. Because the implementation for deleting resources and
    /// collections is identical, this same function covers both cases.
    ///
    /// # Errors
    ///
    /// If there are any network errors or the response could not be parsed.
    pub async fn force_delete(&self, href: impl AsRef<str>) -> Result<(), WebDavError> {
        let request = Request::builder()
            .method(Method::DELETE)
            .uri(self.relative_uri(href.as_ref())?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(Body::empty())?;

        let (head, _body) = self.request(request).await?;

        check_status(head.status).map_err(WebDavError::BadStatusCode)
    }

    pub(crate) async fn multi_get(
        &self,
        collection_href: &str,
        body: String,
        property: &PropertyName<'_, '_>,
    ) -> Result<Vec<FetchedResource>, WebDavError> {
        let request = Request::builder()
            .method("REPORT")
            .uri(self.relative_uri(collection_href)?)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(Body::from(body))?;

        let (head, body) = self.request(request).await?;
        check_status(head.status)?;

        multi_get_parse(body, property)
    }
}

#[inline]
pub(crate) fn check_status(status: StatusCode) -> Result<(), StatusCode> {
    if status.is_success() {
        Ok(())
    } else {
        Err(status)
    }
}

pub mod mime_types {
    pub const CALENDAR: &[u8] = b"text/calendar";
    pub const ADDRESSBOOK: &[u8] = b"text/vcard";
}

/// Metadata for a resource.
///
/// This type is returned when listing resources. It contains metadata on
/// resources but no the resource data itself.
#[derive(Debug, PartialEq)]
pub struct ListedResource {
    pub details: ItemDetails,
    /// This value is not URL-encoded.
    pub href: String,
}

/// Metadata for a collection.
///
/// This type is returned when listing collections. It contains metadata on
/// collection itself, but not the entires themselves.
#[derive(Debug)]
pub struct FoundCollection {
    /// This value is not URL-encoded.
    pub href: String,
    pub etag: Option<String>,
    /// From: <https://www.rfc-editor.org/rfc/rfc6578>
    pub supports_sync: bool,
    // TODO: query displayname by default too.
}

pub(crate) fn parse_prop_href(
    body: impl AsRef<[u8]>,
    url: &Uri,
    property: &PropertyName<'_, '_>,
) -> Result<Option<Uri>, WebDavError> {
    let body = std::str::from_utf8(body.as_ref())?;
    let doc = roxmltree::Document::parse(body)?;
    let root = doc.root_element();

    let props = root
        .descendants()
        .filter(|node| node.tag_name() == *property)
        .collect::<Vec<_>>();

    if props.len() == 1 {
        if let Some(href_node) = props[0]
            .children()
            .find(|node| node.tag_name() == names::HREF)
        {
            let maybe_href = href_node
                .text()
                .map(|raw| percent_decode_str(raw).decode_utf8())
                .transpose()?;
            let Some(href) = maybe_href else {
                return Ok(None);
            };
            let path = PathAndQuery::from_str(&href)
                .map_err(|e| WebDavError::InvalidResponse(Box::from(e)))?;

            let mut parts = url.clone().into_parts();
            parts.path_and_query = Some(path);
            return Some(Uri::from_parts(parts))
                .transpose()
                .map_err(|e| WebDavError::InvalidResponse(Box::from(e)));
        }
    }

    check_multistatus(root)?;

    Err(WebDavError::InvalidResponse(
        "missing property in response but no error".into(),
    ))
}

fn parse_prop(
    body: impl AsRef<[u8]>,
    property: &PropertyName<'_, '_>,
) -> Result<Option<String>, WebDavError> {
    let body = std::str::from_utf8(body.as_ref())?;
    let doc = roxmltree::Document::parse(body)?;
    let root = doc.root_element();

    let prop = root
        .descendants()
        .find(|node| node.tag_name() == *property)
        // Hack to work around: https://github.com/cyrusimap/cyrus-imapd/issues/4489
        .or_else(|| {
            root.descendants()
                .find(|node| node.tag_name().name() == property.name())
        });

    if let Some(prop) = prop {
        return Ok(prop.text().map(str::to_string));
    }

    check_multistatus(root)?;

    Err(WebDavError::InvalidResponse(
        "Property is missing from response, but response is non-error.".into(),
    ))
}

fn list_resources_parse(
    body: impl AsRef<[u8]>,
    collection_href: &str,
) -> Result<Vec<ListedResource>, WebDavError> {
    let body = std::str::from_utf8(body.as_ref())?;
    let doc = roxmltree::Document::parse(body)?;
    let root = doc.root_element();
    let responses = root
        .descendants()
        .filter(|node| node.tag_name() == names::RESPONSE);

    let mut items = Vec::new();
    for response in responses {
        let href = get_unquoted_href(&response)?.to_string();

        // Don't list the collection itself.
        // INVARIANT: href has been unquoted. collection_href parameter MUST NOT be URL-encoded.
        if href == collection_href {
            continue;
        }

        let etag = response
            .descendants()
            .find(|node| node.tag_name() == names::GETETAG)
            .and_then(|node| node.text().map(str::to_string));
        let content_type = response
            .descendants()
            .find(|node| node.tag_name() == names::GETCONTENTTYPE)
            .and_then(|node| node.text().map(str::to_string));
        let resource_type = if let Some(r) = response
            .descendants()
            .find(|node| node.tag_name() == names::RESOURCETYPE)
        {
            ResourceType {
                is_calendar: r.descendants().any(|n| n.tag_name() == names::CALENDAR),
                is_collection: r.descendants().any(|n| n.tag_name() == names::COLLECTION),
                is_address_book: r.descendants().any(|n| n.tag_name() == names::ADDRESSBOOK),
            }
        } else {
            ResourceType::default()
        };

        items.push(ListedResource {
            details: ItemDetails {
                content_type,
                etag,
                resource_type,
            },
            href,
        });
    }

    Ok(items)
}

fn multi_get_parse(
    body: impl AsRef<[u8]>,
    property: &PropertyName<'_, '_>,
) -> Result<Vec<FetchedResource>, WebDavError> {
    let body = std::str::from_utf8(body.as_ref())?;
    let doc = roxmltree::Document::parse(body)?;
    let responses = doc
        .root_element()
        .descendants()
        .filter(|node| node.tag_name() == names::RESPONSE);

    let mut items = Vec::new();
    for response in responses {
        let status = match check_multistatus(response) {
            Ok(()) => None,
            Err(WebDavError::BadStatusCode(status)) => Some(status),
            Err(e) => return Err(e),
        };

        let has_propstat = response // There MUST be zero or one propstat.
            .descendants()
            .any(|node| node.tag_name() == names::PROPSTAT);

        if has_propstat {
            let href = get_unquoted_href(&response)?.to_string();

            if let Some(status) = status {
                items.push(FetchedResource {
                    href,
                    content: Err(status),
                });
                continue;
            }

            let etag = response
                .descendants()
                .find(|node| node.tag_name() == crate::names::GETETAG)
                .ok_or(WebDavError::InvalidResponse(
                    "missing etag in response".into(),
                ))?
                .text()
                .ok_or(WebDavError::InvalidResponse("missing text in etag".into()))?
                .to_string();
            let data = get_newline_corrected_text(&response, property)?;

            items.push(FetchedResource {
                href,
                content: Ok(FetchedResourceContent { data, etag }),
            });
        } else {
            let hrefs = response
                .descendants()
                .filter(|node| node.tag_name() == names::HREF);

            for href in hrefs {
                let href = href
                    .text()
                    .map(percent_decode_str)
                    .ok_or(WebDavError::InvalidResponse("missing text in href".into()))?
                    .decode_utf8()?
                    .to_string();
                let status = status.ok_or(WebDavError::InvalidResponse(
                    "missing props but no error status code".into(),
                ))?;
                items.push(FetchedResource {
                    href,
                    content: Err(status),
                });
            }
        }
    }

    Ok(items)
}

#[cfg(test)]
mod more_tests {

    use http::{StatusCode, Uri};

    use crate::{
        dav::{list_resources_parse, multi_get_parse, parse_prop, parse_prop_href, ListedResource},
        names::{self, CALENDAR_COLOUR, CALENDAR_DATA, CURRENT_USER_PRINCIPAL, DISPLAY_NAME},
        FetchedResource, FetchedResourceContent, ItemDetails, ResourceType,
    };

    #[test]
    fn test_multi_get_parse() {
        let raw = br#"
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/">
  <response>
    <href>/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/</href>
    <propstat>
      <prop>
        <resourcetype>
          <collection/>
          <C:calendar/>
        </resourcetype>
        <getcontenttype>text/calendar; charset=utf-8</getcontenttype>
        <getetag>"1591712486-1-1"</getetag>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
  <response>
    <href>/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/395b00a0-eebc-40fd-a98e-176a06367c82.ics</href>
    <propstat>
      <prop>
        <resourcetype/>
        <getcontenttype>text/calendar; charset=utf-8; component=VEVENT</getcontenttype>
        <getetag>"e7577ff2b0924fe8e9a91d3fb2eb9072598bf9fb"</getetag>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;

        let results = list_resources_parse(
            raw,
            "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/",
        )
        .unwrap();

        assert_eq!(results, vec![ListedResource {
            details: ItemDetails {
                content_type: Some("text/calendar; charset=utf-8; component=VEVENT".into()),
                etag: Some("\"e7577ff2b0924fe8e9a91d3fb2eb9072598bf9fb\"".into()),
                resource_type: ResourceType {
                    is_collection: false,
                    is_calendar: false,
                    is_address_book: false
                },
            },
            href: "/dav/calendars/user/vdirsyncer@fastmail.com/cc396171-0227-4e1c-b5ee-d42b5e17d533/395b00a0-eebc-40fd-a98e-176a06367c82.ics".into()
        }]);
    }

    #[test]
    fn test_multi_get_parse_with_err() {
        let raw = br#"
<ns0:multistatus xmlns:ns0="DAV:" xmlns:ns1="urn:ietf:params:xml:ns:caldav">
  <ns0:response>
    <ns0:href>/user/calendars/Q208cKvMGjAdJFUw/qJJ9Li5DPJYr.ics</ns0:href>
    <ns0:propstat>
      <ns0:status>HTTP/1.1 200 OK</ns0:status>
      <ns0:prop>
        <ns0:getetag>"adb2da8d3cb1280a932ed8f8a2e8b4ecf66d6a02"</ns0:getetag>
        <ns1:calendar-data>CALENDAR-DATA-HERE</ns1:calendar-data>
      </ns0:prop>
    </ns0:propstat>
  </ns0:response>
  <ns0:response>
    <ns0:href>/user/calendars/Q208cKvMGjAdJFUw/rKbu4uUn.ics</ns0:href>
    <ns0:status>HTTP/1.1 404 Not Found</ns0:status>
  </ns0:response>
</ns0:multistatus>
"#;

        let results = multi_get_parse(raw, &CALENDAR_DATA).unwrap();

        assert_eq!(
            results,
            vec![
                FetchedResource {
                    href: "/user/calendars/Q208cKvMGjAdJFUw/qJJ9Li5DPJYr.ics".into(),
                    content: Ok(FetchedResourceContent {
                        data: "CALENDAR-DATA-HERE".into(),
                        etag: "\"adb2da8d3cb1280a932ed8f8a2e8b4ecf66d6a02\"".into(),
                    })
                },
                FetchedResource {
                    href: "/user/calendars/Q208cKvMGjAdJFUw/rKbu4uUn.ics".into(),
                    content: Err(StatusCode::NOT_FOUND)
                }
            ]
        );
    }

    #[test]
    fn test_multi_get_parse_mixed() {
        let raw = br#"
<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav">
    <d:response>
        <d:href>/remote.php/dav/calendars/vdirsyncer/1678996875/</d:href>
        <d:propstat>
            <d:prop>
                <d:resourcetype>
                    <d:collection/>
                    <cal:calendar/>
                </d:resourcetype>
            </d:prop>
            <d:status>HTTP/1.1 200 OK</d:status>
        </d:propstat>
        <d:propstat>
            <d:prop>
                <d:getetag/>
            </d:prop>
            <d:status>HTTP/1.1 404 Not Found</d:status>
        </d:propstat>
    </d:response>
</d:multistatus>"#;

        let results = multi_get_parse(raw, &CALENDAR_DATA).unwrap();

        assert_eq!(
            results,
            vec![FetchedResource {
                href: "/remote.php/dav/calendars/vdirsyncer/1678996875/".into(),
                content: Err(StatusCode::NOT_FOUND)
            }]
        );
    }

    #[test]
    fn test_multi_get_parse_encoding() {
        let b = r#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <response>
    <href>/dav/calendars/user/hugo@whynothugo.nl/2100F960-2655-4E75-870F-CAA793466105/0F276A13-FBF3-49A1-8369-65EEA9C6F891.ics</href>
    <propstat>
      <prop>
        <getetag>"4219b87012f42ce7c4db55599aa3b579c70d8795"</getetag>
        <C:calendar-data><![CDATA[BEGIN:VCALENDAR
CALSCALE:GREGORIAN
PRODID:-//Apple Inc.//iOS 17.0//EN
VERSION:2.0
BEGIN:VTODO
COMPLETED:20230425T155913Z
CREATED:20210622T182718Z
DTSTAMP:20230915T132714Z
LAST-MODIFIED:20230425T155913Z
PERCENT-COMPLETE:100
SEQUENCE:1
STATUS:COMPLETED
SUMMARY:Comidas: ñoquis, 西红柿
UID:0F276A13-FBF3-49A1-8369-65EEA9C6F891
X-APPLE-SORT-ORDER:28
END:VTODO
END:VCALENDAR
]]></C:calendar-data>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;

        let resources = multi_get_parse(b, &names::CALENDAR_DATA).unwrap();
        let content = resources.into_iter().next().unwrap().content.unwrap();
        assert!(content.data.contains("ñoquis"));
        assert!(content.data.contains("西红柿"));
    }

    /// See: <https://github.com/RazrFalcon/roxmltree/issues/108>
    #[test]
    fn test_multi_get_parse_encoding_another() {
        let b = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<multistatus xmlns=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\n  <response>\n    <href>/dav/calendars/user/hugo@whynothugo.nl/2100F960-2655-4E75-870F-CAA793466105/0F276A13-FBF3-49A1-8369-65EEA9C6F891.ics</href>\n    <propstat>\n      <prop>\n        <getetag>\"4219b87012f42ce7c4db55599aa3b579c70d8795\"</getetag>\n        <C:calendar-data><![CDATA[BEGIN(baño)END\r\n]]></C:calendar-data>\n      </prop>\n      <status>HTTP/1.1 200 OK</status>\n    </propstat>\n  </response>\n</multistatus>\n";
        let resources = multi_get_parse(b, &names::CALENDAR_DATA).unwrap();
        let content = resources.into_iter().next().unwrap().content.unwrap();
        assert!(content.data.contains("baño"));
    }

    #[test]
    fn test_parse_prop_href() {
        let raw = br#"
<multistatus xmlns="DAV:">
  <response>
    <href>/dav/calendars</href>
    <propstat>
      <prop>
        <current-user-principal>
          <href>/dav/principals/user/vdirsyncer@example.com/</href>
        </current-user-principal>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;

        let results = parse_prop_href(
            raw,
            &Uri::try_from("https://example.com/").unwrap(),
            &CURRENT_USER_PRINCIPAL,
        )
        .unwrap();

        assert_eq!(
            results,
            Some(
                Uri::try_from("https://example.com/dav/principals/user/vdirsyncer@example.com/")
                    .unwrap()
            )
        );
    }

    #[test]
    fn test_parse_prop_cdata() {
        let raw = br#"
            <multistatus xmlns="DAV:">
                <response>
                    <href>/path</href>
                    <propstat>
                        <prop>
                            <displayname><![CDATA[test calendar]]></displayname>
                        </prop>
                        <status>HTTP/1.1 200 OK</status>
                    </propstat>
                </response>
            </multistatus>
            "#;

        let results = parse_prop(raw, &DISPLAY_NAME).unwrap();

        assert_eq!(results, Some("test calendar".into()));
    }

    #[test]
    fn test_parse_prop_text() {
        let raw = br#"
<ns0:multistatus xmlns:ns0="DAV:" xmlns:ns1="http://apple.com/ns/ical/">
  <ns0:response>
    <ns0:href>/user/calendars/pxE4Wt4twPqcWPbS/</ns0:href>
    <ns0:propstat>
      <ns0:status>HTTP/1.1 200 OK</ns0:status>
      <ns0:prop>
        <ns1:calendar-color>#ff00ff</ns1:calendar-color>
      </ns0:prop>
    </ns0:propstat>
  </ns0:response>
</ns0:multistatus>"#;

        let results = parse_prop(raw, &CALENDAR_COLOUR).unwrap();
        assert_eq!(results, Some("#ff00ff".into()));

        parse_prop(raw, &DISPLAY_NAME).unwrap_err();
    }

    #[test]
    fn test_parse_prop() {
        // As returned by Fastmail.
        let body = concat!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
            "<multistatus xmlns=\"DAV:\">",
            "<response>",
            "<href>/dav/calendars/user/hugo@whynothugo.nl/37c044e7-4b3d-4910-ba31-55038b413c7d/</href>",
            "<propstat>",
            "<prop>",
            "<calendar-color><![CDATA[#FF2968]]></calendar-color>",
            "</prop>",
            "<status>HTTP/1.1 200 OK</status>",
            "</propstat>",
            "</response>",
            "</multistatus>",
        );
        let parsed = parse_prop(body, &names::CALENDAR_COLOUR).unwrap();
        assert_eq!(parsed, Some(String::from("#FF2968")));
    }
}
