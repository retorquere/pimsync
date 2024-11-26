// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::{
    collections::HashMap, fs::File, path::PathBuf, process::Stdio, sync::Arc, time::Duration,
};

use anyhow::{bail, ensure, Context};
use camino::Utf8PathBuf;
use hyper_rustls::{ConfigBuilderExt, HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::connect::HttpConnector;
use libdav::{
    auth::{Auth, Password},
    dav::WebDavClient,
    CalDavClient, CardDavClient,
};
use log::{debug, error, info};
use rustls::{client::danger::DangerousClientConfigBuilder, ClientConfig, RootCertStore};
use scfg::{Directive, Scfg};
use tokio::sync::Mutex;
use vstorage::{
    base::{IcsItem, Item, Storage, VcardItem},
    caldav::CalDavStorage,
    carddav::CardDavStorage,
    sync::declare::{CollectionDescription, DeclaredMapping, OnEmpty, StoragePair},
    vdir::{PropertyWithFilename, VdirStorage},
    webcal::WebCalStorage,
    CollectionId,
};

use crate::{
    stdio::StdIo,
    tls::{
        cert_and_key_from_pemfile, certs_from_pemfile, key_from_pemfile,
        FingerprintAndWebPkiVerifier, FingerprintVerifier,
    },
    App, NamedPair, RawCommand,
};

/// A deserialised configuration file.
#[derive(Debug)]
pub(crate) struct Config {
    status_path: Utf8PathBuf,
    /// Used when storages do not implement or support monitoring.
    // FIXME: TODO: move into individual storage definitions.
    interval: Duration,
    pairs: HashMap<String, Scfg>,
    storages: HashMap<String, Scfg>,
}

impl Config {
    /// Convert this configuration into an `App` instance.
    ///
    /// This consumes the configuration to avoid copying any data needlessly and freeing up any
    /// unnecessary data.
    ///
    /// If `enabled_pairs` is not `None`, only pairs with a matching name will be loaded.
    pub(crate) async fn into_app<'storages>(mut self) -> anyhow::Result<App> {
        let status_dir =
            expand_tilde(self.status_path).context("Expanding tilde for status_dir")?;

        // Only already-initialised storages (name -> instance).
        // TODO: keep instance so I can later lock storages before using them.
        let mut storages = HashMap::<String, EitherStorage>::new();
        let mut locks = HashMap::<String, Arc<Mutex<()>>>::new();

        let mut calendar_pairs = Vec::new();
        let mut contact_pairs = Vec::new();

        for (name, mut config) in self.pairs {
            info!("Initialising pair {name}");
            let mut collections = Vec::<Collections>::new();

            let name_a = take_single_param_from_directive(&mut config, "storage_a")?;
            let storage_a = init_storage(&mut self.storages, &mut storages, &name_a)
                .await
                .with_context(|| format!("initialising storage {name_a}"))?;

            let name_b = take_single_param_from_directive(&mut config, "storage_b")?;
            let storage_b = init_storage(&mut self.storages, &mut storages, &name_b)
                .await
                .with_context(|| format!("initialising storage {name_b}"))?;

            if let Some(directives) = config.remove("collections") {
                for directive in directives {
                    let params = directive.params().join(" ");
                    collections.push(parse_collections_directive(&params)?);
                }
            }

            if let Some(directives) = config.remove("collection") {
                for directive in directives {
                    collections.push(parse_collection_directive(directive)?);
                }
            }

            let on_empty = if let Some(directive) = take_single_directive(&mut config, "on_empty")?
            {
                parse_on_empty(directive).context("Parsing on_empty")?
            } else {
                OnEmpty::default()
            };

            let conflict_resolution = if let Some(mut directive) =
                take_single_directive(&mut config, "conflict_resolution")?
            {
                let mut params = directive.take_params().into_iter();
                match params.next().as_deref() {
                    Some("cmd") => {
                        Some(RawCommand::try_from(params).context("parsing conflict_resolution")?)
                    }
                    // TODO: other 'keep a', 'keep b'.
                    _ => bail!("conflict_resolution expects a cmd parameter"),
                }
            } else {
                None
            };

            // TODO: metadata

            let lock_a = locks.entry(name_a.clone()).or_default().clone();
            let lock_b = locks.entry(name_b.clone()).or_default().clone();

            // Keep locks sorted based on storage name. Prevents deadlocks.
            let locks = if name_a < name_b {
                (lock_a, lock_b)
            } else {
                (lock_b, lock_a)
            };

            let status_path = status_dir.join(format!("{name}.status"));
            match (storage_a, storage_b) {
                (EitherStorage::Calendar(a), EitherStorage::Calendar(b)) => {
                    calendar_pairs.push(NamedPair {
                        name,
                        inner: init_pair(collections, (a, b), on_empty),
                        status_path,
                        conflict_resolution,
                        locks,
                        names: (name_a, name_b),
                    });
                }
                (EitherStorage::Calendar(_), EitherStorage::AddressBook(_)) => {
                    bail!("pair {} mixes calendar storage with contacts storage", name)
                }
                (EitherStorage::AddressBook(_), EitherStorage::Calendar(_)) => {
                    bail!("pair {} mixes contacts storage with calendar storage", name)
                }
                (EitherStorage::AddressBook(a), EitherStorage::AddressBook(b)) => {
                    contact_pairs.push(NamedPair {
                        name,
                        inner: init_pair(collections, (a, b), on_empty),
                        status_path,
                        conflict_resolution,
                        locks,
                        names: (name_a, name_b),
                    });
                }
            }
        }

        Ok(App {
            calendar_pairs,
            contact_pairs,
            interval: self.interval,
            stdio: Arc::new(StdIo::new()),
        })
    }
}

