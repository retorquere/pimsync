// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    fs::File,
    io::Read,
    marker::PhantomData,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, Context};
use camino::{Utf8Path, Utf8PathBuf};
use hyper_rustls::{ConfigBuilderExt, HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::connect::HttpConnector;
use libdav::auth::Password;
use log::{debug, error};
use rustls::{client::danger::DangerousClientConfigBuilder, ClientConfig, RootCertStore};
use serde::{Deserialize, Deserializer};
use vstorage::{
    base::{IcsItem, Item, Storage, VcardItem},
    caldav::CalDavStorage,
    carddav::CardDavStorage,
    sync::declare::{CollectionDescription, DeclaredMapping, StoragePair},
    vdir::VdirStorage,
    webcal::WebCalStorage,
    CollectionId,
};

use crate::{
    stdio::StdIo,
    tls::{
        cert_and_key_from_pemfile, certs_from_pemfile, key_from_pemfile,
        FingerprintAndWebPkiVerifier, FingerprintVerifier,
    },
    App, NamedPair, NamedStorage, RawCommand, VERSION,
};

/// A deserialised configuration file.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
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
            // Cannot pop a from storages; it might needed for another pair.
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
                    )?;
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
                    )?;
                    contact_pairs.push(pair);
                }
            }
        }

        Ok(App {
            calendar_pairs,
            contact_pairs,
            interval: Duration::from_secs(self.general.interval),
            stdio: Arc::new(StdIo::new()),
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
#[serde(deny_unknown_fields)]
pub(crate) struct GeneralSection {
    status_path: Utf8PathBuf,
    /// In seconds. Used when storages do not implement or support monitoring.
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_interval() -> u64 {
    300
}

/// A "pair" section of the parsed configuration file
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct PairSection {
    a: String,
    b: String,
    collections: Collections,
    #[allow(dead_code)]
    metadata: Option<Vec<String>>,
    conflict_resolution: Option<VecDeque<String>>,
    // TODO: partial_sync
}

impl PairSection {
    fn try_into_named_pair<I: Item>(
        self,
        name: String,
        a: Arc<dyn Storage<I>>,
        b: Arc<dyn Storage<I>>,
        status_dir: &Utf8Path,
    ) -> anyhow::Result<NamedPair<I>> {
        let status_path = status_dir.join(format!("{name}.status"));

        let mut pair = StoragePair::new(a, b);

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

        let conflict_resolution = match self.conflict_resolution {
            Some(args) => {
                let mut args: VecDeque<_> = args.into_iter().map(OsString::from).collect();
                Some(RawCommand {
                    command: args
                        .pop_front()
                        .context("conflict_resolution must specify a command")?,
                    args: args.into(),
                })
            }
            None => None,
        };

        Ok(NamedPair {
            name,
            inner: pair,
            status_path,
            conflict_resolution,
        })
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
enum Collections {
    #[serde(rename = "all")]
    All,
    #[serde(untagged)]
    Mappings(Vec<CollectionValue>),
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
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

fn deserialise_collection_id<'de, D>(deserializer: D) -> Result<CollectionId, D::Error>
where
    D: Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    CollectionId::try_from(s).map_err(serde::de::Error::custom)
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
enum Collection {
    #[serde(rename = "id", deserialize_with = "deserialise_collection_id")]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
enum StorageSection {
    // TODO: a "protect" flag to protect one side if EVERYTHING is about to be deleted:
    // - off
    // - items: refuses to operate if any item would be deleted.
    // - collection: refuses to operate if a non-empty collection would be emptied or deleted.
    // - storage: refuses to operate if ALL collections would be emptied or deleted.
    // TODO: changelog MUST mention the change in default behaviour here.
    #[serde(rename = "vdir/icalendar")]
    VdirIcalendar(Vdir<IcsItem>),

    #[serde(rename = "vdir/vcard")]
    VdirVcard(Vdir<VcardItem>),

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
            StorageSection::VdirIcalendar(def) => {
                let inner = Arc::new(def.into_storage()?);
                EitherStorage::Calendar(NamedStorage { name, inner })
            }
            StorageSection::VdirVcard(def) => {
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
#[serde(deny_unknown_fields)]
struct Vdir<I: Item> {
    path: Utf8PathBuf,
    fileext: String,
    /// Not implemented; bails.
    encoding: Option<String>,
    // TODO: post_hook
    // TODO: fileignoreext
    #[allow(dead_code)]
    post_hook: Option<OsString>,
    #[serde(default)]
    item: PhantomData<I>,
}

impl<I: Item> Vdir<I> {
    fn into_storage(self) -> anyhow::Result<VdirStorage<I>> {
        if self.encoding.is_some() {
            // I don't want to implement a feature that is potentially unused.
            // If someone really needs this, it's doable.
            error!("Vdir storage does no implement 'encoding' in v2.0.0.");
            error!("If you need to define a specific encoding, please open an issue.");
            bail!("'encoding' is not implemented for vdir storages.");
        }
        let path = expand_tilde(self.path).context("error expanding tilde for storage")?;
        // v0.X series expected the leading dot. This is not ideal and should be deprecated.
        let fileext = self
            .fileext
            .strip_prefix('.')
            .unwrap_or(&self.fileext)
            .to_string();
        Ok(VdirStorage::new(path, fileext))
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct CardDav {
    url: String,
    username: StringOrCommand,
    password: StringOrCommand,
    #[serde(flatten)]
    network_opts: HttpsConfig,
}

impl CardDav {
    async fn into_storage(self) -> anyhow::Result<CardDavStorage<HttpsConnector<HttpConnector>>> {
        Ok(CardDavStorage::new(
            self.url.parse()?,
            libdav::auth::Auth::Basic {
                username: self.username.into_string()?,
                // TODO: don't prompt if won't be sync'ed
                password: Some(self.password.into_password()?),
            },
            self.network_opts.into_connector()?,
        )
        .await?)
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct CalDav {
    url: StringOrCommand,
    username: StringOrCommand,
    password: StringOrCommand,
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
                // TODO: don't prompt if won't be sync'ed
                password: Some(self.password.into_password()?),
            },
            self.network_opts.into_connector()?,
        )
        .await?)
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct Http {
    url: StringOrCommand,
    /// A name for the single collection inside this storage.
    #[serde(deserialize_with = "deserialise_collection_id")]
    collection: CollectionId,
    #[serde(flatten)]
    #[allow(dead_code)]
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
#[serde(deny_unknown_fields)]
struct HttpsConfig {
    verify: Option<PathBuf>,
    verify_fingerprint: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    auth: Auth,
    auth_cert: Option<ClientCert>,
    #[serde(default = "default_useragent")]
    #[allow(dead_code)]
    useragent: String,
}

impl HttpsConfig {
    // TODO: keep a global cache using the hash of these.
    //       this would allow re-using the same TLS store for all clients.
    fn into_connector(self) -> anyhow::Result<HttpsConnector<HttpConnector>> {
        let tls_config = ClientConfig::builder();
        let tls_config = match (self.verify, self.verify_fingerprint) {
            (None, None) => tls_config.with_native_roots()?,
            (None, Some(fingerprint)) => {
                let verifier = Arc::from(FingerprintVerifier::new(&fingerprint)?);
                DangerousClientConfigBuilder { cfg: tls_config }
                    .with_custom_certificate_verifier(verifier)
            }
            (Some(path), None) => {
                let mut root_store = RootCertStore::empty();
                for cert in certs_from_pemfile(&path)? {
                    root_store.add(cert)?;
                }
                tls_config.with_root_certificates(root_store)
            }
            (Some(path), Some(fingerprint)) => {
                let mut root_store = RootCertStore::empty();
                for cert in certs_from_pemfile(&path)? {
                    root_store.add(cert)?;
                }
                let verifier =
                    Arc::from(FingerprintAndWebPkiVerifier::new(&fingerprint, root_store)?);
                DangerousClientConfigBuilder { cfg: tls_config }
                    .with_custom_certificate_verifier(verifier)
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
enum ClientCert {
    SingleFile(PathBuf),
    SeparateKeyAndCert(PathBuf, PathBuf),
}

// TODO: singlefile

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
enum StringOrCommand {
    Raw(String),
    Command { command: Vec<String> },
}

impl StringOrCommand {
    fn into_string(self) -> anyhow::Result<String> {
        match self {
            StringOrCommand::Raw(s) => Ok(s),
            StringOrCommand::Command { command } => {
                // TODO: should expand user and normalise paths.
                let mut values = command.into_iter();
                let cmd = values
                    .next()
                    .context("A command requires at least one value")?;
                let output = Command::new(cmd)
                    .args(values)
                    .stdout(Stdio::piped())
                    .output()
                    .context("problem executing command")?;
                match output.status.code() {
                    Some(0) => Ok(std::str::from_utf8(&output.stdout)?.trim().to_owned()),
                    Some(code) => bail!("Command exited with status {}.", code),
                    None => bail!("Command exited unexpectedly."),
                }
            }
        }
    }

    fn into_password(self) -> anyhow::Result<Password> {
        let string = self.into_string()?;
        if string.is_empty() {
            bail!("Command returned an empty password. This is likely a misconfiguration.")
        }
        Ok(Password::from(string))
    }
}

fn parse_from_file(mut path: File) -> anyhow::Result<Config> {
    let mut raw = String::new();
    path.read_to_string(&mut raw)?;
    let config = toml::from_str::<Config>(&raw)?;

    Ok(config)
}

/// Open the default path.
///
/// Attempts to open multiple paths in sequence and returns the first that works.
fn open_default_path() -> anyhow::Result<File> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(xdg).join("vdirsyncer/config.toml");
        if let Ok(file) = File::open(&path) {
            debug!("Opened config file {}", path.to_string_lossy());
            return Ok(file);
        }
        debug!("Could not open config file {}", path.to_string_lossy());
    }

    #[allow(deprecated)] // Only problematic on unsupported platforms.
    if let Some(home) = std::env::home_dir() {
        let path = home.join(".config/vdirsyncer/config.toml");
        if let Ok(file) = File::open(&path) {
            debug!("Opened config file {}", path.to_string_lossy());
            return Ok(file);
        }
        debug!("Could not open config file {}", path.to_string_lossy());
    }

    bail!("No usable configuration file found");
}

pub(crate) fn load_from_default_path() -> anyhow::Result<Config> {
    parse_from_file(open_default_path()?)
}
