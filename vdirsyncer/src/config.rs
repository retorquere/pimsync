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
    num::ParseIntError,
    os::unix::prelude::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::SystemTime,
};

use anyhow::{bail, Context};
use hyper::client::HttpConnector;
use hyper_rustls::{ConfigBuilderExt, HttpsConnector, HttpsConnectorBuilder};
use itertools::Itertools;
use libdav::auth::Password;
use rustls::{
    client::{ServerCertVerified, ServerCertVerifier},
    CertificateError, ClientConfig, RootCertStore,
};
use serde::Deserialize;
use vstorage::{
    base::{IcsItem, Item, Storage, VcardItem},
    caldav::{CalDavDefinition, CalDavStorage},
    carddav::{CardDavDefinition, CardDavStorage},
    filesystem::{FilesystemDefinition, FilesystemStorage},
    sync::declare::{CollectionDescription, DeclaredMapping, StoragePair, StoragePairBuilder},
    webcal::{WebCalDefinition, WebCalStorage},
    CollectionId,
};

use crate::tls::{
    cert_and_key_from_pemfile, certs_from_pemfile, key_from_pemfile, FingerprintAndWebPkiVerifier,
    FingerprintVerifier,
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

// TODO: the Config instance should be consumed when converting into Storages and Pairs.
//       this would reduce a lot of pointless cloning and copying values.
impl Config {
    /// Returns the `status_path`, expanding a leading tilde if present.
    pub(crate) fn status_path(&self) -> Cow<Path> {
        expand_tilde(&self.general.status_path)
    }

    pub(crate) async fn storages(
        &self,
    ) -> anyhow::Result<(
        HashMap<String, Arc<dyn Storage<IcsItem>>>,
        HashMap<String, Arc<dyn Storage<VcardItem>>>,
    )> {
        let mut calendars = HashMap::new();
        let mut address_books = HashMap::new();

        for (name, source) in self.storages.iter() {
            match source.storage().await? {
                EitherStorage::Calendar(c) => {
                    calendars.insert(name.clone(), c);
                }
                EitherStorage::AddressBook(a) => {
                    address_books.insert(name.clone(), a);
                }
            }
        }

        Ok((calendars, address_books))
    }

    pub(crate) fn pairs<'storages>(
        &self,
        calendars: &'storages HashMap<String, Arc<dyn Storage<IcsItem>>>,
        contacts: &'storages HashMap<String, Arc<dyn Storage<VcardItem>>>,
        // TODO: the "previous state" is required here.
    ) -> anyhow::Result<(
        Vec<StoragePair<'storages, IcsItem>>,
        Vec<StoragePair<'storages, VcardItem>>,
    )> {
        let mut calendar_pairs = Vec::new(); // TODO: with_capacity?
        let mut contact_pairs = Vec::new(); // TODO: with_capacity?

        for (name, source) in self.pairs.iter() {
            match (calendars.get(&source.a), calendars.get(&source.b)) {
                (None, None) => {
                    match (contacts.get(&source.a), contacts.get(&source.b)) {
                        (None, None) => {
                            bail!("Pair {} is missing both storages.", name);
                        }
                        (None, Some(_)) => {
                            bail!("Pair {} is missing contacts storage A.", name);
                        }
                        (Some(_), None) => {
                            bail!("Pair {} is missing contacts storage B.", name);
                        }
                        (Some(a), Some(b)) => {
                            contact_pairs.push(create_pair(name, source, a, b));
                        }
                    };
                }
                (None, Some(_)) => {
                    bail!("Pair {} is missing calendar storage A.", name);
                }
                (Some(_), None) => {
                    bail!("Pair {} is missing calendar storage B.", name);
                }
                (Some(a), Some(b)) => {
                    calendar_pairs.push(create_pair(name, source, a, b));
                }
            }
        }
        Ok((calendar_pairs, contact_pairs))
    }
}

