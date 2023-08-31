// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Common types used by caldav and carddav builder.
//!
//! See [`CalDavClient::builder`] and [`CrdlDavClient::builder`] as entry points.
//!
//! [`CalDavClient::builder`]: `crate::CalDavClient::builder`
//! [`CrdlDavClient::builder`]: `crate::CardDavClient::builder`
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
    pub fn with_email<S: AsRef<str>>(
        self,
        email: &EmailAddress,
    ) -> Result<ClientBuilder<ClientType, NeedsPassword>, WithEmailError> {
        // The `Uri` type is broken for this case. See: https://github.com/hyperium/http/issues/596

        Ok(ClientBuilder {
            state: NeedsPassword {
                uri: Uri::try_from(email.domain())?,
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
