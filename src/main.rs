// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2
#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]

use std::{
    fs::File,
    io::{read_to_string, Write},
    path::PathBuf,
    time::Duration,
};

use anyhow::{bail, Context};
use camino::Utf8PathBuf;
use config::{open_default_path, parse_config};
use conflict::interactive_resolution;
use futures_util::future::{select, Either};
use log::{debug, error, info, trace, warn};
use tokio::task::JoinSet;
use vstorage::sync::{
    declare::StoragePair,
    execute::Executor,
    plan::Plan,
    status::{StatusDatabase, StatusError},
    SyncError,
};

use crate::cli::{Cli, Command};

mod auth;
mod cli;
mod config;
mod conflict;
mod tls;
mod ua;

/// Current app version (determined at compile-time).
pub const VERSION: &str = env!("PIMSYNC_VERSION");

/// Per-storage conflict resolution mechanism.
#[derive(PartialEq, Debug)]
pub(crate) enum ConflictResolution {
    /// In case of conflict, keep the contents of storage a.
    KeepA,
    /// In case of conflict, keep the contents of storage b.
    KeepB,
    /// In case of conflict, fix it running a command.
    Cmd(RawCommand),
}

/// Pair with a name, as defined in the configuration file.
pub(crate) struct NamedPair {
    name: String,
    pub(crate) inner: StoragePair,
    status_path: Utf8PathBuf,
    conflict_resolution: Option<ConflictResolution>,
    names: (String, String),
    intervals: (Duration, Duration),
}

/// Data necessary to create a new `Command` instance.
///
/// Contrary to [`std::process::Command`], this can be used more than once.
#[derive(PartialEq, Debug)]
pub struct RawCommand {
    command: String,
    args: Vec<String>,
}

impl RawCommand {
    #[must_use]
    pub fn command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.command);
        cmd.args(&self.args);
        cmd
    }

    /// Create a new `RawCommand` from the given arguments.
    ///
    /// Returns `None` if arguments are empty.
    pub(crate) fn from_args<I>(mut args: I) -> Option<Self>
    where
        I: Iterator<Item = String>,
    {
        Some(RawCommand {
            command: args.next()?,
            args: args.collect(),
        })
    }
}

/// Simply log non-fatal errors.
#[allow(clippy::needless_pass_by_value)] // Required interface to pass this function.
pub fn log_error(error: SyncError) {
    error!("{error:?}");
}

#[derive(Debug, thiserror::Error)]
enum DaemonError {
    #[error("error interacting with status database: {0}")]
    Status(StatusError),
    #[error("error initialising storage monitor: {0}")]
    Monitor(vstorage::Error),
}

impl NamedPair {
    /// Sync this pair indefinitely
    ///
    /// Returns an error if an only if a fatal synchronisation error occurred.
    async fn daemon(self) -> DaemonError {
        // Start monitor before first sync; doing the opposite could lead to missing events between
        // first sync and initialising the monitor.
        let mut mon_a = match self.inner.storage_a().monitor(self.intervals.0).await {
            Ok(monitor) => monitor,
            Err(err) => return DaemonError::Monitor(err),
        };
        let mut mon_b = match self.inner.storage_b().monitor(self.intervals.1).await {
            Ok(monitor) => monitor,
            Err(err) => return DaemonError::Monitor(err),
        };

        // Open status DB once and keep that handle open.
        let status = match StatusDatabase::open_or_create(&self.status_path) {
            Ok(status) => status,
            Err(err) => return DaemonError::Status(err),
        };

        loop {
            // FIXME: implement partial sync. See: https://todo.sr.ht/~whynothugo/pimsync/131

            match select(mon_a.next_event(), mon_b.next_event()).await {
                Either::Left((event, _)) => {
                    debug!("Monitor for A yielded event {:?}", event);
                    // TODO: Build set of Changes based on received events.
                }
                Either::Right((event, _)) => {
                    debug!("Monitor for B yielded event {:?}", event);
                    // TODO: Build set of Changes based on received events.
                }
            };
            // TODO: Drain any remaining events in a non-blocking way (or with <100ms timeout).
            //       Handle batches of events together.

            warn!("Partial sync is not implemented; will perform full sync");

            debug!("Creating plan for storage pair '{}'.", self.name);
            match Plan::new(&self.inner, Some(&status)).await {
                Ok(plan) => {
                    if let Some(ConflictResolution::KeepA | ConflictResolution::KeepB) =
                        self.conflict_resolution
                    {
                        error!("Conflict auto-resolution is not implemented");
                    }
                    self.print_plan(&plan);
                    if let Err(err) = Executor::new(log_error).plan(plan, &status).await {
                        return DaemonError::Status(err);
                    };
                }
                Err(err) => error!("Error synchronising {}: {:?}", self.name, err),
            };
        }
    }

