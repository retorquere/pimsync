// Copyright 2026 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Generic connector infrastructure for hyper clients.
//!
//! Building blocks for connecting to servers over arbitrary transports (TCP, Unix
//! sockets, etc.). These types exist with the intent of being moved out into a
//! separate re-usable library.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper::Uri;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::tokio::WithHyperIo;
use tokio::net::UnixStream;
use tower_service::Service;

/// Wraps a [`UnixStream`] for use with hyper's legacy client.
///
/// [`WithHyperIo`] bridges tokio's I/O traits to hyper's `Read`/`Write`. This newtype
/// adds the [`Connection`] impl required by `hyper_util::client::legacy::Client`.
pub struct ConnectedIo(WithHyperIo<UnixStream>);

impl hyper::rt::Read for ConnectedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for ConnectedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl Connection for ConnectedIo {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

/// Connector that always connects to a fixed Unix socket path.
///
/// Implements [`Service<Uri>`] for use with hyper's legacy client. The URI passed to
/// [`call`](Service::call) is ignored for connection routing and is only used by hyper
/// for the `Host` header and request-target. The actual connection is always make to
/// the stored socket path.
#[derive(Clone)]
pub struct UnixConnector {
    path: Arc<Path>,
}

impl UnixConnector {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Arc::from(path.into()),
        }
    }
}

impl Service<Uri> for UnixConnector {
    type Response = ConnectedIo;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let path = self.path.clone();
        Box::pin(async move {
            let stream = UnixStream::connect(&*path).await?;
            Ok(ConnectedIo(WithHyperIo::new(stream)))
        })
    }
}
