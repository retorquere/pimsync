// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Authentication-related types.

use base64::{prelude::BASE64_STANDARD, write::EncoderWriter};
use core::fmt;
use http::{HeaderValue, Request};
use std::io::Write;

use crate::dav::RequestError;

/// Wrapper around a [`String`] that is not printed when debugging.
///
/// # Examples
///
/// ```
/// # use libdav::auth::Password;
/// let p1 = Password::from("secret");
/// let p2 = String::from("secret").into();
///
/// assert_eq!(p1, p2);
/// ```
///
/// # Display
///
/// The [`core::fmt::Display`] trait is intentionally not implemented. Use either
/// [`Password::into_string`] or [`Password::as_str()`].
#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<REDACTED>")
    }
}

impl<S> From<S> for Password
where
    String: From<S>,
{
    fn from(value: S) -> Self {
        Password(String::from(value))
    }
}

#[allow(clippy::from_over_into)] // `From<Password> for String` is not feasible.
impl Into<String> for Password {
    /// Returns the underlying string.
    fn into(self) -> String {
        self.0
    }
}

impl Password {
    /// Returns the underlying string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }

    /// Returns a reference to the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Authentication schemes supported by [`WebDavClient`](crate::dav::WebDavClient).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum Auth {
    None,
    Basic {
        username: String,
        password: Option<Password>,
    },
}

impl Auth {
    /// Apply this authentication to a request.
    pub(crate) fn apply<B>(&self, mut request: Request<B>) -> Result<Request<B>, RequestError> {
        // TODO: this will need to be async for things like Digest auth or OAuth.
        match self {
            Auth::None => Ok(request),
            Auth::Basic { username, password } => {
                let mut sequence = b"Basic ".to_vec();
                let mut encoder = EncoderWriter::new(sequence, &BASE64_STANDARD);
                if let Some(pwd) = password {
                    write!(encoder, "{username}:{}", pwd.0)?;
                } else {
                    write!(encoder, "{username}:")?;
                }
                sequence = encoder.finish()?;

                let mut header = HeaderValue::from_bytes(&sequence)
                    .expect("base64 string contains only ascii characters");
                header.set_sensitive(true);

                request
                    .headers_mut()
                    .insert(hyper::header::AUTHORIZATION, header);

                Ok(request)
            }
        }
    }
}
