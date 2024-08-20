use lexopt::ValueExt as _;
use rustix::fd::FromRawFd as _;
use std::fs::File;

pub(crate) enum Command {
    Check,
    Daemon {
        ready_fd: Option<File>,
        pair: Option<String>,
    },
    Sync {
        dry_run: bool,
        pair: Option<String>,
    },
    ResolveConflicts {
        dry_run: bool,
        pair: Option<String>,
    },
    Discover,
    Version,
}

pub(crate) struct Cli {
    pub command: Command,
    pub log_level: log::LevelFilter,
}

impl Cli {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Cli, lexopt::Error> {
        let mut command = None;
        let mut log_level = log::LevelFilter::Warn;

        args.next(); // Skip arg0
        let mut parser = lexopt::Parser::from_args(args);
        while let Some(arg) = parser.next()? {
            match arg {
                lexopt::Arg::Short('v') => log_level = parser.value()?.parse()?,
                lexopt::Arg::Value(raw_cmd) => {
                    command = match raw_cmd.string()?.as_str() {
                        "check" => Some(Command::Check),
                        "daemon" => {
                            let mut pair = None;
                            let mut ready_fd = None;
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    lexopt::Arg::Short('r') => {
                                        let raw_fd = parser.value()?.parse()?;
                                        if raw_fd < 3 {
                                            return Err(
                                                "Readiness fd must be greater than 2".into()
                                            );
                                        }
                                        // SAFETY: this file descriptor is not accessed elsewhere.
                                        // The user is responsible for ensuring that they have
                                        // supplied a valid open file.
                                        ready_fd = Some(unsafe { File::from_raw_fd(raw_fd) });
                                    }
                                    lexopt::Arg::Value(raw_pair) => pair = Some(raw_pair.string()?),
                                    _ => return Err(arg.unexpected()),
                                };
                            }
                            Some(Command::Daemon { ready_fd, pair })
                        }
                        "sync" => {
                            let mut pair = None;
                            let mut dry_run = false;
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    lexopt::Arg::Short('d') => dry_run = true,
                                    lexopt::Arg::Value(raw_pair) => pair = Some(raw_pair.string()?),
                                    _ => return Err(arg.unexpected()),
                                };
                            }
                            Some(Command::Sync { pair, dry_run })
                        }
                        "resolve-conflicts" => {
                            let mut pair = None;
                            let mut dry_run = false;
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    lexopt::Arg::Short('d') => dry_run = true,
                                    lexopt::Arg::Value(raw_pair) => pair = Some(raw_pair.string()?),
                                    _ => return Err(arg.unexpected()),
                                };
                            }
                            Some(Command::ResolveConflicts { pair, dry_run })
                        }
                        "discover" => Some(Command::Discover),
                        "version" => Some(Command::Version),
                        cmd => return Err(format!("Unknown command: {cmd}").into()),
                    };
                    break;
                }
                _ => return Err(arg.unexpected()),
            };
        }

        Ok(Cli {
            command: command.ok_or(lexopt::Error::from("No command specified"))?,
            log_level,
        })
    }
}
