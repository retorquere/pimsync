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
    process::{Command, Stdio},
    sync::Arc,
    time::SystemTime,
};

use anyhow::{bail, Context};
use camino::{Utf8Path, Utf8PathBuf};
use hyper::client::HttpConnector;
use hyper_rustls::{ConfigBuilderExt, HttpsConnector, HttpsConnectorBuilder};
use libdav::auth::Password;
use log::debug;
use rustls::{ClientConfig, RootCertStore};
use serde::Deserialize;
use vstorage::{
    base::{IcsItem, Item, Storage, VcardItem},
    caldav::CalDavStorage,
    carddav::CardDavStorage,
    filesystem::FilesystemStorage,
    sync::{
        declare::{CollectionDescription, DeclaredMapping, StoragePair},
        state::PairState,
    },
    webcal::WebCalStorage,
    CollectionId,
};

use crate::{
    tls::{
        cert_and_key_from_pemfile, certs_from_pemfile, key_from_pemfile,
        FingerprintAndWebPkiVerifier, FingerprintVerifier,
    },
    App, NamedPair, NamedStorage, VERSION,
};

/// A deserialised configuration file.
#[derive(Deserialize, Debug)]
pub(crate) struct Config {
    general: GeneralSection,
    #[serde(rename = "pair")]
    pairs: HashMap<String, PairSection>,
    #[serde(rename = "storage")]
    storages: HashMap<String, StorageSection>,
}

impl Config {
    /// Convert this configuration into an `App` instance.
    ///
    /// This consumes the configuration to avoid copying any data needlessly and freeing up any
    /// unnecessary data.
    pub(crate) async fn into_app<'storages>(self) -> anyhow::Result<App> {
        let status_dir = expand_tilde(self.general.status_path)
            .context("error expanding tilde for status_dir")?;
        // Initialise storages once, to avoid duplicating any.
        // TODO: do this in parallel: https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html
        let storages = {
            let mut storages = Vec::with_capacity(self.storages.len());
            for (name, source) in self.storages {
                storages.push(source.into_storage(name).await?);
            }
            storages
        };

        let mut calendar_pairs = Vec::new();
        let mut contact_pairs = Vec::new();

        for (name, source) in self.pairs {
            let a = storages
                .iter()
                .find(|s| s.name() == source.a)
                .with_context(|| {
                    format!("pair {} refers to undefined storage {}.", name, source.a)
                })?;
            let b = storages
                .iter()
                .find(|s| s.name() == source.b)
                .with_context(|| {
                    format!("pair {} refers to undefined storage {}.", name, source.b)
                })?;

            match (a, b) {
                (EitherStorage::Calendar(a), EitherStorage::Calendar(b)) => {
                    let pair = source.try_into_named_pair(
                        name,
                        a.inner.clone(),
                        b.inner.clone(),
                        &status_dir,
                    );
                    calendar_pairs.push(pair);
                }
                (EitherStorage::Calendar(_), EitherStorage::AddressBook(_)) => {
                    bail!("pair {} mixes calendar storage with contacts storage", name)
                }
                (EitherStorage::AddressBook(_), EitherStorage::Calendar(_)) => {
                    bail!("pair {} mixes contacts storage with calendar storage", name)
                }
                (EitherStorage::AddressBook(a), EitherStorage::AddressBook(b)) => {
                    let pair = source.try_into_named_pair(
                        name,
                        a.inner.clone(),
                        b.inner.clone(),
                        &status_dir,
                    );
                    contact_pairs.push(pair);
                }
            }
        }

        Ok(App {
            calendar_pairs,
            contact_pairs,
        })
    }
}

/// # Errors
///
/// If this path starts with tilde AND the home directory is non-UTF8.
fn expand_tilde(orig: Utf8PathBuf) -> Result<Utf8PathBuf, camino::FromPathBufError> {
    let mut iter = orig.as_str().chars();
    if let Some('~') = iter.next() {
        if let Some('/') = iter.next() {
            #[allow(deprecated)] // Only problematic on unsupported platforms.
            let home = std::env::home_dir().expect("must resolve home path to expand tilde");
            let home = Utf8PathBuf::try_from(home)?;
            let rest = iter.collect::<String>();
            return Ok(home.join(rest));
        }
    }
    Ok(orig)
}

/// The "general" section of the parsed configuration file
#[derive(Deserialize, Debug)]
pub(crate) struct GeneralSection {
    status_path: Utf8PathBuf,
}

/// A "pair" section of the parsed configuration file
#[derive(Deserialize, Debug)]
struct PairSection {
    a: String,
    b: String,
    collections: Collections,
    metadata: Option<Vec<String>>,
    // TODO: conflict_resolution: Option<Vec<String>>,
    // TODO: partial_sync
}

