// Copyright 2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: ISC

use std::task::{Context, Poll};

use base64::Engine as _;
use hyper::{
    header::AUTHORIZATION,
    http::{HeaderValue, Request, Response},
};
use tower::{Layer, Service};

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Debug, Clone)]
pub struct AddAuthorization<S> {
    inner: S,
    value: Option<HeaderValue>,
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
        }
        self.inner.call(req)
    }
}

/// Layer that adds authorization to requests.
#[derive(Debug, Clone)]
pub struct AddAuthorizationLayer {
    value: Option<HeaderValue>,
}

impl AddAuthorizationLayer {
    pub fn auto(credentials: Option<(String, String)>) -> Self {
        let value = credentials.map(|(username, password)| {
            let encoded = BASE64.encode(format!("{username}:{password}"));
            let mut value = HeaderValue::try_from(format!("Basic {encoded}"))
                .expect("base64 encoded string is a valid header value");
            value.set_sensitive(true);
            value
        });
        AddAuthorizationLayer { value }
    }
}

impl<S> Layer<S> for AddAuthorizationLayer {
    type Service = AddAuthorization<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AddAuthorization {
            inner,
            value: self.value.clone(),
        }
    }
}
