// Copyright 2023-2024 Hugo Osvaldo Barrera
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
use log::{debug, error, info, trace, warn};
use rustix::fs::sync;
use stdio::{StdIo, StdIoLock};
use tempfile::NamedTempFile;
use tokio::{sync::Mutex, task::JoinSet};
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

use crate::cli::{Cli, Command};

mod cli;
mod config;
mod stdio;
mod tls;

pub const VERSION: &str = "2.0.0-alpha0";

/// Pair with a name, as defined in the configuration file.
pub(crate) struct NamedPair<I: Item> {
    name: String,
    pub(crate) inner: StoragePair<I>,
    status_path: Utf8PathBuf,
    conflict_resolution: Option<RawCommand>,
    /// Discretionary locks taken before using a storage.
    /// These MUST be sorted based on storage name to prevent possible deadlocks.
    locks: (Arc<Mutex<()>>, Arc<Mutex<()>>),
    names: (String, String),
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
        StatusDatabase::open_readonly(&self.status_path)
            .with_context(|| format!("opening status db for {}", self.name))
    }

    fn open_status_rw(&self) -> anyhow::Result<StatusDatabase> {
        StatusDatabase::open_or_create(&self.status_path)
            .with_context(|| format!("opening or creating status db for {}", self.name))
    }

    /// Sync this pair indefinitely
    ///
    /// Returns an error if an only if a fatal synchronisation error occurred.
    async fn daemon(self, interval: Duration) -> anyhow::Error {
        loop {
            let lock_0 = self.locks.0.lock().await;
            let lock_1 = self.locks.1.lock().await;
            match self.create_plan().await {
                Ok(plan) => {
                    self.print_plan(&plan);
                    if let Err(err) = self.execute_plan(plan).await {
                        return err;
                    }
                }
                Err(err) => error!("error creating plan for {}: {}", self.name, err),
            }
            drop(lock_0);
            drop(lock_1);

            // TODO: HTTPS connections are kept open for a while; this should also be configurable.
            warn!(
                "Monitoring is not implemented, will auto-sync every {} minutes.",
                interval.as_secs() / 60
            );
            tokio::time::sleep(interval).await;
        }
    }

    async fn sync_once(self, dry_run: bool /* ui-lock ? */) -> anyhow::Result<()> {
        // TODO: take some broadcast channel where events are sent:
        //       enum Event: CreatePlan, PrintPlan, ExecutePlan, Monitor
        let lock_0 = self.locks.0.lock().await;
        let lock_1 = self.locks.1.lock().await;
        let plan = self.create_plan().await?;
        self.print_plan(&plan);
        if !dry_run {
            self.execute_plan(plan).await?;
        }
        drop(lock_0);
        drop(lock_1);
        Ok(())
    }

    /// Returns an error only if it is fatal.
    ///
    /// If partial errors occurred during synchronisations, returns `Ok(None)`.
    async fn create_plan(&self) -> anyhow::Result<Plan<I>> {
        debug!("Creating plan for storage pair '{}'.", self.name);
        Ok(Plan::new(&self.inner, self.open_status_ro()?.as_ref()).await?)
    }

    // Execute and consume the current plan.
    async fn execute_plan(&self, plan: Plan<I>) -> anyhow::Result<()> {
        Ok(plan.execute(&self.open_status_rw()?, log_error).await?)
    }

    async fn discover(&self) -> anyhow::Result<()> {
        // TODO: discover displaynames and colours too
        let disco = self.inner.storage_a().discover_collections().await?;
        println!("For pair {}, storage a/{}:", self.name, self.names.0);
        for collection in disco.collections() {
            println!("- id={} href={}", collection.id(), collection.href());
        }

        let disco = self.inner.storage_b().discover_collections().await?;
        println!("For pair {}, storage b/{}:", self.name, self.names.1);
        for collection in disco.collections() {
            println!("- id={} href={}", collection.id(), collection.href());
        }

        Ok(())
    }

    fn print_plan(&self, plan: &Plan<I>) {
        // TODO: need to lock stdout/stderr for concurrent runs.
        info!(">>> Plan for storage pair '{}'", self.name);
        for cp in &plan.collection_plans {
            info!(
                "collection: {}, action: {}. {} item actions. {} property actions.",
                cp.alias(),
                cp.collection_action,
                cp.item_actions.len(),
                cp.property_actions.len(),
            );

            for item in &cp.item_actions {
                info!("item: {}", item);
                debug!("{item:?}");
            }
            for prop in &cp.property_actions {
                info!("property: {:?}", prop);
            }
        }
    }

    async fn resolve_conflicts(self, stdio: Arc<StdIo>) -> anyhow::Result<()> {
        // TODO: are storage locks necessary here?
        let Some(ref raw_cmd) = self.conflict_resolution else {
            error!("No conflict resolution command for {}.", self.name);
            return Ok(());
        };
        info!("Resolving conflicts for pair {}.", self.name);

        let plan = self.create_plan().await?;
        self.print_plan(&plan);

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

        trace!("Taking stdio lock...");
        let lock = stdio.lock().await;
        trace!("Stdio lock taken.");

        for (i, (a, b)) in conflicts.into_iter().enumerate() {
            println!("Next is item {}/{total}", i + 1);
            continue_or_abort(&lock)?;

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
fn continue_or_abort(stdio: &StdIoLock) -> anyhow::Result<()> {
    let input = stdio.stdin();

    loop {
        println!("Continue? [Y/n]");
        // Need to read entire lines because the stdlib implicitly buffers stdin.
        let mut response = String::new();
        input
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
    interval: Duration,
    calendar_pairs: Vec<NamedPair<IcsItem>>,
    contact_pairs: Vec<NamedPair<VcardItem>>,
    stdio: Arc<StdIo>,
}

impl App {
    async fn discover(&self) -> anyhow::Result<()> {
        // FIXME: if multiple pairs share a storage, only print that storage once.
        for pair in &self.calendar_pairs {
            pair.discover().await?;
        }
        for pair in &self.contact_pairs {
            pair.discover().await?;
        }
        Ok(())
    }

    async fn daemon(self) -> anyhow::Result<()> {
        let mut set = JoinSet::new();
        for pair in self.calendar_pairs {
            set.spawn(pair.daemon(self.interval));
        }
        for pair in self.contact_pairs {
            set.spawn(pair.daemon(self.interval));
        }

        while let Some(res) = set.join_next().await {
            match res {
                Ok(err) => error!("Error in sync task: {}.", err),
                Err(joinerr) => error!("Sync task aborted: {}.", joinerr),
            }
        }
        anyhow::bail!("All sync tasks exited.");
    }

    async fn sync(self, dry_run: bool) -> anyhow::Result<()> {
        let mut set = JoinSet::new();
        for pair in self.calendar_pairs {
            set.spawn(pair.sync_once(dry_run));
        }
        for pair in self.contact_pairs {
            set.spawn(pair.sync_once(dry_run));
        }

        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(err)) => error!("Error in sync task: {}.", err),
                Err(joinerr) => error!("Sync task aborted: {}.", joinerr),
            }
        }
        Ok(())
    }

    async fn resolve_conflicts(self, dry_run: bool) -> anyhow::Result<()> {
        if dry_run {
            bail!("dry_run is not implemented for resolve-conflicts");
        }

        let mut set = JoinSet::new();
        for pair in self.calendar_pairs {
            set.spawn(pair.resolve_conflicts(self.stdio.clone()));
        }
        for pair in self.contact_pairs {
            set.spawn(pair.resolve_conflicts(self.stdio.clone()));
        }

        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(err)) => error!("Error resolving conflicts: {}.", err),
                Err(joinerr) => error!("Sync task aborted: {}.", joinerr),
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = match Cli::parse(std::env::args()) {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("Bad usage: {err}\n");
            eprintln!("Usage: vdirsyncer [-v LOGLEVEL] COMMAND [ARGS...]");
            eprintln!("Commands:");
            eprintln!("\tcheck\t\t\t\tcheck configuration and exit");
            eprintln!("\tdaemon -[r] [PAIR]\t\tkeep storages in sync");
            eprintln!("\tsync [-d] [PAIR]\t\tsync storages once");
            eprintln!("\tresolve-conflicts [-d] [PAIR]\tmanually resolve conflicts");
            eprintln!("\tdiscover\t\t\tprint discovered collections");
            eprintln!("\tversion\t\t\t\tprint version");
            eprintln!("See 'man vdirsyncer' for details");
            std::process::exit(100);
        }
    };

    if let Command::Version = cli.command {
        println!("vdirsyncer {VERSION}");
        return Ok(());
    };

    simple_logger::SimpleLogger::new()
        .with_level(cli.log_level)
        .init()
        .expect("logger should initialise");
    info!("Logging enabled with {} level", cli.log_level);

    let config = config::load_from_default_path().context("could not load configuration file")?;
    trace!("Parsed configuration: {:?}", &config);

    let app = config
        .into_app(cli.pairs)
        .await
        .context("initialising application")?;
    debug!("Initialised application");

    match cli.command {
        Command::Check => Ok(()),
        Command::Daemon { ready_fd } => {
            // Everything is ready; indicate this before actual daemon work.
            if let Some(mut f) = ready_fd {
                f.write_all(b"READY=1\n")
                    .context("writing to readiness fd")?;
                // File is closed implicitly here.
            };
            app.daemon().await
        }
        Command::Sync { dry_run } => app.sync(dry_run).await,
        Command::ResolveConflicts { dry_run } => app.resolve_conflicts(dry_run).await,
        Command::Discover => app.discover().await,
        Command::Version => unreachable!(),
    }

    // TODO: turn storages into lockables
    // TODO: create per-pair tasks and sync pairs in parallel.
}
