use lexopt::ValueExt as _;

pub(crate) enum Command {
    Check,
    Daemon { pair: Option<String> },
    Sync { dry_run: bool, pair: Option<String> },
    ResolveConflicts { dry_run: bool, pair: Option<String> },
    Discover,
    Version,
}

pub(crate) struct Cli {
    pub command: Command,
    pub log_level: log::Level,
}

impl Cli {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Cli, lexopt::Error> {
        let mut verbose = 0;
        let mut command = None;

        args.next(); // Skip arg0
        let mut parser = lexopt::Parser::from_args(args);
        while let Some(arg) = parser.next()? {
            match arg {
                lexopt::Arg::Short('v') => verbose += 1,
                lexopt::Arg::Value(raw_cmd) => {
                    command = match raw_cmd.string()?.as_str() {
                        "check" => Some(Command::Check),
                        "daemon" => {
                            let mut pair = None;
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    lexopt::Arg::Short('v') => verbose += 1,
                                    lexopt::Arg::Value(raw_pair) => pair = Some(raw_pair.string()?),
                                    _ => return Err(arg.unexpected()),
                                };
                            }
                            Some(Command::Daemon { pair })
                        }
                        "sync" => {
                            let mut pair = None;
                            let mut dry_run = false;
                            while let Some(arg) = parser.next()? {
                                match arg {
                                    lexopt::Arg::Short('v') => verbose += 1,
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
                                    lexopt::Arg::Short('v') => verbose += 1,
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

        let log_level = match verbose {
            0 => log::Level::Warn,
            1 => log::Level::Info,
            2 => log::Level::Debug,
            3 => log::Level::Trace,
            _ => log::Level::max(),
        };

        Ok(Cli {
            command: command.ok_or(lexopt::Error::from("No command specified"))?,
            log_level,
        })
    }
}
