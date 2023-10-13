// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Types for parsing the configuration file.
#![allow(unused)]

use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsString,
    marker::PhantomData,
    os::unix::prelude::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use vstorage::{
    base::{IcsItem, Item, VcardItem},
    filesystem::FilesystemStorage,
};

#[derive(Deserialize, Debug)]
pub struct Config {
    general: GeneralSection,
    pair: HashMap<String, PairSection>,
    storage: HashMap<String, StorageSection>,
}

#[derive(Deserialize, Debug)]
struct GeneralSection {
    status_path: PathBuf,
}

impl GeneralSection {
    /// Returns the `status_path`, expanding a leading tilde if present.
    pub fn status_path(&self) -> Cow<Path> {
        let mut iter = self.status_path.as_path().as_os_str().as_bytes().iter();
        if let Some(b'~') = iter.next() {
            if let Some(b'/') = iter.next() {
                #[allow(deprecated)] // Only imperfect on unsupported platforms.
                let home = std::env::home_dir().expect("must resolve home path to expand tilde");
                let home = home.into_os_string().into_vec().into_iter();
                let all = home.chain(iter.copied());
                let os_string = OsString::from_vec(all.collect::<Vec<_>>());
                return Cow::Owned(PathBuf::from(os_string));
            }
        }
        Cow::Borrowed(&self.status_path)
    }
}

#[derive(Deserialize, Debug)]
struct PairSection {
    a: String,
    b: String,
    collections: Vec<CollectionValue>,
    metadata: Option<Vec<String>>,
    // TODO: conflict_resolution: Option<Vec<String>>,
    // TODO: partial_sync
}

#[derive(Deserialize, Debug)]
enum CollectionValue {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "from a")]
    FromA,
    #[serde(rename = "from b")]
    FromB,
    #[serde(rename = "mapped")]
    Mapped(String, Collection, Collection),
    #[serde(untagged)]
    Collection(Collection),
}

#[derive(Deserialize, Debug)]
pub enum Collection {
    #[serde(rename = "id")]
    Id(String),
    #[serde(rename = "href")]
    Href(String),
}

#[derive(Deserialize, Debug)]
enum CollectionSpecial {
    #[serde(rename = "all")]
    All,
    #[serde(rename = "from a")]
    FromA,
    #[serde(rename = "from b")]
    FromB,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum StorageSection {
    #[serde(rename = "filesystem/icalendar")]
    FilesystemIcalendar(Filesystem<IcsItem>),

    #[serde(rename = "filesystem/vcard")]
    FilesystemVcard(Filesystem<VcardItem>),

    #[serde(rename = "carddav")]
    CardDav(CardDav),

    #[serde(rename = "caldav")]
    CalDav(CalDav),

    #[serde(rename = "http")]
    Http(Http),
}

#[derive(Deserialize, Debug)]
struct Filesystem<I: Item> {
    path: PathBuf,
    fileext: String,
    // TODO: encoding
    // TODO: post_hook
    // TODO: fileignoreext
    post_hook: Option<OsString>,
    #[serde(default)]
    item: PhantomData<I>,
}

#[derive(Deserialize, Debug)]
struct CardDav {
    url: String,
    username: StringOrFetch,
    password: StringOrFetch,
    #[serde(flatten)]
    network_opts: NetworkOptions,
}

#[derive(Deserialize, Debug)]
struct CalDav {
    url: StringOrFetch,
    username: StringOrFetch,
    password: StringOrFetch,
    // TODO: start_date
    // TODO: end_date
    // TODO: item_types
    #[serde(flatten)]
    network_opts: NetworkOptions,
}

#[derive(Deserialize, Debug)]
pub struct Http {
    url: StringOrFetch,
    /// A name for the single collection inside this storage.
    collection: String,
    #[serde(flatten)]
    network_opts: NetworkOptions,
}

#[derive(Deserialize, Debug)]
struct NetworkOptions {
    verify: Option<PathBuf>,
    verify_fingerprint: Option<String>,
    #[serde(default)]
    auth: Auth,
    // TODO: auth_cert
    #[serde(default = "default_useragent")]
    useragent: String,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "lowercase")]
enum Auth {
    #[default]
    Basic,
    Digest,
    Guess,
}

fn default_useragent() -> String {
    String::from("vdirsyncer/2.0.0-alpha0") // FIXME: hard-coded version
}

// TODO: singlefile

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum StringOrFetch {
    Raw(String),
    Fetch(Fetch),
}

#[derive(Deserialize, Debug)]
struct Fetch {
    // Note: 'shell' and 'prompt' have been dropped.
    fetch: Vec<String>,
}

pub fn parse_from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Config> {
    let raw = std::fs::read_to_string(path)?;
    let config: Config = toml::from_str(&raw)?;

    Ok(config)
}

impl<I: Item> Filesystem<I> {
    fn build_storage(&self) -> FilesystemStorage<I> {
        todo!() // TODO
    }
}
