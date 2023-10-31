// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

mod config;
mod tls;

fn main() -> anyhow::Result<()> {
    let config = config::parse_from_file("/home/hugo/.config/vdirsyncer/config.toml")?;

    dbg!(config);
    todo!();
}
