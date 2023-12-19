// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Builder types used for both caldav and carddav clients.
//!
//! The main type here is [`ClientBuilder`].
use std::marker::PhantomData;

use domain::base::Dname;
use email_address::EmailAddress;
use http::Uri;
use hyper::client::connect::Connect;

use crate::{
    auth::{Auth, Password},
    common::{find_home_set, Rfc6764Protocol},
    dav::WebDavClient,
    dns::{find_context_path_via_txt_records, resolve_srv_record},
    BootstrapError, InvalidUrl,
};

pub struct NeedsUri(());
pub struct NeedsAuth {
    uri: Uri,
}
pub struct NeedsPassword {
    uri: Uri,
    username: String,
}
pub struct PendingDiscovery {
    pub(crate) uri: Uri,
    pub(crate) auth: Auth,
}
// Hint: This state is required to have generic without_discovery and with_home_set functions.
pub struct Ready<C>
where
    C: Connect + Clone + Sync + Send + 'static,
{
    pub(crate) dav_client: WebDavClient<C>,
    pub(crate) home_set: Option<Uri>,
}

/// A builder for clients.
///
/// Use [`CalDavClient::builder`] and [`CardDavClient::builder`] to create a new builder instance.
///
/// [`CalDavClient::builder`]: `super::CalDavClient::builder`
/// [`CardDavClient::builder`]: `super::CardDavClient::builder`
///
/// # Example
///
///```no_run
/// # use http::Uri;
/// use libdav::CardDavClient;
/// use libdav::auth::Auth;
/// use hyper_rustls::HttpsConnectorBuilder;
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// # let base_url = Uri::try_from("https://example.com").unwrap();
/// # let username = "test".to_string();
/// # let password = "test".to_string().into();
///
/// let https = HttpsConnectorBuilder::new()
///     .with_native_roots()
///     .https_or_http()
///     .enable_http1()
///     .build();
/// let carddav_client = CardDavClient::builder()
///     .with_uri(base_url)
///     .with_auth(Auth::Basic {
///         username,
///         password: Some(password),
///     })
///     .bootstrap(https)
///     .await
///     .unwrap()
///     .build();
/// # })
///```
#[allow(clippy::module_name_repetitions)]
pub struct ClientBuilder<ClientType, State> {
    state: State,
    phantom: PhantomData<ClientType>,
}

#[derive(thiserror::Error, Debug)]
pub enum WithEmailError {
    #[error("failed to build Uri from host portion")]
    Invalidhost(#[from] http::uri::InvalidUri),
}

impl<ClientType> ClientBuilder<ClientType, NeedsUri> {
    pub(crate) fn new() -> ClientBuilder<ClientType, NeedsUri> {
        ClientBuilder {
            state: NeedsUri(()),
            phantom: PhantomData,
        }
    }

    /// Sets the host and port from a `Uri`.
    ///
    /// # Example
    ///
    /// ```
    /// use hyper::client::HttpConnector;
    /// use hyper_rustls::HttpsConnector;
    /// use libdav::CalDavClient;
    /// use libdav::CardDavClient;
    ///
    /// CardDavClient::<HttpsConnector<HttpConnector>>::builder()
    ///     .with_uri("https://example.com".parse().unwrap());
    ///
    /// CalDavClient::<HttpsConnector<HttpConnector>>::builder()
    ///     .with_uri("caldavs://example.com".parse().unwrap());
    /// ```
    ///
    /// # Caveats
    ///
    /// Using a `mailto` Uri here is currently not possible due to [this bug in hyper].
    ///
    /// [this bug in hyper]: https://github.com/hyperium/http/issues/596
    pub fn with_uri(self, uri: Uri) -> ClientBuilder<ClientType, NeedsAuth> {
        ClientBuilder {
            state: NeedsAuth { uri },
            phantom: self.phantom,
        }
    }

    /// Sets the host and username from an email.
    ///
    /// # Errors
    ///
    /// If building the `base_uri` fails with the host extracted from the email address.
    pub fn with_email(
        self,
        email: &EmailAddress,
    ) -> Result<ClientBuilder<ClientType, NeedsPassword>, WithEmailError> {
        Ok(ClientBuilder {
            state: NeedsPassword {
                uri: Uri::try_from(email.domain())?,
                // TODO: rfc6764 says "clients MUST first use the "mailbox" portion of the calendar
                // user address provided by the user in the case of a "mailto:" address and, if
                // that results in an authentication failure, SHOULD fall back to using the "local-
                // part" extracted from the "mailto:" address."
                //
                // To implement this, the builder needs to be aware of this variation, and `build`
                // needs to be async.
                username: email.to_string(),
            },
            phantom: self.phantom,
        })
    }
}

impl<ClientType> ClientBuilder<ClientType, NeedsAuth> {
    /// Sets the authentication type and credentials.
    pub fn with_auth(self, auth: Auth) -> ClientBuilder<ClientType, PendingDiscovery> {
        ClientBuilder {
            state: PendingDiscovery {
                uri: self.state.uri,
                auth,
            },
            phantom: self.phantom,
        }
    }
}

impl<ClientType> ClientBuilder<ClientType, NeedsPassword> {
    /// Sets the password.
    ///
    /// Passing a `String` works, but using the `Password` type is recommended.
    pub fn with_password<P: Into<Password>>(
        self,
        password: P,
    ) -> ClientBuilder<ClientType, PendingDiscovery> {
        ClientBuilder {
            state: PendingDiscovery {
                uri: self.state.uri,
                auth: Auth::Basic {
                    username: self.state.username,
                    password: Some(password.into()),
                },
            },
            phantom: self.phantom,
        }
    }

