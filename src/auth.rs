// Copyright 2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: ISC

use std::task::{Context, Poll};

use base64::Engine as _;
use hyper::{
    header::AUTHORIZATION,
    http::{HeaderValue, Request, Response},
};
use tower::Service;

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Debug, Clone)]
pub struct AddAuthorization<S> {
    inner: S,
    value: Option<HeaderValue>,
}

impl<S> AddAuthorization<S> {
    pub fn auto(inner: S, credentials: Option<(String, String)>) -> AddAuthorization<S> {
        match credentials {
            Some((username, password)) => AddAuthorization::basic(inner, &username, &password),
            None => Self::none(inner),
        }
    }
    /// Add no Authorization header.
    pub fn none(inner: S) -> AddAuthorization<S> {
        AddAuthorization { inner, value: None }
    }

    /// Add Authorization header with username and (optionally) password.
    ///
    /// To use no password, supply `""` as a password.
    pub fn basic(inner: S, username: &str, password: &str) -> AddAuthorization<S> {
        let encoded = BASE64.encode(format!("{username}:{password}"));
        let mut value = HeaderValue::try_from(format!("Basic {encoded}"))
            .expect("base64 encoded string is a valid header value");
        value.set_sensitive(true);
        AddAuthorization {
            inner,
            value: Some(value),
        }
    }
}

impl<S, Tx, Rx> Service<Request<Tx>> for AddAuthorization<S>
where
    S: Service<Request<Tx>, Response = Response<Rx>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Tx>) -> Self::Future {
        if let Some(value) = &self.value {
            req.headers_mut().insert(AUTHORIZATION, value.clone());
        };
        self.inner.call(req)
    }
}
