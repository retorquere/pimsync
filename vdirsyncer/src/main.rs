// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

#![allow(unused)]

use std::{fs::OpenOptions, io::Write, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{bail, Context};
use camino::Utf8PathBuf;
use clap::Parser;
use log::{debug, error, info, trace, warn};
use vstorage::{
    base::{IcsItem, Item, Storage, VcardItem},
    sync::{declare::StoragePair, plan::Plan, status::StatusDatabase},
};

use crate::cli::{Command, Vdirsyncer};

mod cli;
mod config;
mod tls;

pub const VERSION: &str = "2.0.0-alpha0";

/// Storage with a name, as defined in the configuration file.
pub(crate) struct NamedStorage<I: Item> {
    name: String,
    // TODO: this should be wrapped in a Mutex. Once we allow pairs to synchronise concurrently,
    // this will avoid the same pair being re-used.
    inner: Arc<dyn Storage<I>>,
}

/// Pair with a name, as defined in the configuration file.
pub(crate) struct NamedPair<I: Item> {
    name: String,
    inner: StoragePair<I>,
    status_path: Utf8PathBuf,
}

impl<I: Item> NamedPair<I> {
    /// Returns `None` if the database doesn't exist.
    fn open_status_ro(&self) -> anyhow::Result<Option<StatusDatabase>> {
        Ok(StatusDatabase::open_readonly(&self.status_path)?)
    }

    fn open_status_rw(&self) -> anyhow::Result<StatusDatabase> {
        Ok(StatusDatabase::open_or_create(&self.status_path)?)
    }

    /// Returns an error only if it is fatal.
    ///
    /// If partial errors occurred during synchronisations, returns `Ok(())`.
    async fn synchronise_pair(self: &NamedPair<I>, dry_run: bool) -> anyhow::Result<()> {
        // TODO: lock storages so we can do things in parallel
        let status = match self.open_status_ro() {
            Ok(s) => s,
            Err(err) => {
                error!("Skipping {}; failed to open status db: {}", self.name, err);
                return Ok(());
            }
        };

        debug!("Creating plan for storage pair '{}'.", self.name);
        let plan = match Plan::new(&self.inner, status.as_ref()).await {
            Ok(p) => p,
            Err(err) => {
                error!("Skipping pair {}; planning failed: {}", self.name, err);
                return Ok(());
            }
        };
        drop(status);

        // FIXME: print this in more human-friendly format
        // TODO: print with log level INFO
        dbg!(&plan);

        if dry_run {
            debug!("Dry run: not synchronising.");
        } else {
            let status = self.open_status_rw()?;
            let sync_result = plan.execute(&status).await;
            for err in sync_result.errors() {
                error!("Error during syncrhonisation: {err}");
            }
        }

        Ok(())
    }

    async fn discover(&self) -> anyhow::Result<()> {
        let disco = self.inner.storage_a().discover_collections().await?;
        println!("For pair {}, storage a:", self.name);
        for collection in disco.collections() {
            println!("- id={} href={}", collection.id(), collection.href());
        }

        let disco = self.inner.storage_b().discover_collections().await?;
        println!("For pair {}, storage b:", self.name);
        for collection in disco.collections() {
            println!("- id={} href={}", collection.id(), collection.href());
        }

        Ok(())
    }
}

pub(crate) struct App {
    // TODO: this also needs a global Mutex, which will be taken when trying to take locks on
    // individual storages.
    interval: Duration,
    calendar_pairs: Vec<NamedPair<IcsItem>>,
    contact_pairs: Vec<NamedPair<VcardItem>>,
}

impl App {
    /// Returns an error if a fatal error has ocurred.
    async fn sync(&self, dry_run: bool) -> anyhow::Result<()> {
        for pair in &self.calendar_pairs {
            pair.synchronise_pair(dry_run).await?;
        }
        for pair in &self.contact_pairs {
            pair.synchronise_pair(dry_run).await?;
        }
        info!("Synchronisation complete");
        Ok(())
    }

    async fn discover(&self) -> anyhow::Result<()> {
        for pair in &self.calendar_pairs {
            pair.discover().await?;
        }
        for pair in &self.contact_pairs {
            pair.discover().await?;
        }
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Vdirsyncer::parse();
    let log_level = cli.log_level();
    simple_logger::init_with_level(log_level).expect("logger should initialise");
    info!("Logging enabled with {} level", log_level);

    let config = config::load_from_default_path().context("could not load configuration file")?;
    debug!("Parsed configuration: {:?}", &config);

    let app = config
        .into_app()
        .await
        .context("Failed to initialise with given configuration.")?;
    debug!("Initialised application");

    match cli.command {
        Command::Check => Ok(()),
        Command::Sync {
            continuous,
            dry_run,
        } => {
            if continuous {
                if dry_run {
                    bail!("dry-run and continuous are mutually exclusive");
                }
                warn!("Storage monitoring is not implemented, will auto-sync every 5 minutes.");
                // TODO: HTTPS connections are kept open for a while; this should also be configurable.
                loop {
                    app.sync(false).await;
                    // TODO: make this interval configurable.
                    tokio::time::sleep(app.interval).await;
                }
            } else {
                app.sync(dry_run).await
            }
        }
        Command::Discover => app.discover().await,
    }

    // TODO: turn storages into lockables
    // TODO: create per-pair tasks and sync pairs in parallel.
}