fn create_pair<'a, I: Item>(
    name: &str,
    source: &PairSection,
    a: &Arc<dyn Storage<I>>,
    b: &Arc<dyn Storage<I>>,
) -> StoragePair<'a, I> {
    let mut pair = StoragePair::builder(a.clone(), b.clone());
    for cv in &source.collections {
        pair = match cv {
            CollectionValue::All => pair.with_all_from_a().with_all_from_b(),
            CollectionValue::FromA => pair.with_all_from_a(),
            CollectionValue::FromB => pair.with_all_from_b(),
            CollectionValue::Mapped(alias, a, b) => {
                let mapping = DeclaredMapping::Mapped {
                    alias: alias.clone(),
                    a: a.to_description(),
                    b: b.to_description(),
                };
                pair.with_mapping(mapping)
            }
            CollectionValue::Collection(col) => pair.with_mapping(col.to_mapping()),
        };
    }
    pair.build()
}

fn expand_tilde(orig: &PathBuf) -> Cow<Path> {
    let mut iter = orig.as_path().as_os_str().as_bytes().iter();
    if let Some(b'~') = iter.next() {
        if let Some(b'/') = iter.next() {
            #[allow(deprecated)] // Only problematic on unsupported platforms.
            let home = std::env::home_dir().expect("must resolve home path to expand tilde");
            let home = home.into_os_string().into_vec().into_iter();
            let all = home.chain(std::iter::once(b'/')).chain(iter.copied());
            let os_string = OsString::from_vec(all.collect::<Vec<_>>());
            return Cow::Owned(PathBuf::from(os_string));
        }
    }
    Cow::Borrowed(&orig)
}

#[derive(Deserialize, Debug)]
pub(crate) struct GeneralSection {
    status_path: PathBuf,
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
enum Collection {
    // TODO: can I re-use vstorage::sync::declare::CollectionDescription here?
    #[serde(rename = "id")]
    Id(CollectionId),
    #[serde(rename = "href")]
    Href(String),
}

impl Collection {
    // TODO: these would be less inefficient if they consumed their input.

    fn to_description(&self) -> CollectionDescription {
        match self {
            Collection::Id(id) => CollectionDescription::Id { id: id.clone() },
            Collection::Href(href) => CollectionDescription::Href { href: href.clone() },
        }
    }

