use clap::Parser;

#[derive(Parser)]
#[clap(author, version = "2.0.0-alpha0", about, long_about = None)]
pub(crate) struct Vdirsyncer {
    /// Check configuration file and exit
    #[arg(short, long)]
    pub(crate) check: bool,

    /// Sync configured storage pairs.
    #[arg(short, long)]
    pub(crate) sync: bool,

    /// Continuously sync.
    #[arg(short, long)]
    pub(crate) daemon: bool,

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
