use lexopt::{Arg, Parser, ValueExt as _};
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
        let mut parser = Parser::from_args(args);
        while let Some(arg) = parser.next()? {
            match arg {
                Arg::Short('v') => log_level = parser.value()?.parse()?,
                Arg::Short('c') => config_file = Some(parser.value()?.string()?),
                Arg::Value(raw_cmd) => {
                    command = Some(match raw_cmd.string()?.as_str() {
                        "check" => parse_check(&mut parser, &mut names)?,
                        "daemon" => parse_daemon(&mut parser, &mut names)?,
                        "sync" => parse_sync(&mut parser, &mut names)?,
                        "resolve-conflicts" => parse_resolve_conflicts(&mut parser, &mut names)?,
                        "discover" => parse_discover(&mut parser, &mut names)?,
                        "repair" => parse_repair(&mut parser, &mut names)?,
                        "version" => parse_version(&mut parser)?,
                        cmd => return Err(format!("Unknown command: {cmd}").into()),
                    });
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

fn parse_check(parser: &mut Parser, names: &mut FilterNames) -> Result<Command, lexopt::Error> {
    while let Some(arg) = parser.next()? {
        match arg {
            Arg::Value(pair_name) => names.push(pair_name.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(Command::Check)
}

fn parse_daemon(parser: &mut Parser, names: &mut FilterNames) -> Result<Command, lexopt::Error> {
    let mut ready_fd = None;
    while let Some(arg) = parser.next()? {
        match arg {
            Arg::Short('r') => {
                let raw_fd = parser.value()?.parse()?;
                if raw_fd == 2 {
                    return Err("Cannot use stderr as readiness fd".into());
                }
                // SAFETY: file descriptor is not accessed elsewhere.
                // User is responsible for supplying a valid open file descriptor.
                ready_fd = Some(unsafe { File::from_raw_fd(raw_fd) });
            }
            Arg::Value(pair_name) => names.push(pair_name.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(Command::Daemon { ready_fd })
}

fn parse_sync(parser: &mut Parser, names: &mut FilterNames) -> Result<Command, lexopt::Error> {
    let mut dry_run = false;
    while let Some(arg) = parser.next()? {
        match arg {
            Arg::Short('n') => dry_run = true,
            Arg::Value(pair_name) => names.push(pair_name.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(Command::Sync { dry_run })
}

fn parse_resolve_conflicts(
    parser: &mut Parser,
    names: &mut FilterNames,
) -> Result<Command, lexopt::Error> {
    let mut dry_run = false;
    while let Some(arg) = parser.next()? {
        match arg {
            Arg::Short('n') => dry_run = true,
            Arg::Value(pair_name) => names.push(pair_name.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(Command::ResolveConflicts { dry_run })
}

fn parse_discover(parser: &mut Parser, names: &mut FilterNames) -> Result<Command, lexopt::Error> {
    while let Some(arg) = parser.next()? {
        match arg {
            Arg::Value(pair_name) => names.push(pair_name.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(Command::Discover)
}

fn parse_repair(parser: &mut Parser, names: &mut FilterNames) -> Result<Command, lexopt::Error> {
    while let Some(arg) = parser.next()? {
        match arg {
            Arg::Value(name) => names.push(name.string()?),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(Command::Repair)
}

fn parse_version(parser: &mut Parser) -> Result<Command, lexopt::Error> {
    if let Some(arg) = parser.next()? {
        return Err(arg.unexpected());
    }
    Ok(Command::Version)
}