fn parse_collections_directive(params: &str) -> anyhow::Result<Collections> {
    let c = if params == "all" {
        Collections::All
    } else if params == "from a" {
        Collections::FromA
    } else if params == "from b" {
        Collections::FromB
    } else {
        bail!("Invalid value for colletions: {params}");
    };
    Ok(c)
}

// TODO: a "protect" flag to protect one side if EVERYTHING is about to be deleted:
// - off
// - items: refuses to operate if any item would be deleted.
// - collection: refuses to operate if a non-empty collection would be emptied or deleted.
// - storage: refuses to operate if ALL collections would be emptied or deleted.
// TODO: changelog MUST mention the change in default behaviour here.

/// If the storage is in `raw_storages`, initialise it and add it into `parsed_storages`.
/// Otherwise, find it in `parsed_storages`.
async fn init_storage(
    raw_storages: &mut HashMap<String, Scfg>,
    parsed_storages: &mut HashMap<String, EitherStorage>,
    storage_name: &str,
) -> anyhow::Result<EitherStorage> {
    if let Some((name, mut config)) = raw_storages.remove_entry(storage_name) {
        let type_ = take_single_param_from_directive(&mut config, "type")?;
        let storage = match type_.as_ref() {
            "vdir/icalendar" => EitherStorage::Calendar(parse_vdir(config)?),
            "vdir/vcard" => EitherStorage::AddressBook(parse_vdir(config)?),
            // TODO: should not await here; return a FutureStorage instead.
            "carddav" => EitherStorage::AddressBook(parse_carddav(config).await?),
            "caldav" => EitherStorage::Calendar(parse_caldav(config).await?),
            "webcal" => EitherStorage::Calendar(parse_webcal(config)?),
            _ => bail!("Unknown storage type: {type_}"),
        };

        let inner = storage.clone();
        info!("Initialised storage {name}");
        parsed_storages.insert(name.to_string(), storage);
        Ok(inner)
    } else {
        debug!("Re-using storage {storage_name}");
        parsed_storages
            .get(storage_name)
            .cloned()
            .with_context(|| format!("Storage {storage_name} is not defined."))
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

fn init_pair<I: Item>(
    collections: Vec<Collections>,
    storages: (Arc<dyn Storage<I>>, Arc<dyn Storage<I>>),
    on_empty: OnEmpty,
    // TODO: partial_sync
) -> StoragePair<I> {
    let mut pair = StoragePair::new(storages.0, storages.1);

    for collection in collections {
        pair = match collection {
            Collections::All => pair.with_all_from_a().with_all_from_b(),
            Collections::FromA => pair.with_all_from_a(),
            Collections::FromB => pair.with_all_from_b(),
            Collections::Named(id) => pair.with_mapping(DeclaredMapping::direct(id)),
            Collections::Mapped(alias, a, b) => {
                pair.with_mapping(DeclaredMapping::Mapped { alias, a, b })
            }
        }
    }

    pair.on_empty(on_empty)
}

fn parse_collection_directive(mut directive: Directive) -> anyhow::Result<Collections> {
    let mut params = directive.take_params();
    match params.len() {
        1 => params
            .remove(0)
            .parse()
            .context("Parsing collection id")
            .map(Collections::Named),
        2.. => bail!(
            "unexpected second parameter {} in collection directive",
            params[1]
        ),
        0 => {
            let mut child = directive
                .take_child()
                .context("Collection directive must specify an id or a block")?;

            let alias = take_single_param_from_directive(&mut child, "alias")?;
            let id_a = take_single_directive(&mut child, "id_a")?;
            let href_a = take_single_directive(&mut child, "href_a")?;
            let id_b = take_single_directive(&mut child, "id_b")?;
            let href_b = take_single_directive(&mut child, "href_b")?;

            let a = parse_individual_collection(id_a, href_a)?;
            let b = parse_individual_collection(id_b, href_b)?;
            Ok(Collections::Mapped(alias, a, b))
        }
    }
}

/// Flatten a `Vec` which is expected to have a single item.
fn flatten_single_vec<T>(mut vec: Vec<T>) -> Option<T> {
    if vec.len() == 1 {
        vec.pop()
    } else {
        None
    }
}

fn parse_individual_collection(
    id: Option<Directive>,
    href: Option<Directive>,
) -> anyhow::Result<CollectionDescription> {
    Ok(match (id, href) {
        (None, None) => bail!("collection block must define either id_a or href_a"),
        (None, Some(mut href)) => {
            let href = flatten_single_vec(href.take_params())
                .context("Collection href must define exactly one parameter")?;
            CollectionDescription::Href { href }
        }
        (Some(mut id), None) => {
            let id = flatten_single_vec(id.take_params())
                .context("Collection id must define exactly one parameter")?
                .parse()
                .context("Parsing collection id")?;
            CollectionDescription::Id { id }
        }
        (Some(_), Some(_)) => bail!("Collection block cannot define both id_a and href_a."),
    })
}

fn parse_on_empty(mut directive: Directive) -> anyhow::Result<OnEmpty> {
    let val = directive
        .take_params()
        .pop()
        .context("Directive on_empty must include one parameter")?;

    match val.as_ref() {
        "skip" => Ok(OnEmpty::Skip),
        "sync" => Ok(OnEmpty::Sync),
        _ => bail!("on_empty must specify either 'skip' or 'sync'"),
    }
}

// Temporary field
enum Collections {
    All,
    FromA,
    FromB,
    Named(CollectionId),
    Mapped(String, CollectionDescription, CollectionDescription),
}

#[derive(Clone)] // Cheap clone; enum + Arc.
pub(crate) enum EitherStorage {
    Calendar(Arc<dyn Storage<IcsItem>>),
    AddressBook(Arc<dyn Storage<VcardItem>>),
}

fn parse_vdir<I: Item + 'static>(mut config: Scfg) -> anyhow::Result<Arc<dyn Storage<I>>>
where
    I::Property: PropertyWithFilename,
{
    let path = take_single_param_from_directive(&mut config, "path")?;
    let path = Utf8PathBuf::from(path);
    let path = expand_tilde(path).context("Expanding tilde for storage")?;

    let fileext = take_single_param_from_directive(&mut config, "fileext")?;
    // v0.X series expected the leading dot. This is not ideal and should be deprecated.
    let fileext = fileext.strip_prefix('.').unwrap_or(&fileext).to_string();

    if config.remove("encoding").is_some() {
        // I don't want to implement a feature that is potentially unused.
        // If someone really needs this, it's doable.
        error!("Vdir storage does no implement 'encoding' in v2.0.0.");
        error!("If you need to define a specific encoding, please open an issue.");
        bail!("'encoding' is not implemented for vdir storages.");
    }

    // TODO: post_hook
    // TODO: fileignoreext

    Ok(Arc::new(VdirStorage::new(path, fileext)))
}

enum IntoString {
    Raw(String),
    Cmd(RawCommand),
}

impl IntoString {
    fn into_string(self) -> anyhow::Result<String> {
        match self {
            IntoString::Raw(raw) => Ok(raw),
            IntoString::Cmd(raw_command) => {
                let output = raw_command
                    .command()
                    .stdout(Stdio::piped())
                    .output()
                    .context("Error executing command")?;
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

async fn parse_carddav(mut config: Scfg) -> anyhow::Result<Arc<dyn Storage<VcardItem>>> {
    let url = take_single_param_from_directive(&mut config, "url")?;
    let auth = parse_auth(&mut config).context("Parsing carddav storage auth")?;
    if let Some(socket) = url.strip_prefix("unix://") {
        let host = hex::encode(socket.as_bytes());
        let url = (format!("unix://{host}:0/"))
            .parse()
            .context("Building pseudo-url for socket connection")?;

        let webdav = WebDavClient::new(url, auth, hyperlocal::UnixConnector);
        let client = CardDavClient::new(webdav);
        Ok(Arc::new(CardDavStorage::new(client).await?))
    } else {
        let url = url.parse().context("Parsing carddav url")?;
        let network_opts = parse_tls_config(&mut config)?;
        let webdav = WebDavClient::new(url, auth, network_opts.into_connector()?);
        let client = CardDavClient::new_via_bootstrap(webdav).await?;
        Ok(Arc::new(CardDavStorage::new(client).await?))
    }
}

async fn parse_caldav(mut config: Scfg) -> anyhow::Result<Arc<dyn Storage<IcsItem>>> {
    let url = take_single_param_from_directive(&mut config, "url")?;
    let auth = parse_auth(&mut config).context("Parsing caldav storage auth")?;

    // TODO: start_date
    // TODO: end_date
    // TODO: item_types

    if let Some(socket) = url.strip_prefix("unix://") {
        let host = hex::encode(socket.as_bytes());
        let url = (format!("unix://{host}:0/"))
            .parse()
            .context("Building pseudo-url for socket connection")?;

        let webdav = WebDavClient::new(url, auth, hyperlocal::UnixConnector);
        let client = CalDavClient::new(webdav);
        Ok(Arc::new(CalDavStorage::new(client).await?))
    } else {
        let url = url.parse().context("Parsing caldav url")?;
        let network_opts = parse_tls_config(&mut config)?;
        let webdav = WebDavClient::new(url, auth, network_opts.into_connector()?);
        let client = CalDavClient::new_via_bootstrap(webdav).await?;
        Ok(Arc::new(CalDavStorage::new(client).await?))
    }
}

fn parse_webcal(mut config: Scfg) -> anyhow::Result<Arc<dyn Storage<IcsItem>>> {
    let url =
        take_single_directive(&mut config, "url")?.context("Webcal storage must define a url")?;
    let url = parse_into_string(url)
        .context("Parsing webcal URL")?
        .into_string() // TODO: don't allocate this into string.
        .context("Resolving webcal URL")?
        .parse()?;

    let collection_id = take_single_param_from_directive(&mut config, "collection_id")?
        .parse()
        .context("Parsing webcal url")?;

    // TODO: network_opts
    // TODO: authentication fields
    // TODO: TLS fields

    Ok(Arc::new(WebCalStorage::new(url, collection_id)?))
}

fn parse_auth(directive: &mut Scfg) -> anyhow::Result<Auth> {
    let username = take_single_directive(directive, "username")?
        .map(parse_into_string)
        .transpose()
        .context("Parsing username directive")?;
    let password = take_single_directive(directive, "password")?
        .map(parse_into_string)
        .transpose()
        .context("Parsing username directive")?;

    let auth = match username {
        Some(x) => Auth::Basic {
            username: x.into_string().context("Resolving username")?,
            password: match password {
                Some(y) => Some(y.into_password().context("Resolving password")?),
                None => None,
            },
        },
        None => Auth::None,
    };
    Ok(auth)
}

fn parse_into_string(directive: Directive) -> anyhow::Result<IntoString> {
    let result = if let Some(param) = directive.params().first() {
        IntoString::Raw(param.to_owned())
    } else {
        IntoString::Cmd(parse_block_with_raw_cmd(directive)?)
    };
    Ok(result)
}

fn parse_block_with_raw_cmd(mut directive: Directive) -> anyhow::Result<RawCommand> {
    let mut block = directive
        .take_child()
        .context("Must define a parameter or a block")?;
    let params = take_single_directive(&mut block, "cmd")?
        .context("Block must define a cmd directive")?
        .take_params()
        .into_iter();

    RawCommand::try_from(params)
}

#[derive(Debug, Default)]
struct HttpsConfig {
    verify: Option<PathBuf>,
    verify_fingerprint: Option<String>,
    auth_cert: Option<ClientCert>,
}

fn parse_tls_config(config: &mut Scfg) -> anyhow::Result<HttpsConfig> {
    let mut tls = HttpsConfig::default();

    if let Some(mut verify) = take_single_directive(config, "verify")? {
        let path = verify
            .take_params()
            .pop()
            .context("verify must specify one parameter")?
            .parse()
            .context("verify must specify a valid path")?;
        tls.verify = Some(path);
    }

    if let Some(mut fp) = take_single_directive(config, "verify_fingerprint")? {
        let fingerprint = fp
            .take_params()
            .pop()
            .context("verify_fingerprint must specify one parameter")?;
        tls.verify_fingerprint = Some(fingerprint);
    }

    if let Some(mut auth_cert) = take_single_directive(config, "auth_cert")? {
        let params = auth_cert.take_params();
        let mut params = params.iter();

        let first = params
            .next()
            .context("auth_cert must specify at least one parameter")?;
        let cert = if let Some(second) = params.next() {
            ClientCert::SeparateKeyAndCert(first.parse()?, second.parse()?)
        } else {
            ClientCert::SingleFile(first.parse()?)
        };

        tls.auth_cert = Some(cert);
    }

    Ok(tls)
}

/// Take a directive expecting it at most once.
///
/// # Errors
///
/// If the directive is defined more than once.
fn take_single_directive(config: &mut Scfg, name: &str) -> anyhow::Result<Option<Directive>> {
    if let Some(mut directives) = config.remove(name) {
        ensure!(
            directives.len() == 1,
            "{name} may only be specified once per block.",
        );
        let directive = directives
            .pop()
            .expect("directives contains exactly one element");
        Ok(Some(directive))
    } else {
        Ok(None)
    }
}

/// Take a single parameter from a directive.
///
/// # Errors
///
/// Returns an error if zero or more than one parameter is specified.
fn take_single_param_from_directive(config: &mut Scfg, name: &str) -> anyhow::Result<String> {
    let mut directive = take_single_directive(config, name)?
        .with_context(|| format!("directive {name} not found"))?;
    let mut params = directive.take_params();
    ensure!(
        params.len() == 1,
        "{name} must specify exactly one parameter"
    );
    Ok(params.pop().expect("at least one parameter is defined"))
}

impl HttpsConfig {
    // TODO: keep a global cache using the hash of these.
    //       this would allow re-using the same TLS store for all clients.
    fn into_connector(self) -> anyhow::Result<HttpsConnector<HttpConnector>> {
        let tls_config = ClientConfig::builder();
        let tls_config = match (self.verify, self.verify_fingerprint) {
            // FIXME: loads and parses certs again for each client.
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

#[derive(Debug)]
enum ClientCert {
    SingleFile(PathBuf),
    SeparateKeyAndCert(PathBuf, PathBuf),
}

// TODO: singlefile

/// Parse a given file as a configuration file.
pub(crate) fn parse_config(
    raw_config: &str,
    enabled_pairs: Option<&Vec<String>>,
) -> anyhow::Result<Config> {
    // TODO: The Scfg crate crates multiple copies of each string in the entire configuration file.
    //       I want a high-level API like the Scfg crate, but the zero-copy approach from scfg-scanner.
    let mut parser = raw_config
        .parse::<Scfg>()
        .context("Parsing configuration file")?;

    // TODO: should use Cow<str>, not String as keys.
    let mut pairs = HashMap::<String, Scfg>::new();
    let mut storages = HashMap::<String, Scfg>::new();

    let interval = if let Some(mut directive) = take_single_directive(&mut parser, "interval")? {
        let mut params = directive.take_params().into_iter();
        params
            .next()
            .context("Interval must have exactly one parameter")?
            .parse()
            .context("Interval must be a valid integer")?
    } else {
        300
    };

    let status_path = take_single_param_from_directive(&mut parser, "status_path")?;

    if let Some(directives) = parser.remove("pair") {
        for mut directive in directives {
            let name = directive
                .take_params()
                .pop() // TODO: ignores superfluous values
                .context("pair must specify a name")?;

            // Skip disabled pairs.
            if let Some(enabled) = enabled_pairs {
                if !enabled.iter().any(|e| *e == name) {
                    continue;
                };
            }

            info!("Enabled pair {name}");
            let child = directive.take_child().context("pair must define a block")?;
            pairs.insert(name, child);
        }
    }

    if let Some(directives) = parser.remove("storage") {
        for mut directive in directives {
            let name = directive
                .take_params()
                .pop() // TODO: ignores superfluous values
                .context("storage must specify a name")?;

            let child = directive
                .take_child()
                .context("storage must define a block")?;
            storages.insert(name, child);
        }
    }

    // TODO: assert that parser is now empty (e.g.: no superfluous values).

    Ok(Config {
        status_path: Utf8PathBuf::from(status_path),
        interval: Duration::from_secs(interval),
        pairs,
        storages,
    })
}

/// Open the configuration file, expecting it in the default path.
///
/// Returns the path of the file opened and the file itself.
pub(crate) fn open_default_path() -> anyhow::Result<(PathBuf, File)> {
    let path = if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("pimsync/pimsync.conf")
    } else {
        #[allow(deprecated)]
        let home = std::env::home_dir().context("Could not resolve $XDG_CONFIG_HOME nor $HOME.")?;
        home.join(".config/pimsync/pimsync.conf")
    };

    let file =
        File::open(&path).with_context(|| format!("Could not open {}.", path.to_string_lossy()))?;
    debug!("Opened config file {}", path.to_string_lossy());
    Ok((path, file))
}
