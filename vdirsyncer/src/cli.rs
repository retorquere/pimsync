use clap::{Parser, Subcommand};

use crate::VERSION;

#[derive(Subcommand, PartialEq)]
pub(crate) enum Command {
    /// Check configuration file and exit
    Check,
    /// Sync configured storage pairs.
    Sync {
        /// Continuously monitor for changes and re-synchronise.
        #[arg(short, long)]
        continuous: bool,
        /// Only plan changes but don't execute any.
        #[arg(short, long)]
        dry_run: bool,
        /// Only synchronise this pair
        #[arg()]
        pair: Option<String>,
    },
    /// Discover and display remote collections.
    Discover,
}

#[derive(Parser)]
#[clap(author, version = VERSION, about, long_about = None)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Vdirsyncer {
    #[command(subcommand)]
    pub(crate) command: Command,

    /// Increase verbosity (can be specified more than once).
    #[clap(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

impl Vdirsyncer {
    pub(crate) fn log_level(&self) -> log::Level {
        match self.verbose {
            0 => log::Level::Warn,
            1 => log::Level::Info,
            2 => log::Level::Debug,
            3 => log::Level::Trace,
            _ => log::Level::max(),
        }
    }
}
