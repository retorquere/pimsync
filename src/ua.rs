// Copyright 2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::task::{Context, Poll};

use hyper::{
    Request, Response,
    header::{HeaderValue, USER_AGENT},
};
use tower::{Layer, Service};

#[derive(Debug, Clone)]
pub struct UserAgent<S> {
    inner: S,
    user_agent: HeaderValue,
}

impl<S, Tx, Rx> Service<Request<Tx>> for UserAgent<S>
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
        req.headers_mut()
            .insert(USER_AGENT, self.user_agent.clone());
        self.inner.call(req)
    }
}

/// Layer that adds a User-Agent header to requests.
#[derive(Debug, Clone)]
pub struct UserAgentLayer {
    user_agent: HeaderValue,
}

impl UserAgentLayer {
    pub fn new(user_agent: HeaderValue) -> Self {
        UserAgentLayer { user_agent }
    }
}

impl<S> Layer<S> for UserAgentLayer {
    type Service = UserAgent<S>;

    fn layer(&self, inner: S) -> Self::Service {
        UserAgent {
            inner,
            user_agent: self.user_agent.clone(),
        }
    }
}
