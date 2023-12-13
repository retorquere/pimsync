// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

#![allow(unused)]

use std::{fs::OpenOptions, io::Write, path::PathBuf, sync::Arc};

use anyhow::Context;
use log::{debug, error, trace};
use vstorage::{
    base::{IcsItem, Item, Storage, VcardItem},
    sync::{declare::StoragePair, plan::Plan, state::PairState},
};

mod config;
mod tls;

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
    status_path: PathBuf,
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
        let serialised = toml::to_string(new_state).context("Failed to serialise new status.")?;

        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.status_path)
            .context("Failed to open file to save status status.")?
            .write(serialised.as_bytes())
            .context("Error writing serialise status")?;

        Ok(())
    }
}

pub(crate) struct App {
    // TODO: this also needs a global Mutex, which will be taken when trying to take locks on
    // individual storages.
    calendar_pairs: Vec<NamedPair<IcsItem>>,
    contact_pairs: Vec<NamedPair<VcardItem>>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let log_level = log::Level::Debug;
    simple_logger::init_with_level(log_level).expect("logger should initialise");

    let config = config::parse_from_file("/home/hugo/.config/vdirsyncer/config.toml")?;
    debug!("Parsed configuration: {:?}", &config);

    let app = config
        .into_app()
        .await
        .context("Failed to initialise with given configuration.")?;
    debug!("Initialised application");

    syncrhonise_pairs(app.calendar_pairs).await;
    syncrhonise_pairs(app.contact_pairs).await;

    // TODO: turn storages into lockables
    // TODO: create per-pair tasks and sync pairs in parallel.

    dbg!("Sync complete!");
    Ok(())
}

async fn syncrhonise_pairs<I: Item>(pairs: Vec<NamedPair<I>>) -> anyhow::Result<()> {
    for pair in pairs {
        // TODO: locking storages so we can do things in parallel
        let state = pair.load_state()?;

        debug!("Creating plan for storage pair '{}'.", pair.name);
        let plan = Plan::new(&pair.inner, state.as_ref()).await?;

        dbg!(&plan);

        let sync_result = plan.execute().await;
        for err in sync_result.errors() {
            error!("Error during syncrhonisation: {err}");
        }

        // TODO: print state to stdout if this fails.
        //       keep in mind that this is FATAL!!
        pair.save_state(sync_result.final_state()).unwrap(); // FIXME: handle this delicately

        // TODO: save state ATOMICALLY!
    }
    Ok(())
}
