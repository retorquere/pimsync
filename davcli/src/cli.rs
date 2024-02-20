// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use clap::{Args, Parser, Subcommand, ValueEnum};
use http::Uri;

use crate::{caldav, carddav};

#[derive(Clone, ValueEnum)]
enum Verbosity {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

#[derive(Args)]
pub(crate) struct Server {
    /// A base URL from which to discover the server.
    ///
    /// Examples: `http://localhost:8080`, `https://example.com`.
    #[arg(long)]
    pub(crate) server_url: Uri,

    /// Username for authentication.
    #[arg(long)]
    pub(crate) username: String,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct Proto {
    /// Operate on a CalDav server.
    #[arg(long)]
    caldav: bool,

    /// Operate on a CardDav server.
    #[arg(long)]
    carddav: bool,
}

#[derive(Parser)]
#[clap(author, version = env!("DAVCLI_VERSION"), about, long_about = None)]
pub(crate) struct Cli {
    #[command(flatten)]
    proto: Proto,

    #[command(subcommand)]
    pub(crate) command: ServerCommand,

    /// Change logging verbosity
    ///
    /// Logging is always directed to `stderr`.
    #[clap(short, long)]
    verbose: Option<Verbosity>,
}

#[derive(Subcommand)]
pub(crate) enum ServerCommand {
    /// Perform discovery and print results
    Discover,
    /// Find collections under the home set.
    FindCollections,
    /// List items in a given collection.
    ListItems { collection_href: String },
    /// List all collections and items recursively.
    Tree,
    /// Fetches a single item.
    Get { resource_href: String },
    /// Create a new item.
    ///
    /// Data is read from stdin.
    Create { resource_href: String },
    /// Delete an item or collection.
    Delete {
        #[arg(long)]
        force: bool,
        href: String,
    },
}

impl Cli {
    pub(crate) fn execute(self) -> anyhow::Result<()> {
        assert_ne!(self.proto.carddav, self.proto.caldav);
        if self.proto.caldav {
            caldav::execute(self.command)
        } else {
            carddav::execute(self.command)
        }
    }

    /// Returns the desired log level based on the amount of `-v` flags.
    /// The default log level is WARN.
    pub(crate) fn log_level(&self) -> log::Level {
        match self.verbose {
            Some(Verbosity::Error) => log::Level::Error,
            Some(Verbosity::Warn) | None => log::Level::Warn,
            Some(Verbosity::Info) => log::Level::Info,
            Some(Verbosity::Debug) => log::Level::Debug,
            Some(Verbosity::Trace) => log::Level::Trace,
        }
    }
}

#[test]
fn verify_cli() {
    use clap::CommandFactory;
    Cli::command().debug_assert()
}
