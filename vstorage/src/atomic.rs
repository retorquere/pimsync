// Copyright 2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: ISC
//
use std::{
    ffi::{OsStr, OsString},
    os::{fd::OwnedFd, unix::ffi::OsStrExt},
    path::PathBuf,
    pin::Pin,
};

use rand::{distributions::Alphanumeric, thread_rng, Rng};
use rustix::{
    fd::AsFd,
    fs::{AtFlags, Mode, OFlags},
};
use tokio::{fs::File, io::AsyncWrite};

use crate::{Error, ErrorKind, Result};

/// File implementation which writes atomically.
///
/// After writting to an `AtomicFile`, either [`AtomicFile::commit`] or [`AtomicFile::commit_new`]
/// must be called. The write operation will be done atomically: regardless of whether (and when)
/// the opreation is interrupted, the resulting file is either untouched, or has all content
/// written. The target file will never be partially written.
pub struct AtomicFile {
    tempfile: File,
    dir: OwnedFd,
    temp_name: OsString,
    final_name: OsString,
}

impl AtomicFile {
    pub fn new(path: impl Into<PathBuf>) -> Result<AtomicFile> {
        let path = path.into();
        let dirpath = path
            .parent()
            .ok_or(ErrorKind::InvalidInput.error("path requires a parent"))?;
        let final_name = path
            .file_name()
            .ok_or(ErrorKind::InvalidInput.error("path requires a filename"))?
            .to_os_string();

        let dir = if dirpath.as_os_str().is_empty() {
            rustix::fs::open(".", OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())
        } else {
            rustix::fs::open(dirpath, OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty())
        }
        .map_err(|e| Error::new(ErrorKind::Io, e))?;

        let temp_name = {
            let mut rng = thread_rng();

            let mut buf = *b"123456.tmp";
            for c in buf.iter_mut().take(6) {
                *c = rng.sample(Alphanumeric);
            }

            OsStr::from_bytes(&buf).to_os_string()
        };

        // TODO: use O_TMPFILE instead of O_EXCL on linux
        //       (but need to fall back to the regular path due to heterogeneous support.

        let tempfile = rustix::fs::openat(
            dir.as_fd(),
            &temp_name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::from(0o600),
        )
        .map(|fd| File::from(std::fs::File::from(fd)))
        .map_err(|e| Error::new(ErrorKind::Io, e))?;

        Ok(AtomicFile {
            tempfile,
            dir,
            temp_name,
            final_name,
        })
    }

    /// Commit content into the specified path, overwriting if it already exists.
    ///
    /// # Caveats
    ///
    /// If the file does not exist, this operations also succeeds.
    pub fn commit(self) -> Result<()> {
        // TODO: must fsync parent directory first
        rustix::fs::renameat(&self.dir, self.temp_name, &self.dir, self.final_name)
            .map_err(|e| Error::new(ErrorKind::Io, e))?;
        Ok(())
    }

    /// Commit content into the specified path, failing if it already exists.
    pub fn commit_new(self) -> Result<()> {
        // TODO: must fsync parent directory first
        rustix::fs::linkat(
            &self.dir,
            &self.temp_name,
            &self.dir,
            &self.final_name,
            AtFlags::empty(),
        )
        .map_err(|e| Error::new(ErrorKind::Io, e))?;
        rustix::fs::unlinkat(self.dir, self.temp_name, AtFlags::empty())
            .map_err(|e| Error::new(ErrorKind::Io, e))?;
        Ok(())
    }
}

impl AsyncWrite for AtomicFile {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let inner = Pin::new(&mut Pin::get_mut(self).tempfile);
        AsyncWrite::poll_write(inner, cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let inner = Pin::new(&mut Pin::get_mut(self).tempfile);
        AsyncWrite::poll_flush(inner, cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let inner = Pin::new(&mut Pin::get_mut(self).tempfile);
        AsyncWrite::poll_shutdown(inner, cx)
    }
}
