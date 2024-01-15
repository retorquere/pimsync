use clap::Parser;

use crate::VERSION;

#[derive(Parser)]
#[clap(author, version = VERSION, about, long_about = None)]
pub(crate) struct Vdirsyncer {
    /// Check configuration file and exit
    #[arg(short = 'C', long)]
    pub(crate) check: bool,

    /// Sync configured storage pairs.
    #[arg(short, long)]
    pub(crate) sync: bool,

    /// Continuously monitor for changes and re-synchronise.
    #[arg(short, long)]
    pub(crate) continuous: bool,

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