    /// Sets no password.
    pub fn without_password(self) -> ClientBuilder<ClientType, PendingDiscovery> {
        ClientBuilder {
            state: PendingDiscovery {
                uri: self.state.uri,
                auth: Auth::Basic {
                    username: self.state.username,
                    password: None,
                },
            },
            phantom: self.phantom,
        }
    }
}

impl<ClientType: Rfc6764Protocol> ClientBuilder<ClientType, PendingDiscovery> {
    /// Perform client bootstrap sequence.
    ///
    /// Determines the server's real host and the context path of the resources for a server,
    /// following the discovery mechanism described in [rfc6764].
    ///
    /// [rfc6764]: https://www.rfc-editor.org/rfc/rfc6764
    ///
    /// # Errors
    ///
    /// If any of the underlying DNS or HTTP requests fail, or if any of the responses fail to
    /// parse.
    ///
    /// Does not return an error if DNS records as missing, only if they contain invalid data.
    pub async fn bootstrap<C>(
        self,
        connector: C,
    ) -> Result<ClientBuilder<ClientType, Ready<C>>, BootstrapError>
    where
        C: Connect + Clone + Sync + Send,
    {
        let service = ClientType::service(&self.state.uri)?;
        let home_set_prop = ClientType::home_set_property();

        let domain = self.state.uri.host().ok_or(InvalidUrl::MissingHost)?;
        let port = self.state.uri.port_u16().unwrap_or(service.default_port());

        let dname = Dname::bytes_from_str(domain).map_err(InvalidUrl::InvalidDomain)?;
        let host_candidates = resolve_srv_record(service, &dname, port)
            .await?
            .ok_or(BootstrapError::NotAvailable)?;

        let mut dav_client = WebDavClient::new(self.state.uri, self.state.auth, connector);
        if let Some(path) = find_context_path_via_txt_records(service, &dname).await? {
            let candidate = &host_candidates[0];

            // TODO: check `DAV:` capabilities here.
            dav_client.base_url = Uri::builder()
                .scheme(service.scheme())
                .authority(format!("{}:{}", candidate.0, candidate.1))
                .path_and_query(path)
                .build()
                .map_err(BootstrapError::UnusableSrv)?;
        } else {
            for candidate in host_candidates {
                if let Ok(Some(url)) = dav_client
                    .find_context_path(service, &candidate.0, candidate.1)
                    .await
                {
                    dav_client.base_url = url;
                    break;
                }
            }
        }

        dav_client.principal = dav_client.find_current_user_principal().await?;
        let home_set = find_home_set(&dav_client, home_set_prop).await?;

        Ok(ClientBuilder {
            state: Ready {
                dav_client,
                home_set,
            },
            phantom: self.phantom,
        })
    }

    /// Create a client without any discovery.
    ///
    /// This constructor is recommended only for situations where DNS-based discovery is
    /// unavailable or undesirable.
    ///
    /// When in doubt, use [`ClientBuilder::build`].
    // TODO: normalise wording; we mix "discovery" and "bootstrap" in some places.
    pub fn without_discovery<C>(self, connector: C) -> ClientBuilder<ClientType, Ready<C>>
    where
        C: Connect + Clone + Sync + Send + 'static,
    {
        ClientBuilder {
            state: Ready {
                dav_client: WebDavClient::new(self.state.uri, self.state.auth, connector),
                home_set: None,
            },
            phantom: self.phantom,
        }
    }

    pub fn with_home_set<C>(
        self,
        connector: C,
        home_set: Uri,
    ) -> ClientBuilder<ClientType, Ready<C>>
    where
        C: Connect + Clone + Sync + Send + 'static,
    {
        ClientBuilder {
            state: Ready {
                dav_client: WebDavClient::new(self.state.uri, self.state.auth, connector),
                home_set: Some(home_set),
            },
            phantom: self.phantom,
        }
    }
}

impl<ClientType: Rfc6764Protocol, C> ClientBuilder<ClientType, Ready<C>>
where
    C: Connect + Clone + Sync + Send,
{
    /// Returns a webdav client and the discovered home set.
    pub(crate) fn into_parts(self) -> (WebDavClient<C>, Option<Uri>) {
        (self.state.dav_client, self.state.home_set)
    }
}
