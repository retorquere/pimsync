// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

#![allow(unused)]

use std::sync::Arc;

use anyhow::Context;
use log::debug;
use vstorage::{
    base::{IcsItem, Item, Storage, VcardItem},
    sync::declare::StoragePair,
};

mod config;
mod tls;

/// Storage with a name, as defined in the configuration file.
pub(crate) struct NamedStorage<I: Item> {
    name: String,
    // TODO: this should be wrapped in a Mutex. Once we allow pairs to synchronise concurrently,
    // this will avoid the same pair being re-used.
    inner: Arc<dyn Storage<I>>,
}

/// Pair with a name, as defined in the configuration file.
pub(crate) struct NamedPair<I: Item> {
    name: String,
    inner: StoragePair<I>,
}

pub(crate) struct App {
    // TODO: this also needs a global Mutex, which will be taken when trying to take locks on
    // individual storages.
    calendar_pairs: Vec<NamedPair<IcsItem>>,
    contact_pairs: Vec<NamedPair<VcardItem>>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let log_level = log::Level::Debug;
    simple_logger::init_with_level(log_level).expect("logger should initialise");

    let config = config::parse_from_file("/home/hugo/.config/vdirsyncer/config.toml")?;
    debug!("Parsed configuration: {:?}", &config);

    let _app = config
        .into_app()
        .await
        .context("Failed to initialise with given configuration.")?;
    debug!("Initialised application");

    todo!();
}
