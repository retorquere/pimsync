// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2
#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]

use std::{
    ffi::OsString,
    io::{read_to_string, Seek, Write},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context};
use camino::Utf8PathBuf;
use clap::Parser;
use log::{debug, error, info, warn};
use rustix::fs::sync;
use tempfile::NamedTempFile;
use tokio::sync::Mutex;
use vstorage::{
    base::{IcsItem, Item, Storage, VcardItem},
    sync::{
        declare::StoragePair,
        plan::{ItemAction, Plan},
        status::StatusDatabase,
        SyncError,
    },
    Etag,
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
    pub(crate) inner: StoragePair<I>,
    status_path: Utf8PathBuf,
    plan: Mutex<Option<Plan<I>>>,
    conflict_resolution: Option<RawCommand>,
}

/// Data necessary to create a new `Command` instance.
///
/// This helper is used for commands that need to be executed multiple times.
pub struct RawCommand {
    command: OsString,
    args: Vec<OsString>,
}

/// Simply log non-fatal errors.
#[allow(clippy::needless_pass_by_value)]
pub fn log_error(error: SyncError) {
    error!("{error}");
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
    /// If partial errors occurred during synchronisations, returns `Ok(None)`.
    async fn create_plan(&self) -> anyhow::Result<()> {
        let mut plan = self.plan.lock().await;
        if plan.is_some() {
            bail!("Pair {} already has a plan!", self.name);
        }
        let status = match self.open_status_ro() {
            Ok(s) => s,
            Err(err) => {
                error!("Skipping {}, failed to open status db: {}", self.name, err);
                return Ok(());
            }
        };

        debug!("Creating plan for storage pair '{}'.", self.name);
        match Plan::new(&self.inner, status.as_ref()).await {
            Ok(p) => *plan = Some(p),
            Err(err) => error!("Skipping pair {}; planning failed: {}", self.name, err),
        };
        Ok(())
    }

    // Execute and consume the current plan.
    async fn execute_plan(&self) -> anyhow::Result<()> {
        let mut plan = self.plan.lock().await;
        if let Some(plan) = plan.take() {
            let status = self.open_status_rw()?;
            plan.execute(&status, log_error).await?;
        } else {
            debug!("no plan to execute for {}", self.name);
        };

        Ok(())
    }

    async fn discover(&self) -> anyhow::Result<()> {
        // TODO: discover displaynames and colours too
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

    async fn print_plan(&self) {
        // TODO: need to lock stdout/stderr for concurrent runs.
        let plan = self.plan.lock().await;
        if let Some(plan) = plan.as_ref() {
            info!(">>> Plan for storage pair '{}'", self.name);
            for cp in &plan.collection_plans {
                info!(
                    "collection: {}, action: {}. {} item actions.",
                    cp.alias,
                    cp.collection_action,
                    cp.item_actions.len()
                );

                for item in &cp.item_actions {
                    info!("item: {}", item);
                    debug!("{item:?}");
                }
            }
        }
    }

    async fn resolve_conflicts(&self) -> anyhow::Result<()> {
        let Some(ref raw_cmd) = self.conflict_resolution else {
            error!("No conflict resolution command for {}.", self.name);
            return Ok(());
        };

        let Some(plan) = self.plan.lock().await.take() else {
            bail!("Attempted to resolve conflicts without a plan.")
        };

        info!("Resolving conflicts for pair {}.", self.name);

        // TODO: when we parallelise, should take lock on stdin here.

        let conflicts = plan
            .collection_plans
            .into_iter()
            .flat_map(|cp| cp.item_actions)
            .filter_map(|action| match action {
                ItemAction::Conflict { a, b, .. } => Some((a, b)),
                _ => None,
            })
            // Collect in order to compute the total amount (for display purposes).
            .collect::<Vec<_>>();

        let total = conflicts.len();

        for (i, (a, b)) in conflicts.into_iter().enumerate() {
            info!("Next is item {}/{total}", i + 1);
            continue_or_abort()?;

            // TODO: improve logging here.
            // TODO: move duplicated logic into a "read_item_to_tempfile" function.
            let (mut temp_a, etag_a) = save_item_to_tempfile(self.inner.storage_a(), &a.href)
                .await
                .context("fetching conflicted item from A")?;
            let (mut temp_b, etag_b) = save_item_to_tempfile(self.inner.storage_b(), &b.href)
                .await
                .context("fetching conflicted item from B")?;

            info!("Running conflict resolution for item {}", a.uid);
            let exit_status = std::process::Command::new(&raw_cmd.command)
                .args(&raw_cmd.args)
                .arg(temp_a.path())
                .arg(temp_b.path())
                .spawn()
                .context("executing conflict resolution command")?
                .wait()
                .context("waiting for conflict resolution command")?;

            if !exit_status.success() {
                error!("Conflict resolution command failed: {exit_status}");
                continue;
            }

            // Ensure that files are committed; otherwise we sometimes read empty data.
            sync();
            temp_a.rewind().context("seeking in temporary file for A")?;
            temp_b.rewind().context("seeking in temporary file for B")?;

            let new_a = read_to_string(temp_a).context("reading resolved item A")?;
            let new_b = read_to_string(temp_b).context("reading resolved item B")?;

            if new_a.is_empty() {
                error!("Resolved item A is empty.");
                continue;
            }
            if new_b.is_empty() {
                error!("Resolved item B is empty.");
                continue;
            }
            if new_a.trim() != new_b.trim() {
                error!("Conflict resolution yielded mismatching items; skipping");
                continue;
            }

            let new = I::from(new_a);
            drop(new_b);

            self.inner
                .storage_a()
                .update_item(&a.href, &etag_a, &new)
                .await
                .context("uploading resolved item into A")?;
            debug!("Uploaded resolved item to A.");

            self.inner
                .storage_b()
                .update_item(&b.href, &etag_b, &new)
                .await
                .context("uploading resolved item into B")?;
            debug!("Uploaded resolved item to B.");

            info!("Resolved conflicts for '{}'.", new.ident());
        }

        Ok(())
    }
}

async fn save_item_to_tempfile<I: Item>(
    storage: &dyn Storage<I>,
    href: &str,
) -> anyhow::Result<(NamedTempFile, Etag)> {
    let mut temp =
        NamedTempFile::new().context("creating temporary file for conflict resolution")?;
    debug!("Fetching {href} for conflict resolution...");
    let (data, etag) = storage
        .get_item(href)
        .await
        .context("fetching conflicting item from a")?;
    temp.write_all(data.as_str().as_bytes())
        .context("writing item into temporary file")?;

    Ok((temp, etag))
}

/// Returns an error if user chooses to abort.
fn continue_or_abort() -> anyhow::Result<()> {
    let stdin = std::io::stdin();

    loop {
        println!("Continue? [Y/n]");
        // Need to read entire lines because the stdlib implicitly buffers stdin.
        let mut response = String::new();
        stdin
            .read_line(&mut response)
            .context("Reading response from stdin")?;

        match response.trim().to_lowercase().as_str() {
            "" | "y" => return Ok(()),
            "n" => bail!("Aborted by user."),
            _ => {}
        };
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
    // Only retain a pair with the given name.
    fn only(&mut self, name: &str) {
        self.calendar_pairs.retain(|p| p.name == name);
        self.contact_pairs.retain(|p| p.name == name);
    }

    /// Returns an error if a fatal error has ocurred.
    async fn sync(&self, dry_run: bool, resolve_conflicts: bool) -> anyhow::Result<()> {
        // TODO: This needs to be rewritten entirely; duplicating each action for calendars and
        //       address books is terrible.
        // TODO: maybe should spawn a task for each pair here.
        //       the task takes some broadcast channel where events are sent:
        //       - enum Event: CreatePlan, PrintPlan, ExecutePlan, Monitor

        // TODO: protect from concurrent runs!
        for pair in &self.calendar_pairs {
            pair.create_plan().await?;
        }
        for pair in &self.contact_pairs {
            pair.create_plan().await?;
        }

        for pair in &self.calendar_pairs {
            pair.print_plan().await;
        }
        for pair in &self.contact_pairs {
            pair.print_plan().await;
        }

        // TODO: if conflict resolution is from_a or from_b, rewrite conflicting actions.

        if resolve_conflicts {
            for pair in &self.calendar_pairs {
                pair.resolve_conflicts().await?;
            }
            for pair in &self.contact_pairs {
                pair.resolve_conflicts().await?;
            }
            info!("Ran conflict resolution; skipping regular sync.");
            return Ok(());
        }

        if dry_run {
            info!("Dry run: not synchronising.");
        } else {
            for pair in &self.calendar_pairs {
                pair.execute_plan().await?;
            }
            for pair in &self.contact_pairs {
                pair.execute_plan().await?;
            }
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

    let mut app = config
        .into_app()
        .await
        .context("Failed to initialise with given configuration.")?;
    debug!("Initialised application");

    match cli.command {
        Command::Check => Ok(()),
        Command::Sync { pair } => {
            if let Some(name) = pair {
                app.only(&name);
            }
            warn!("Storage monitoring is not implemented, will auto-sync every 5 minutes.");
            // TODO: HTTPS connections are kept open for a while; this should also be configurable.
            loop {
                app.sync(false, false).await?;
                // TODO: make this interval configurable.
                tokio::time::sleep(app.interval).await;
            }
        }
        Command::SyncOnce { dry_run, pair } => {
            if let Some(name) = pair {
                app.only(&name);
            }
            app.sync(dry_run, false).await
        }
        Command::ResolveConflicts { dry_run, pair } => {
            if let Some(name) = pair {
                app.only(&name);
            }
            app.sync(dry_run, true).await
        }
        Command::Discover => app.discover().await,
    }

    // TODO: turn storages into lockables
    // TODO: create per-pair tasks and sync pairs in parallel.
}
