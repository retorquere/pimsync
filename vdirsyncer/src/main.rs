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
    sync::{declare::StoragePair, plan::Plan, state::PairState},
};

use crate::cli::Vdirsyncer;

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
    fn load_state(&self) -> anyhow::Result<Option<PairState>> {
        match std::fs::read_to_string(&self.status_path) {
            Ok(raw) => {
                let state: PairState = toml::from_str(&raw)
                    .with_context(|| format!("Parsing status file for {}.", self.name))?;
                debug!("Loaded status file for pair {}", self.name);
                Ok(Some(state))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                debug!("No status file for pair {}.", self.name);
                Ok(None)
            }
            Err(e) => Err(e).context("Error reading status file for {name}."),
        }
    }

    fn save_state(&self, new_state: &PairState) -> anyhow::Result<()> {
        debug!("Saving state file for pair {}.", self.name);
        let serialised = toml::to_string(new_state).context("Failed to serialise new status.")?;

        // FIXME: save atomically
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.status_path)
            .context("Failed to open file to save state.")?
            .write(serialised.as_bytes())
            .context("Error writing new state to file.")?;

        Ok(())
    }

    /// Returns an error only if it is fatal.
    ///
    /// If partial errors occurred during synchronisations, returns `Ok(())`.
    async fn synchronise_pair(self: &NamedPair<I>, dry_run: bool) -> anyhow::Result<()> {
        // TODO: lock storages so we can do things in parallel
        let state = match self.load_state() {
            Ok(s) => s,
            Err(err) => {
                // TODO: is this enough, or is {:?} better for the error?
                error!("Skipping pair {}; failed to load state: {}", self.name, err);
                return Ok(());
            }
        };

        debug!("Creating plan for storage pair '{}'.", self.name);
        let plan = match Plan::new(&self.inner, state.as_ref()).await {
            Ok(p) => p,
            Err(err) => {
                // TODO: is this enough, or is {:?} better for the error?
                error!("Skipping pair {}; planning failed: {}", self.name, err);
                return Ok(());
            }
        };

        // TODO: print this in more human-friendly format
        dbg!(&plan);

        if dry_run {
            debug!("Dry run: not synchronising.");
        } else {
            let sync_result = plan.execute().await;
            for err in sync_result.errors() {
                error!("Error during syncrhonisation: {err}");
            }

            if let Err(err) = self.save_state(sync_result.final_state()) {
                error!("Saving the current state failed. This is a fatal error.");
                error!("If any changes occurr before the next synchronisation, they will result in conflict!");
                return Err(err);
            };
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
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Vdirsyncer::parse();
    let log_level = cli.log_level();
    simple_logger::init_with_level(log_level).expect("logger should initialise");
    info!("Logging enabled with {} level", log_level);

    let config = config::load_from_default_path().context("could not load configuration file")?;
    debug!("Parsed configuration: {:?}", &config);

    if cli.check {
        return Ok(());
    }

    let app = config
        .into_app()
        .await
        .context("Failed to initialise with given configuration.")?;
    debug!("Initialised application");

    if cli.continuous {
        if cli.dry_run {
            bail!("--dry-run and --continuous are mutually exclusive");
        }
        warn!("Storage monitoring is not implemented, will auto-sync every 5 minutes.");
        // TODO: HTTPS connections are kept open for a while; this should also be configurable.
        loop {
            app.sync(false).await;
            // TODO: make this interval configurable.
            tokio::time::sleep(app.interval).await;
        }
    } else {
        app.sync(cli.dry_run).await
    }

    // TODO: turn storages into lockables
    // TODO: create per-pair tasks and sync pairs in parallel.
}
