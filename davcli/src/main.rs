#![deny(clippy::pedantic)]
#![deny(clippy::unwrap_used)]

// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use clap::Parser;

mod caldav;
mod carddav;
mod cli;

fn main() -> anyhow::Result<()> {
    // TODO: also support email as input?
    let cli = cli::Cli::parse();
    simple_logger::init_with_level(cli.log_level()).expect("logger configuration is valid");

    cli.execute()
}
