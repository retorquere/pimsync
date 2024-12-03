// Copyright 2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2
use std::io::{stdin, Stdin};

use tokio::sync::{Mutex, MutexGuard};

/// Lock for stdin
///
/// Used to prevent concurrent tasks from interrupting each other when performing interactive I/O.
pub struct StdIo {
    stdin: Mutex<Stdin>,
}

impl StdIo {
    // TODO: should only be doable once (but might mess up testing?)
    pub fn new() -> StdIo {
        StdIo {
            stdin: Mutex::new(stdin()),
        }
    }

    pub async fn lock(&self) -> StdIoLock {
        let stdin = self.stdin.lock().await;
        StdIoLock { stdin }
    }
}

/// This lock is safe to hold across await points.
pub struct StdIoLock<'a> {
    stdin: MutexGuard<'a, Stdin>,
}

impl StdIoLock<'_> {
    pub fn stdin(&self) -> &Stdin {
        &self.stdin
    }
}
