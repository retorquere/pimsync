use lexopt::{Arg, ValueExt as _};
use log::warn;
use rustix::fd::FromRawFd as _;
use std::{collections::HashSet, fs::File};

pub(crate) enum Command {
    Check,
    Daemon { ready_fd: Option<File> },
    Sync { dry_run: bool },
    ResolveConflicts { dry_run: bool },
    Discover,
    Repair,
    Version,
}

pub(crate) struct Cli {
    pub command: Command,
    pub log_level: log::LevelFilter,
    pub config_file: Option<String>,
    pub names: FilterNames,
}

/// Names by which to filter pairs or storages.
///
/// If no names are specified as filters, then all names are treated as wanted.
pub struct FilterNames(Option<HashSet<String>>);

impl FilterNames {
    fn new() -> FilterNames {
        FilterNames(None)
    }

    fn push(&mut self, new: String) {
        if !self.0.get_or_insert_default().insert(new) {
            warn!("Duplicate name specified.");
        }
    }

    /// Returns `true` if the provided name is a wanted one.
    ///
    /// Removes `name` from list of wanted items; calling this a second time always returns `false`.
    pub fn wants(&mut self, name: &str) -> bool {
        if let Some(set) = &mut self.0 {
            set.remove(name)
        } else {
            true
        }
    }

    /// Return the name of the next missing wanted name, if any.
    pub fn next_missing(self) -> Option<String> {
        self.0.and_then(|names| names.into_iter().next())
    }
}

impl Cli {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Cli, lexopt::Error> {
        let mut command = None;
        let mut log_level = log::LevelFilter::Warn;
        let mut config_file: Option<String> = None;
        let mut names = FilterNames::new();

        args.next(); // Skip arg0
        let mut parser = lexopt::Parser::from_args(args);
        while let Some(arg) = parser.next()? {
            match arg {
                Arg::Short('v') => log_level = parser.value()?.parse()?,
                Arg::Short('c') => config_file = Some(parser.value()?.string()?),
                Arg::Value(raw_cmd) => {
                    command = match raw_cmd.string()?.as_str() {
                        "check" => {
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    Arg::Value(pair_name) => names.push(pair_name.string()?),
                                    _ => return Err(arg.unexpected()),
                                }
                            }
                            Some(Command::Check)
                        }
                        "daemon" => {
                            let mut ready_fd = None;
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    Arg::Short('r') => {
                                        let raw_fd = parser.value()?.parse()?;
                                        if raw_fd == 2 {
                                            return Err("Cannot use stderr as readiness fd".into());
                                        }
                                        // SAFETY: this file descriptor is not accessed elsewhere.
                                        // The user is responsible for ensuring that they have
                                        // supplied a valid open file.
                                        ready_fd = Some(unsafe { File::from_raw_fd(raw_fd) });
                                    }
                                    Arg::Value(pair_name) => names.push(pair_name.string()?),
                                    _ => return Err(arg.unexpected()),
                                }
                            }
                            Some(Command::Daemon { ready_fd })
                        }
                        "sync" => {
                            let mut dry_run = false;
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    Arg::Short('n') => dry_run = true,
                                    Arg::Value(pair_name) => names.push(pair_name.string()?),
                                    _ => return Err(arg.unexpected()),
                                }
                            }
                            Some(Command::Sync { dry_run })
                        }
                        "resolve-conflicts" => {
                            let mut dry_run = false;
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    Arg::Short('n') => dry_run = true,
                                    Arg::Value(pair_name) => names.push(pair_name.string()?),
                                    _ => return Err(arg.unexpected()),
                                }
                            }
                            Some(Command::ResolveConflicts { dry_run })
                        }
                        "discover" => {
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    Arg::Value(pair_name) => names.push(pair_name.string()?),
                                    _ => return Err(arg.unexpected()),
                                }
                            }
                            Some(Command::Discover)
                        }
                        "repair" => {
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    Arg::Value(name) => names.push(name.string()?),
                                    _ => return Err(arg.unexpected()),
                                }
                            }
                            Some(Command::Repair)
                        }
                        "version" => {
                            if let Some(arg) = parser.next()? {
                                return Err(arg.unexpected());
                            }
                            Some(Command::Version)
                        }
                        cmd => return Err(format!("Unknown command: {cmd}").into()),
                    };
                    break;
                }
                _ => return Err(arg.unexpected()),
            }
        }

        Ok(Cli {
            command: command.ok_or(lexopt::Error::from("No command specified"))?,
            log_level,
            config_file,
            names,
        })
    }
}