    fn to_mapping(&self) -> DeclaredMapping {
        match self {
            Collection::Id(id) => DeclaredMapping::Direct {
                description: CollectionDescription::Id { id: id.clone() },
            },
            Collection::Href(href) => DeclaredMapping::Direct {
                description: CollectionDescription::Href { href: href.clone() },
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

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum StorageSection {
    // TODO: a "protect" flag to protect one side if EVERYTHING is about to be deleted:
    // - off
    // - items: refuses to operate if any item would be deleted.
    // - collection: refuses to operate if a non-empty collection would be emptied or deleted.
    // - storage: refuses to operate if ALL collections would be emptied or deleted.
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
    Calendar(Arc<dyn Storage<IcsItem>>),
    AddressBook(Arc<dyn Storage<VcardItem>>),
}

impl StorageSection {
    pub(crate) async fn storage(&self) -> anyhow::Result<EitherStorage> {
        Ok(match self {
            StorageSection::FilesystemIcalendar(def) => {
                EitherStorage::Calendar(Arc::new(def.to_storage()))
            }
            StorageSection::FilesystemVcard(def) => {
                EitherStorage::AddressBook(Arc::new(def.to_storage()))
            }
            StorageSection::CardDav(carddav) => {
                EitherStorage::AddressBook(Arc::new(carddav.to_storage().await?))
            }
            StorageSection::CalDav(caldav) => {
                EitherStorage::Calendar(Arc::new(caldav.to_storage().await?))
            }
            StorageSection::Http(http) => EitherStorage::Calendar(Arc::new(http.to_storage()?)),
        })
    }

    // If this is a calendar, return the Storage.
    //
    // - Returns None if no storage matches this type.
    // - Returns Some(_) if a storage matches.
    pub(crate) async fn calendar_storage(
        &self,
    ) -> anyhow::Result<Option<Arc<dyn Storage<IcsItem>>>> {
        Ok(match self {
            StorageSection::FilesystemIcalendar(def) => Some(Arc::new(def.to_storage())),
            StorageSection::FilesystemVcard(_) => None,
            StorageSection::CardDav(_) => None,
            StorageSection::CalDav(caldav) => Some(Arc::new(caldav.to_storage().await?)),
            StorageSection::Http(http) => Some(Arc::new(http.to_storage()?)),
        })
    }

    // If this is a calendar, return the Storage.
    pub(crate) async fn contact_storage(
        &self,
    ) -> anyhow::Result<Option<Arc<dyn Storage<VcardItem>>>> {
        Ok(match self {
            StorageSection::FilesystemIcalendar(_) => None,
            StorageSection::FilesystemVcard(def) => Some(Arc::new(def.to_storage())),
            StorageSection::CardDav(carddav) => Some(Arc::new(carddav.to_storage().await?)),
            StorageSection::CalDav(_) => None,
            StorageSection::Http(_) => None,
        })
    }
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

impl<I: Item> Filesystem<I> {
    fn to_storage(&self) -> FilesystemStorage<I> {
        let path = expand_tilde(&self.path);
        FilesystemDefinition::new(path.to_owned().to_path_buf(), self.fileext.clone()).build()
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
    async fn to_storage(&self) -> anyhow::Result<CardDavStorage<HttpsConnector<HttpConnector>>> {
        Ok(CardDavDefinition {
            url: self.url.to_string().parse()?,
            auth: libdav::auth::Auth::Basic {
                username: self.username.to_string()?,
                password: Some(self.password.to_password()?),
            },
            connector: self.network_opts.to_connector()?,
        }
        .build()
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
    async fn to_storage(&self) -> anyhow::Result<CalDavStorage<HttpsConnector<HttpConnector>>> {
        Ok(CalDavDefinition {
            url: self
                .url
                .to_string()?
                .parse()
                .context("parsing caldav URL")?,
            auth: libdav::auth::Auth::Basic {
                username: self.username.to_string()?,
                password: Some(self.password.to_password()?),
            },
            connector: self.network_opts.to_connector()?,
        }
        .build()
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
    fn to_storage(&self) -> anyhow::Result<WebCalStorage> {
        Ok(WebCalDefinition {
            url: self.url.to_string()?.parse()?,
            collection_name: self.collection.clone(),
        }
        .build()?)
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
    fn to_connector(&self) -> anyhow::Result<HttpsConnector<HttpConnector>> {
        let tls_config = ClientConfig::builder().with_safe_defaults();
        let tls_config = match (&self.verify, &self.verify_fingerprint) {
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

        let tls_config = match &self.auth_cert {
            None => tls_config.with_no_client_auth(),
            Some(cc) => {
                let (certs, key) = match cc {
                    ClientCert::SingleFile(combined_path) => {
                        cert_and_key_from_pemfile(combined_path)?
                    }
                    ClientCert::SeparateKeyAndCert(crt_path, key_path) => {
                        (certs_from_pemfile(crt_path)?, key_from_pemfile(key_path)?)
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
    String::from("vdirsyncer/2.0.0-alpha0") // FIXME: hard-coded version
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
    fn to_string(&self) -> anyhow::Result<String> {
        match self {
            StringOrFetch::Raw(s) => Ok(s.clone()),
            StringOrFetch::Fetch { fetch } => {
                // TODO: should expand user and normalise paths.
                let mut values = fetch.iter();
                if Some(&String::from("command")) != values.next() {
                    bail!("First word of a fetch directive must be 'command'")
                };
                let cmd = values.next().context("extracting command from 'fetch'")?;
                let output = Command::new(cmd)
                    .args(values)
                    .stdout(Stdio::piped())
                    .output()
                    .context("executing fetch command")?;
                Ok(std::str::from_utf8(&output.stdout)?.trim().to_owned())
            }
        }
    }

    fn to_password(&self) -> anyhow::Result<Password> {
        self.to_string().map(Password::from)
    }
}

/// Parse a configuration file at `path`.
pub(crate) fn parse_from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Config> {
    let raw = std::fs::read_to_string(path)?;
    let config: Config = toml::from_str(&raw)?;

    Ok(config)
}
