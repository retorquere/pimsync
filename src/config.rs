// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, ensure, Context};
use camino::Utf8PathBuf;
use hyper::{header::HeaderValue, Uri};
use hyper_rustls::{ConfigBuilderExt, HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client as HyperClient},
    rt::TokioExecutor,
};
use libdav::{dav::WebDavClient, CalDavClient, CardDavClient};
use log::{debug, error, info};
use rustls::{client::danger::DangerousClientConfigBuilder, ClientConfig, RootCertStore};
use scfg::{Directive, Scfg};
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
    auth::AddAuthorization,
    tls::{
        cert_and_key_from_pemfile, certs_from_pemfile, key_from_pemfile,
        FingerprintAndWebPkiVerifier, FingerprintVerifier,
    },
    ua::UserAgent,
    App, ConflictResolution, NamedPair, RawCommand, VERSION,
};

/// A deserialised configuration file.
#[derive(Debug)]
pub(crate) struct Config {
    status_path: Utf8PathBuf,
    /// Used when storages do not implement or support monitoring.
    // FIXME: TODO: move into individual storage definitions.
    interval: Duration,
    pairs: HashMap<String, Scfg>,
    /// Configuration for all storages.
    ///
    /// Blocks which can be a string or a command are resolved prior to insertion here; it is safe
    /// to assume that they are a string (if present).
    storages: HashMap<String, Scfg>,
}

impl Config {
    /// Convert this configuration into an `App` instance.
    ///
    /// This consumes the configuration to avoid copying any data needlessly and freeing up any
    /// unnecessary data.
    pub(crate) async fn into_app(mut self) -> anyhow::Result<App> {
        let status_dir =
            expand_tilde(self.status_path).context("Expanding tilde for status_dir")?;

        // Only already-initialised storages (name -> instance).
        let mut storages = HashMap::<String, EitherStorage>::new();

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

            let on_empty = take_single_directive(&mut config, "on_empty")?
                .map(parse_on_empty)
                .transpose()?
                .unwrap_or_default();

            let conflict_resolution = take_single_directive(&mut config, "conflict_resolution")?
                .map(parse_conflict_resolution)
                .transpose()?;

            // TODO: metadata

            let status_path = status_dir.join(format!("{name}.status"));
            match (storage_a, storage_b) {
                (EitherStorage::Calendar(a), EitherStorage::Calendar(b)) => {
                    calendar_pairs.push(NamedPair {
                        name,
                        inner: init_pair(collections, (a, b), on_empty),
                        status_path,
                        conflict_resolution,
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
                        names: (name_a, name_b),
                    });
                }
            }
        }

