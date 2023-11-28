// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Builder types used for both caldav and carddav clients.
//!
//! The main type here is [`ClientBuilder`].
use std::marker::PhantomData;

use email_address::EmailAddress;
use http::Uri;

use crate::auth::{Auth, Password};

pub struct NeedsUri(());
pub struct NeedsAuth {
    uri: Uri,
}
pub struct NeedsPassword {
    uri: Uri,
    username: String,
}
pub struct Ready {
    pub(crate) uri: Uri,
    pub(crate) auth: Auth,
}

/// A builder for clients.
///
/// Use [`CalDavClient::builder`] and [`CardDavClient::builder`] to create a new builder instance.
///
/// [`CalDavClient::builder`]: `super::CalDavClient::builder`
/// [`CardDavClient::builder`]: `super::CardDavClient::builder`
#[allow(clippy::module_name_repetitions)]
pub struct ClientBuilder<ClientType, State> {
    pub(crate) state: State,
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
    pub fn with_auth(self, auth: Auth) -> ClientBuilder<ClientType, Ready> {
        ClientBuilder {
            state: Ready {
                uri: self.state.uri,
                auth,
            },
            phantom: self.phantom,
        }
    }
}

impl<ClientType> ClientBuilder<ClientType, NeedsPassword> {
    /// Sets the password.
    pub fn with_password<P: Into<Password>>(self, password: P) -> ClientBuilder<ClientType, Ready> {
        ClientBuilder {
            state: Ready {
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
    pub fn without_password(self) -> ClientBuilder<ClientType, Ready> {
        ClientBuilder {
            state: Ready {
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
