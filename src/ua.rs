use std::task::{Context, Poll};

use hyper::{
    header::{HeaderValue, USER_AGENT},
    Request, Response,
};
use tower::Service;

#[derive(Debug, Clone)]
pub struct UserAgent<S> {
    inner: S,
    user_agent: HeaderValue,
}

impl<S> UserAgent<S> {
    /// Add a custom User-Agent to outgoing requests.
    pub fn new(inner: S, user_agent: HeaderValue) -> UserAgent<S> {
        UserAgent { inner, user_agent }
    }
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