    /// Common code between `daemon` and `sync` commands.
    async fn sync_once(&self, dry_run: bool) -> anyhow::Result<()> {
        let plan = self.create_plan().await.context("creating plan")?;

        if let Some(ConflictResolution::KeepA | ConflictResolution::KeepB) =
            self.conflict_resolution
        {
            error!("Conflict auto-resolution is not implemented");
        }

        self.print_plan(&plan);
        if !dry_run {
            let status_rw = StatusDatabase::open_or_create(&self.status_path)
                .with_context(|| format!("open_or_create status db for {}", self.name))?;
            Executor::new(log_error)
                .plan(plan, &status_rw)
                .await
                .context("executing plan")?;
        }
        Ok(())
    }

    async fn create_plan(&self) -> anyhow::Result<Plan> {
        debug!("Creating plan for storage pair '{}'.", self.name);
        let status = StatusDatabase::open(&self.status_path)
            .with_context(|| format!("openstatus db for {}", self.name))?;

        let plan = Plan::new(&self.inner, status.as_ref()).await?;
        // TODO: apply keep_a or keep_b if required.

        Ok(plan)
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

    fn print_plan(&self, plan: &Plan) {
        // TODO: need to lock stdout/stderr for concurrent runs.
        info!(">>> Plan for storage pair '{}'", self.name);
        for cp in &plan.collection_plans {
            info!(
                "collection: {}, action: {}. {} item actions. {} property actions.",
                cp.alias(),
                cp.action,
                cp.items.len(),
                cp.properties.len(),
            );

            for item in &cp.items {
                info!("item: {}", item);
                debug!("{item:?}");
            }
            for prop in &cp.properties {
                info!("property: {:?}", prop);
            }
        }
        if !plan.stale_collections.is_empty() {
            info!("Stale mappings: {:?}", plan.stale_collections);
        }
    }
}

pub(crate) struct App {
    pairs: Vec<NamedPair>,
}

impl App {
    async fn discover(&self) -> anyhow::Result<()> {
        // FIXME: if multiple pairs share a storage, only print that storage once.
        for pair in &self.pairs {
            pair.discover().await?;
        }
        Ok(())
    }

    async fn daemon(self) -> anyhow::Result<()> {
        let mut set = JoinSet::new();
        for pair in self.pairs {
            set.spawn(pair.daemon());
        }

        while let Some(res) = set.join_next().await {
            match res {
                Ok(err) => error!("Error in daemon task: {:?}.", err),
                Err(joinerr) => error!("Daemon task aborted: {:?}.", joinerr),
            }
        }
        anyhow::bail!("All sync tasks exited.");
    }

    async fn sync(self, dry_run: bool) -> anyhow::Result<()> {
        let mut set = JoinSet::new();
        for pair in self.pairs {
            set.spawn(async move { pair.sync_once(dry_run).await });
        }

        while let Some(res) = set.join_next().await {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(err)) => error!("Error in sync task: {:?}.", err),
                Err(joinerr) => error!("Sync task aborted: {:?}.", joinerr),
            }
        }
        Ok(())
    }

    /// Interactively resolve conflicts.
    ///
    /// Storages are resolved sequentially, since each item will require user intervention.
    async fn resolve_conflicts(self, dry_run: bool) -> anyhow::Result<()> {
        if dry_run {
            bail!("dry_run is not implemented for resolve-conflicts");
        }

        for pair in self.pairs {
            if let Err(err) = interactive_resolution(pair).await {
                error!("Error resolving conflicts: {:?}.", err);
            }
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse(std::env::args()).unwrap_or_else(|err| {
        eprintln!("Bad usage: {err}\n");
        eprintln!("Usage: pimsync [-c CONFIGFILE ] [-v LOGLEVEL] COMMAND [PAIR...]");
        eprintln!("Commands:");
        eprintln!("\tcheck\t\t\tcheck configuration and exit");
        eprintln!("\tdaemon -[r READY_FD]\tkeep storages in sync");
        eprintln!("\tsync [-n]\t\tsync storages once");
        eprintln!("\tresolve-conflicts [-n]\tmanually resolve conflicts");
        eprintln!("\tdiscover\t\tprint discovered collections");
        eprintln!("\tversion\t\t\tprint version");
        eprintln!("See 'man pimsync' for details");
        std::process::exit(100);
    });

    if let Command::Version = cli.command {
        println!("pimsync {VERSION}");
        return Ok(());
    };

    simple_logger::SimpleLogger::new()
        .with_level(cli.log_level)
        .init()
        .expect("logger should initialise");
    info!("Logging enabled with {} level", cli.log_level);

    let (config_path, config_file) = match cli.config_file {
        Some(file) => {
            let path = PathBuf::from(file);
            let file = File::open(&path)
                .with_context(|| format!("Could not open {}.", path.to_string_lossy()))?;
            debug!("Opened config file {}", path.to_string_lossy());
            (path, file)
        }
        None => open_default_path()?,
    };

    let config_data = read_to_string(config_file)?;
    let config = parse_config(&config_data, cli.pairs.as_deref()).with_context(|| {
        format!(
            "Could not parse configuration file at {}",
            config_path.display()
        )
    })?;
    trace!("Parsed configuration: {:?}", &config);

    let app = config
        .into_app()
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

    // TODO: create per-pair tasks and sync pairs in parallel.
}