impl PairSection {
    fn try_into_named_pair<I: Item>(
        self,
        name: String,
        a: Arc<dyn Storage<I>>,
        b: Arc<dyn Storage<I>>,
        status_dir: &Utf8Path,
    ) -> NamedPair<I> {
        let status_path = status_dir.join(format!("{name}.status"));

        let mut pair = StoragePair::builder(a, b);

        match self.collections {
            Collections::All => {
                pair = pair.with_all_from_a().with_all_from_b();
            }
            Collections::Mappings(mappings) => {
                for cv in mappings {
                    pair = match cv {
                        CollectionValue::FromA => pair.with_all_from_a(),
                        CollectionValue::FromB => pair.with_all_from_b(),
                        CollectionValue::Mapped(alias, a, b) => {
                            let mapping = DeclaredMapping::Mapped {
                                alias,
                                a: a.into_description(),
                                b: b.into_description(),
                            };
                            pair.with_mapping(mapping)
                        }
                        CollectionValue::Collection(col) => pair.with_mapping(col.into_mapping()),
                    };
                }
            }
        }

        NamedPair {
            name,
            inner: pair.build(),
            status_path,
        }
    }
}

#[derive(Deserialize, Debug)]
enum Collections {
    #[serde(rename = "all")]
    All,
    #[serde(untagged)]
    Mappings(Vec<CollectionValue>),
}

#[derive(Deserialize, Debug)]
enum CollectionValue {
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
enum Collection {
    #[serde(rename = "id")]
    Id(CollectionId),
    #[serde(rename = "href")]
    Href(String),
}

impl Collection {
    fn into_description(self) -> CollectionDescription {
        match self {
            Collection::Id(id) => CollectionDescription::Id { id },
            Collection::Href(href) => CollectionDescription::Href { href },
        }
    }