        Ok(App {
            calendar_pairs,
            contact_pairs,
            interval: self.interval,
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
    let mut params = directive.take_params().into_iter();

    if let Some(param) = params.next() {
        if let Some(next) = params.next() {
            bail!("unexpected second parameter {next} in collection directive");
        }
        param
            .parse()
            .context("Parsing collection id")
            .map(Collections::Named)
    } else {
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

fn parse_conflict_resolution(mut directive: Directive) -> anyhow::Result<ConflictResolution> {
    let mut params = directive.take_params().into_iter();
    match params.next().as_deref() {
        Some("cmd") => RawCommand::try_from(params)
            .context("parsing conflict_resolution")
            .map(ConflictResolution::Cmd),
        Some("keep") => match params.next().as_deref() {
            Some("a") => Ok(ConflictResolution::KeepA),
            Some("b") => Ok(ConflictResolution::KeepB),
            Some(c) => bail!("Invalid parameter for conflict_resolution: keep {c}"),
            None => bail!("Invalid parameter for conflict_resolution: keep"),
        },
        Some(param) => bail!("Invalid parameter for conflict_resolution: {param}"),
        None => bail!("Missing parameter for conflict_resolution"),
    }
}

fn parse_on_empty(mut directive: Directive) -> anyhow::Result<OnEmpty> {
    let val = take_single_param(&mut directive).context("Parsing parameter for on_empty")?;

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
    let path = take_single_param_from_directive(&mut config, "path")?.into();
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

    Ok(Arc::new(VdirStorage::new(path, fileext)))
}

type NetworkWebDav =
    WebDavClient<UserAgent<AddAuthorization<HyperClient<HttpsConnector<HttpConnector>, String>>>>;

type UnixSocketWebDav =
    WebDavClient<UserAgent<AddAuthorization<HyperClient<hyperlocal::UnixConnector, String>>>>;

async fn parse_carddav(mut config: Scfg) -> anyhow::Result<Arc<dyn Storage<VcardItem>>> {
    let url = take_single_param_from_directive(&mut config, "url")?;

    if let Some(socket) = url.strip_prefix("unix://") {
        let webdav = parse_socket_webdav_client(config, socket)?;
        let client = CardDavClient::new(webdav);
        Ok(Arc::new(CardDavStorage::new(client).await?))
    } else {
        let url = url.parse().context("Parsing carddav url")?;
        let webdav = parse_webdav_client(config, url)?;
        let client = CardDavClient::bootstrap_via_service_discovery(webdav).await?;
        Ok(Arc::new(CardDavStorage::new(client).await?))
    }
}

async fn parse_caldav(mut config: Scfg) -> anyhow::Result<Arc<dyn Storage<IcsItem>>> {
    let url = take_single_param_from_directive(&mut config, "url")?;

    if let Some(socket) = url.strip_prefix("unix://") {
        let webdav = parse_socket_webdav_client(config, socket)?;
        let client = CalDavClient::new(webdav);
        Ok(Arc::new(CalDavStorage::new(client).await?))
    } else {
        let url = url.parse().context("Parsing caldav url")?;
        let webdav = parse_webdav_client(config, url)?;
        let client = CalDavClient::bootstrap_via_service_discovery(webdav).await?;
        Ok(Arc::new(CalDavStorage::new(client).await?))
    }
}

/// Parse options common to CalDAV and CardDAV and build the inner `WebDavClient`.
fn parse_webdav_client(mut config: Scfg, url: Uri) -> anyhow::Result<NetworkWebDav> {
    let auth = parse_auth(&mut config).context("Parsing carddav storage auth")?;
    let network_opts = parse_tls_config(&mut config)?;
    let user_agent = parse_user_agent(&mut config)?;

    let connector = network_opts.into_connector()?;
    let raw_client = HyperClient::builder(TokioExecutor::new()).build(connector);
    let auth_client = AddAuthorization::auto(raw_client, auth);
    let ua_client = UserAgent::new(auth_client, user_agent);
    Ok(WebDavClient::new(url, ua_client))
}

fn parse_socket_webdav_client(mut config: Scfg, socket: &str) -> anyhow::Result<UnixSocketWebDav> {
    let host = hex::encode(socket.as_bytes());
    let url = (format!("unix://{host}:0/"))
        .parse()
        .context("Building pseudo-url for socket connection")?;
    let auth = parse_auth(&mut config).context("Parsing carddav storage auth")?;
    let user_agent = parse_user_agent(&mut config)?;

    let raw_client = HyperClient::builder(TokioExecutor::new()).build(hyperlocal::UnixConnector);
    let auth_client = AddAuthorization::auto(raw_client, auth);
    let ua_client = UserAgent::new(auth_client, user_agent);
    Ok(WebDavClient::new(url, ua_client))
}

/// Parses a `user_agent` config directive, or returns the default if absent.
fn parse_user_agent(config: &mut Scfg) -> anyhow::Result<HeaderValue> {
    match take_single_directive(config, "user_agent")? {
        Some(d) => {
            let params = d.params().join("");
            if params.is_empty() {
                bail!("user_agent must not be empty");
            }
            params
                .try_into()
                .context("converting user_agent into a header value")
        }
        None => Ok(default_user_agent()),
    }
}

// INVARIANT: Does not panic; function is idempotent and has a dedicated test.
fn default_user_agent() -> HeaderValue {
    // Ideally this should be const and computed at compile-time.
    format!("pimsync/{}", VERSION.strip_prefix("v").unwrap_or(VERSION))
        .try_into()
        .expect("default UA is a valid header value")
}

fn parse_webcal(mut config: Scfg) -> anyhow::Result<Arc<dyn Storage<IcsItem>>> {
    let url = take_single_param_from_directive(&mut config, "url")
        .context("Webcal storage must define a url")?
        .parse()?;
    let network_opts = parse_tls_config(&mut config)?;
    let user_agent = parse_user_agent(&mut config)?;

    let collection_id = take_single_param_from_directive(&mut config, "collection_id")?
        .parse()
        .context("Parsing webcal url")?;

    // TODO: authentication fields

    let connector = network_opts.into_connector()?;
    let raw_client = HyperClient::builder(TokioExecutor::new()).build(connector);
    let ua_client = UserAgent::new(raw_client, user_agent);
    Ok(Arc::new(WebCalStorage::new(ua_client, url, collection_id)?))
}

/// Returns a `username` and `password` tuple.
///
/// A `cmd` block is not supported here; such commands should be resolved before calling this
/// function.
fn parse_auth(directive: &mut Scfg) -> anyhow::Result<Option<(String, String)>> {
    let username = match take_single_directive(directive, "username")? {
        Some(mut u) => take_single_param(&mut u)?,
        None => return Ok(None),
    };
    let password = match take_single_directive(directive, "password")? {
        Some(mut p) => take_single_param(&mut p)?,
        None => String::new(),
    };
    Ok(Some((username, password)))
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

/// Parse TLS configuration directives, if any, or return the default.
fn parse_tls_config(config: &mut Scfg) -> anyhow::Result<HttpsConfig> {
    let mut tls = HttpsConfig::default();

    if let Some(mut verify) = take_single_directive(config, "verify")? {
        let path = take_single_param(&mut verify)
            .context("Parsing verify directive")?
            .parse()
            .context("verify must specify a valid path")?;
        tls.verify = Some(path);
    }

    if let Some(mut fp) = take_single_directive(config, "verify_fingerprint")? {
        let fingerprint =
            take_single_param(&mut fp).context("Parsing verify_fingerprint directive")?;
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
    take_single_param(&mut directive).with_context(|| format!("Parsing directive {name}"))
}

/// Take a single parameter from a directive.
///
/// "single" here implies that it must not be followed by any other parameters.
fn take_single_param(directive: &mut Directive) -> anyhow::Result<String> {
    let mut params = directive.take_params().into_iter();
    let param = params.next().context("a parameter must be specified")?;
    if params.next().is_some() {
        bail!("no more than one parameter must be specified");
    }
    Ok(param)
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

/// Parse a given file as a configuration file.
///
/// If `enabled_pairs` is not `None`, only pairs with a matching name will be loaded.
pub(crate) fn parse_config(
    raw_config: &str,
    enabled_pairs: Option<&[String]>,
) -> anyhow::Result<Config> {
    // TODO: The Scfg crate crates multiple copies of each string in the entire configuration file.
    //       I want a high-level API like the Scfg crate, but the zero-copy approach from scfg-scanner.
    let mut parser = raw_config
        .parse::<Scfg>()
        .context("Parsing configuration file")?;

    // TODO: should use Cow<str>, not String as keys.
    let mut pairs = HashMap::<String, Scfg>::new();
    let mut storages = HashMap::<String, Scfg>::new();

    let interval = parse_interval(&mut parser)?;

    let status_path = take_single_param_from_directive(&mut parser, "status_path")?;

    let mut enabled_pairs = enabled_pairs.map(|vec| vec.iter().collect::<HashSet<_>>());
    let mut enabled_storages = HashSet::new();

    if let Some(directives) = parser.remove("pair") {
        for mut directive in directives {
            let name = take_single_param(&mut directive).context("Parsing pair directive")?;

            if pairs.keys().any(|existing| *existing == name) {
                bail!("Duplicate definition for pair {name}.");
            }

            // Skip disabled pairs.
            if let Some(ref mut enabled) = enabled_pairs {
                if !enabled.remove(&name) {
                    continue; // Skip if not in enabled list.
                }
            }

            info!("Enabled pair {name}");
            let child = directive.take_child().context("pair must define a block")?;

            // Superfluous parameters are ignored here; this is validated when the directive is
            // converted into a Pair instance.
            let name_a = child
                .get("storage_a")
                .context("pair must defined directive storage_a")?
                .params()
                .first()
                .context("storage_a must include a parameter")?
                .clone();
            let name_b = child
                .get("storage_b")
                .context("pair must defined directive storage_b")?
                .params()
                .first()
                .context("storage_b must include a parameter")?
                .clone();
            enabled_storages.insert(name_a);
            enabled_storages.insert(name_b);

            pairs.insert(name, child);
        }
    }

    if let Some(missing) = enabled_pairs.and_then(|e| e.into_iter().next()) {
        bail!("Requested pair missing from configuration: {missing}");
    }

    if let Some(directives) = parser.remove("storage") {
        for mut directive in directives {
            let name = take_single_param(&mut directive).context("Parsing storage directive")?;
            if !enabled_storages.contains(&name) {
                debug!("Skipping storage {name}; not used by any enabled pair.");
                continue;
            }

            let mut child = directive
                .take_child()
                .context("storage must define a block")?;
            resolve_storage_cmds(&mut child)?;
            storages.insert(name, child);
        }
    }

    // TODO: assert that parser is now empty (e.g.: no superfluous values).

    Ok(Config {
        status_path: Utf8PathBuf::from(status_path),
        interval,
        pairs,
        storages,
    })
}

fn parse_interval(parser: &mut Scfg) -> anyhow::Result<Duration> {
    let seconds = if let Some(mut directive) = take_single_directive(parser, "interval")? {
        take_single_param(&mut directive)
            .context("Parsing interval directive")?
            .parse()
            .context("Interval must be a valid integer")?
    } else {
        300
    };
    Ok(Duration::from_secs(seconds))
}

/// Resolve parameters defined as `cmd` blocks.
///
/// Mutates input block, replacing a `cmd {…}` block with the resolved value.
fn resolve_storage_cmds(storage: &mut Scfg) -> anyhow::Result<()> {
    // XXX: doesn't validate "single" (but this is re-read later).
    let type_ = storage
        .get("type")
        .context("storage must include a 'type' directive")?
        .params()
        .first()
        .context("type directive must specify one parameter")?;

    match type_.as_ref() {
        "vdir/icalendar" | "vdir/vcard" => Ok(()),
        "carddav" | "caldav" => {
            resolve_cmd_inplace(storage, "username").context("resolving username for storage")?;
            resolve_cmd_inplace(storage, "password").context("resolving password for storage")
        }
        "webcal" => resolve_cmd_inplace(storage, "url").context("resolving url for storage"),
        _ => bail!("Unknown storage type: {type_}"),
    }
}

fn resolve_cmd_inplace(storage: &mut Scfg, name: &str) -> anyhow::Result<()> {
    let Some(mut directive) = take_single_directive(storage, name)? else {
        return Ok(());
    };

    let mut params = directive.take_params().into_iter();
    let value = if let Some(param) = params.next() {
        if params.next().is_some() {
            bail!("Found more than one parameter for directive {name}");
        }
        param
    } else {
        let output = parse_block_with_raw_cmd(directive)
            .with_context(|| format!("Parsing cmd for {name} directive"))?
            .command()
            .stdout(Stdio::piped())
            .output()
            .with_context(|| format!("Error executing command for {name} directive"))?;
        match output.status.code() {
            Some(0) => std::str::from_utf8(&output.stdout)?.trim().to_owned(),
            Some(code) => bail!("Command exited with status {}.", code),
            None => bail!("Command exited unexpectedly."),
        }
    };

    let url_directive = storage.add(name);
    url_directive.append_param(value);

    Ok(())
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

#[cfg(test)]
mod test {
    use scfg::Scfg;

    use crate::{ConflictResolution, RawCommand};

    use super::{default_user_agent, parse_conflict_resolution, take_single_directive};

    #[test]
    fn test_default_user_agent() {
        // Validate invariant; function does not panic.
        let _ = default_user_agent();
    }

    #[test]
    fn test_parse_conflict_resolution_keep_a() {
        let mut parser = "conflict_resolution keep a".parse::<Scfg>().unwrap();
        let directive = take_single_directive(&mut parser, "conflict_resolution")
            .unwrap()
            .unwrap();
        let got = parse_conflict_resolution(directive).unwrap();
        assert_eq!(got, ConflictResolution::KeepA);
    }

    #[test]
    fn test_parse_conflict_resolution_keep_b() {
        let mut parser = "conflict_resolution keep b".parse::<Scfg>().unwrap();
        let directive = take_single_directive(&mut parser, "conflict_resolution")
            .unwrap()
            .unwrap();
        let got = parse_conflict_resolution(directive).unwrap();
        assert_eq!(got, ConflictResolution::KeepB);
    }

    #[test]
    fn test_parse_conflict_resolution_cmd() {
        let mut parser =
            "conflict_resolution cmd hiq -dFpassword proto=carddavs username=alice@example.com"
                .parse::<Scfg>()
                .unwrap();
        let directive = take_single_directive(&mut parser, "conflict_resolution")
            .unwrap()
            .unwrap();
        let got = parse_conflict_resolution(directive).unwrap();
        assert_eq!(
            got,
            ConflictResolution::Cmd(RawCommand {
                command: "hiq".to_string(),
                args: vec![
                    "-dFpassword".to_string(),
                    "proto=carddavs".to_string(),
                    "username=alice@example.com".to_string(),
                ],
            })
        );
    }
}
