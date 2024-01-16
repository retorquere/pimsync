use clap::Parser;

use crate::VERSION;

#[derive(Parser)]
#[clap(author, version = VERSION, about, long_about = None)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Vdirsyncer {
    /// Check configuration file and exit
    #[arg(short = 'C', long)]
    pub(crate) check: bool,

    /// Continuously monitor for changes and re-synchronise.
    #[arg(short, long)]
    pub(crate) continuous: bool,

    /// Only plan changes but don't execute any.
    #[arg(short, long)]
    pub(crate) dry_run: bool,

    /// Discover and display remote collections.
    #[arg(short = 'D', long)]
    pub(crate) discover: bool,

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