    fn into_mapping(self) -> DeclaredMapping {
        match self {
            Collection::Id(id) => DeclaredMapping::Direct {
                description: CollectionDescription::Id { id },
            },
            Collection::Href(href) => DeclaredMapping::Direct {
                description: CollectionDescription::Href { href },
            },
        }
    }
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

/// A "storage" section of the parsed configuration file
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum StorageSection {
    // TODO: a "protect" flag to protect one side if EVERYTHING is about to be deleted:
    // - off
    // - items: refuses to operate if any item would be deleted.
    // - collection: refuses to operate if a non-empty collection would be emptied or deleted.
    // - storage: refuses to operate if ALL collections would be emptied or deleted.
    // TODO: changelog MUST mention the change in default behaviour here.
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

enum EitherStorage {
    Calendar(NamedStorage<IcsItem>),
    AddressBook(NamedStorage<VcardItem>),
}

impl EitherStorage {
    fn name(&self) -> &str {
        match self {
            EitherStorage::Calendar(c) => &c.name,
            EitherStorage::AddressBook(a) => &a.name,
        }
    }
}

impl StorageSection {
    pub(crate) async fn into_storage(self, name: String) -> anyhow::Result<EitherStorage> {
        Ok(match self {
            StorageSection::FilesystemIcalendar(def) => {
                let inner = Arc::new(def.into_storage()?);
                EitherStorage::Calendar(NamedStorage { name, inner })
            }
            StorageSection::FilesystemVcard(def) => {
                let inner = Arc::new(def.into_storage()?);
                EitherStorage::AddressBook(NamedStorage { name, inner })
            }
            StorageSection::CardDav(carddav) => {
                let inner = Arc::new(carddav.into_storage().await?);
                EitherStorage::AddressBook(NamedStorage { name, inner })
            }
            StorageSection::CalDav(caldav) => {
                let inner = Arc::new(caldav.into_storage().await?);
                EitherStorage::Calendar(NamedStorage { name, inner })
            }
            StorageSection::Http(http) => {
                let inner = Arc::new(http.into_storage()?);
                EitherStorage::Calendar(NamedStorage { name, inner })
            }
        })
    }
}

#[derive(Deserialize, Debug)]
struct Filesystem<I: Item> {
    path: Utf8PathBuf,
    fileext: String,
    // TODO: encoding
    // TODO: post_hook
    // TODO: fileignoreext
    post_hook: Option<OsString>,
    #[serde(default)]
    item: PhantomData<I>,
}

impl<I: Item> Filesystem<I> {
    fn into_storage(self) -> anyhow::Result<FilesystemStorage<I>> {
        let path = expand_tilde(self.path).context("error expanding tilde for storage")?;
        // v0.X series expected the leading string. This is not ideal and should be deprecated.
        let fileext = self
            .fileext
            .strip_prefix('.')
            .unwrap_or(&self.fileext)
            .to_string();
        Ok(FilesystemStorage::new(path, fileext))
    }
}

#[derive(Deserialize, Debug)]
struct CardDav {
    url: String,
    username: StringOrFetch,
    password: StringOrFetch,
    #[serde(flatten)]
    network_opts: HttpsConfig,
}

impl CardDav {
    async fn into_storage(self) -> anyhow::Result<CardDavStorage<HttpsConnector<HttpConnector>>> {
        Ok(CardDavStorage::new(
            self.url.parse()?,
            libdav::auth::Auth::Basic {
                username: self.username.into_string()?,
                password: Some(self.password.into_password()?),
            },
            self.network_opts.into_connector()?,
        )
        .await?)
    }
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
    network_opts: HttpsConfig,
}

impl CalDav {
    async fn into_storage(self) -> anyhow::Result<CalDavStorage<HttpsConnector<HttpConnector>>> {
        Ok(CalDavStorage::new(
            self.url
                .into_string()?
                .parse()
                .context("parsing caldav URL")?,
            libdav::auth::Auth::Basic {
                username: self.username.into_string()?,
                password: Some(self.password.into_password()?),
            },
            self.network_opts.into_connector()?,
        )
        .await?)
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct Http {
    url: StringOrFetch,
    /// A name for the single collection inside this storage.
    collection: CollectionId,
    #[serde(flatten)]
    https_config: HttpsConfig,
}

impl Http {
    fn into_storage(self) -> anyhow::Result<WebCalStorage> {
        Ok(WebCalStorage::new(
            self.url.into_string()?.parse()?,
            self.collection,
        )?)
    }
}

#[derive(Deserialize, Debug)]
struct HttpsConfig {
    verify: Option<PathBuf>,
    verify_fingerprint: Option<String>,
    #[serde(default)]
    auth: Auth,
    auth_cert: Option<ClientCert>,
    #[serde(default = "default_useragent")]
    useragent: String,
}

impl HttpsConfig {
    // TODO: keep a global cache using the hash of these.
    //       this would allow re-using the same TLS store for all clients.
    fn into_connector(self) -> anyhow::Result<HttpsConnector<HttpConnector>> {
        let tls_config = ClientConfig::builder().with_safe_defaults();
        let tls_config = match (self.verify, self.verify_fingerprint) {
            (None, None) => tls_config
                .with_native_roots()
                .with_certificate_transparency_logs(&[], SystemTime::now()),
            (None, Some(fingerprint)) => {
                let verifier = Arc::from(FingerprintVerifier::new(&fingerprint)?);
                tls_config.with_custom_certificate_verifier(verifier)
            }
            (Some(path), None) => {
                let mut root_store = RootCertStore::empty();
                for cert in certs_from_pemfile(&path)? {
                    root_store.add(&cert)?;
                }
                tls_config
                    .with_root_certificates(root_store)
                    .with_certificate_transparency_logs(&[], SystemTime::now())
            }
            (Some(path), Some(fingerprint)) => {
                let mut root_store = RootCertStore::empty();
                for cert in certs_from_pemfile(&path)? {
                    root_store.add(&cert)?;
                }
                let verifier =
                    Arc::from(FingerprintAndWebPkiVerifier::new(&fingerprint, root_store)?);
                tls_config.with_custom_certificate_verifier(verifier)
            }
        };

        let tls_config = match self.auth_cert {
            None => tls_config.with_no_client_auth(),
            Some(cc) => {
                let (certs, key) = match cc {
                    ClientCert::SingleFile(combined_path) => {
                        cert_and_key_from_pemfile(&combined_path)?
                    }
                    ClientCert::SeparateKeyAndCert(crt_path, key_path) => {
                        (certs_from_pemfile(&crt_path)?, key_from_pemfile(&key_path)?)
                    }
                };
                tls_config.with_client_auth_cert(certs, key)?
            }
        };

        Ok(HttpsConnectorBuilder::new()
            .with_tls_config(tls_config)
            .https_or_http()
            .enable_http1()
            .build())
    }
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
    String::from(VERSION)
}

#[derive(Deserialize, Debug)]
enum ClientCert {
    SingleFile(PathBuf),
    SeparateKeyAndCert(PathBuf, PathBuf),
}

// TODO: singlefile

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum StringOrFetch {
    Raw(String),
    // TODO: evaluate whether I want 'shell' or 'prompt'.
    Fetch { fetch: Vec<String> },
}

impl StringOrFetch {
    fn into_string(self) -> anyhow::Result<String> {
        match self {
            StringOrFetch::Raw(s) => Ok(s),
            StringOrFetch::Fetch { fetch } => {
                // TODO: should expand user and normalise paths.
                let mut values = fetch.into_iter();
                if Some(String::from("command")) != values.next() {
                    bail!("First word of a fetch directive must be 'command'")
                };
                let cmd = values.next().context("extracting command from 'fetch'")?;
                let output = Command::new(cmd)
                    .args(values)
                    .stdout(Stdio::piped())
                    .output()
                    .context("executing fetch command")?;
                match output.status.code() {
                    Some(0) => Ok(std::str::from_utf8(&output.stdout)?.trim().to_owned()),
                    Some(code) => bail!("Fetch command exited with status {}.", code),
                    None => bail!("Fetch command exited unexpectedly."),
                }
            }
        }
    }

    fn into_password(self) -> anyhow::Result<Password> {
        let string = self.into_string()?;
        if string.is_empty() {
            bail!("Fetch returned an empty password. This is likely a misconfiguration.")
        }
        Ok(Password::from(string))
    }
}

/// Parse a configuration file at `path`.
pub(crate) fn parse_from_file(path: impl AsRef<Path>) -> anyhow::Result<Config> {
    let raw = std::fs::read_to_string(path)?;
    let config: Config = toml::from_str(&raw)?;

    Ok(config)
}
