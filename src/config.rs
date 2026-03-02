// Copyright 2023-2026 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    mem::swap,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, bail};
use camino::Utf8PathBuf;
use hyper::{
    Uri,
    header::{HeaderValue, USER_AGENT},
};
use hyper_rustls::{ConfigBuilderExt, HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{Client as HyperClient, connect::HttpConnector},
    rt::TokioExecutor,
};
use log::{debug, error, info, warn};
use rustls::{
    ClientConfig, RootCertStore,
    client::danger::DangerousClientConfigBuilder,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _},
};
use scfg::{Directive, Scfg};
use tokio::{
    sync::{Mutex, Notify},
    task::JoinSet,
};
use tower::{Layer, ServiceBuilder, util::Either};
use tower_http::set_header::SetRequestHeaderLayer;
use tower_http::{
    auth::{AddAuthorization, AddAuthorizationLayer},
    set_header::SetRequestHeader,
};
#[cfg(feature = "jmap")]
use vstorage::jmap::JmapStorage;
use vstorage::libdav::{CalDavClient, CardDavClient, dav::WebDavClient};
#[cfg(feature = "jmap")]
use vstorage::libjmap::{JmapClient, discover_session_resource};
use vstorage::{
    CollectionId, ItemKind,
    base::Storage,
    caldav::{CalDavStorage, CollectionIdSegment},
    carddav::CardDavStorage,
    readonly::ReadOnlyStorage,
    sync::{
        Mode, OneWaySync, TwoWaySync,
        declare::{CollectionDescription, OnDelete, OnEmpty, StoragePair, SyncedCollection},
    },
    vdir::VdirStorage,
    webcal::WebCalStorage,
};

use crate::{
    ConflictResolution, NamedPair, RawCommand, VERSION,
    cli::FilterNames,
    proxy::UnixConnector,
    repair::NamedStorage,
    scfg_util::{
        flatten_single_vec, resolve_cmd_inplace, take_single_directive, take_single_param,
        take_single_param_from_directive,
    },
    tls::{FingerprintAndWebPkiVerifier, FingerprintVerifier, cert_and_key_from_pemfile},
};

/// A deserialised configuration file.
#[derive(Debug)]
pub(crate) struct Config {
    status_path: Utf8PathBuf,
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
    /// Consumes the configuration to avoid copying any data needlessly and frees any
    /// unnecessary data.
    pub(crate) async fn into_named_pairs(self) -> anyhow::Result<Vec<NamedPair>> {
        let status_dir =
            expand_tilde(self.status_path).context("Expanding tilde for status_dir")?;
        let status_dir = Arc::new(status_dir); // TODO: could be Arc<str>
        let storages = Arc::new(StorageBuilder::new(self.storages));

        let mut tasks = JoinSet::new();
        for (name, mut config) in self.pairs {
            info!("Initialising pair {name}");

            let name_a = take_single_param_from_directive(&mut config, "storage_a")?;
            let name_b = take_single_param_from_directive(&mut config, "storage_b")?;

            let storages = storages.clone(); // Arc
            let status_dir = status_dir.clone(); // Arc
            tasks.spawn(async move {
                let ((storage_a, interval_a), (storage_b, interval_b)) =
                    tokio::try_join!(storages.get_storage(&name_a), storages.get_storage(&name_b))?;

                let mut collections = Vec::<Collections>::new();
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
                    .transpose()
                    .context("parsing on_empty")?
                    .unwrap_or_default();

                let on_delete = take_single_directive(&mut config, "on_delete")?
                    .map(parse_on_delete)
                    .transpose()
                    .context("parsing on_delete")?
                    .unwrap_or_default();

                let conflict_resolution =
                    take_single_directive(&mut config, "conflict_resolution")?
                        .map(parse_conflict_resolution)
                        .transpose()?;

                let mode: Arc<dyn Mode> =
                    if let Some(mut d) = take_single_directive(&mut config, "one_way")? {
                        if d.take_params().into_iter().next().is_some() {
                            bail!("the one_way directive takes no parameters")
                        }
                        Arc::new(OneWaySync)
                    } else {
                        Arc::new(TwoWaySync)
                    };

                // TODO: metadata

                let status_path = status_dir.join(format!("{name}.status"));
                match (storage_a.item_kind(), storage_b.item_kind()) {
                    (ItemKind::Calendar, ItemKind::AddressBook) => {
                        bail!("pair {name} mixes calendar storage with contacts storage")
                    }
                    (ItemKind::AddressBook, ItemKind::Calendar) => {
                        bail!("pair {name} mixes contacts storage with calendar storage")
                    }
                    (ItemKind::AddressBook, ItemKind::AddressBook)
                    | (ItemKind::Calendar, ItemKind::Calendar) => Ok(NamedPair {
                        name,
                        inner: init_pair(
                            collections,
                            (storage_a, storage_b),
                            on_empty,
                            on_delete,
                            mode,
                        ),
                        status_path,
                        conflict_resolution,
                        names: (name_a, name_b),
                        intervals: (interval_a, interval_b),
                    }),
                }
            });
        }

        let mut pairs = Vec::new();
        while let Some(res) = tasks.join_next().await {
            match res {
                Ok(Ok(p)) => pairs.push(p),
                Ok(Err(err)) => bail!(err),
                Err(joinerr) => bail!(joinerr),
            }
        }
        debug!("Initialised pairs.");

        Ok(pairs)
    }
}

/// Wrapper around storage that is only initialised when required.
enum LazyStorage {
    /// Raw configuration options for this storage.
    Raw(Scfg),
    /// Notifier to await while the storage is initialised concurrently.
    Initialising(Arc<Notify>),
    /// Storage that has been initialised, plus the duration for its `monitor` interval.
    Ready(Arc<dyn Storage>, Duration),
}

/// Build storages using configuration as input.
struct StorageBuilder {
    /// Keys are names given to storages.
    raw: HashMap<String, Mutex<LazyStorage>>,
}

impl StorageBuilder {
    /// Create a new builder with raw configuration data.
    fn new(raw: HashMap<String, Scfg>) -> Self {
        let raw = raw
            .into_iter()
            .map(|(k, v)| (k, Mutex::new(LazyStorage::Raw(v))))
            .collect();
        StorageBuilder { raw }
    }

    /// Returns a storage with a matching name.
    ///
    /// Ensures that each storage is initialised only once. If two concurrent calls would return
    /// the same storage, one of them will wait until the other resolves the storage.
    ///
    /// # Errors
    ///
    /// Only returns an error if the call to `init_storage` fails. Errors include the name of the
    /// failing storage, so can be bubbled up verbatim.
    async fn get_storage(
        &self,
        storage_name: &str,
    ) -> anyhow::Result<(Arc<dyn Storage>, Duration)> {
        let Some(value) = self.raw.get(storage_name) else {
            bail!("Storage {storage_name} is not defined.")
        };
        let mut lock = value.lock().await;
        match &*lock {
            LazyStorage::Raw(_) => {
                // Set state to "initialising"
                let mut data = LazyStorage::Initialising(Arc::new(Notify::new()));
                swap(&mut *lock, &mut data);
                drop(lock);

                // Initialising the storage might take some time (e.g.: a few network round trips),
                // so we do this after releasing the lock.
                let LazyStorage::Raw(scfg) = data else {
                    unreachable!("Data was mutated while we held a lock.");
                };
                let (storage, duration) = init_storage(scfg, storage_name)
                    .await
                    .with_context(|| format!("Initialising storage {storage_name}"))?;

                // Keep a copy of Arc<Storage> for other calls to get_storage.
                let mut data = LazyStorage::Ready(storage.clone(), duration);
                let mut lock = value.lock().await;
                swap(&mut *lock, &mut data);
                drop(lock);

                // Notify others waiting for this storage to be initialised.
                let LazyStorage::Initialising(notify) = data else {
                    unreachable!("Value was mutated while initialising.");
                };
                notify.notify_waiters();

                Ok((storage, duration))
            }
            LazyStorage::Initialising(notify) => {
                let notify = notify.clone();
                let notify = notify.notified();
                drop(lock);

                notify.await;
                let lock = value.lock().await;
                let LazyStorage::Ready(storage, duration) = &*lock else {
                    unreachable!("Received notification for non-ready storage.");
                };
                Ok((storage.clone(), *duration))
                // Dropping lock releases it.
            }
            LazyStorage::Ready(either_storage, duration) => Ok((either_storage.clone(), *duration)),
        }
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
        bail!("Invalid value for collections: {params}");
    };
    Ok(c)
}

// TODO: a "protect" flag to protect one side if EVERYTHING is about to be deleted:
// - off
// - items: refuses to operate if any item would be deleted.
// - collection: refuses to operate if a non-empty collection would be emptied or deleted.
// - storage: refuses to operate if ALL collections would be emptied or deleted.
// TODO: changelog MUST mention the change in default behaviour here.

/// Initialise a storage based on the given configuration.
async fn init_storage(
    mut config: Scfg,
    name: &str,
) -> anyhow::Result<(Arc<dyn Storage>, Duration)> {
    let type_ = take_single_param_from_directive(&mut config, "type")?;
    let interval = parse_interval(&mut config)?;
    let ro = if let Some(mut ro) = take_single_directive(&mut config, "read_only")? {
        if ro.take_params().into_iter().next().is_some() {
            bail!("the read_only directive takes no parameters")
        }
        true
    } else {
        false
    };
    let storage = match type_.as_ref() {
        "vdir/icalendar" => parse_vdir(config, ItemKind::Calendar, ro)?,
        "vdir/vcard" => parse_vdir(config, ItemKind::AddressBook, ro)?,
        "carddav" => parse_carddav(config, ro).await?,
        "caldav" => parse_caldav(config, ro).await?,
        "webcal" => parse_webcal(config, ro)?,
        #[cfg(feature = "jmap")]
        "jmap/icalendar" => parse_jmap(config, ItemKind::Calendar, ro).await?,
        #[cfg(feature = "jmap")]
        "jmap/vcard" => parse_jmap(config, ItemKind::AddressBook, ro).await?,
        #[cfg(not(feature = "jmap"))]
        "jmap/icalendar" | "jmap/vcard" => bail!("pimsync compiled with JMAP support disabled."),
        _ => bail!("Unknown storage type: {type_}"),
    };

    info!("Initialised storage {name}");
    Ok((storage, interval))
}

/// # Errors
///
/// If this path starts with tilde AND the home directory is non-UTF8.
fn expand_tilde(orig: Utf8PathBuf) -> Result<Utf8PathBuf, camino::FromPathBufError> {
    let mut iter = orig.as_str().chars();
    if let Some('~') = iter.next()
        && let Some('/') = iter.next()
    {
        #[allow(deprecated)] // Only problematic on unsupported platforms.
        let home = std::env::home_dir().expect("must resolve home path to expand tilde");
        let home = Utf8PathBuf::try_from(home)?;
        let rest = iter.collect::<String>();
        return Ok(home.join(rest));
    }
    Ok(orig)
}

fn init_pair(
    collections: Vec<Collections>,
    storages: (Arc<dyn Storage>, Arc<dyn Storage>),
    on_empty: OnEmpty,
    on_delete: OnDelete,
    mode: Arc<dyn Mode>,
) -> StoragePair {
    let mut pair = StoragePair::new(storages.0, storages.1);

    for collection in collections {
        pair = match collection {
            Collections::All => pair.with_all_from_a().with_all_from_b(),
            Collections::FromA => pair.with_all_from_a(),
            Collections::FromB => pair.with_all_from_b(),
            Collections::Named(id) => pair.with_mapping(SyncedCollection::direct(id)),
            Collections::Mapped(alias, a, b) => {
                pair.with_mapping(SyncedCollection::Mapped { alias, a, b })
            }
        }
    }

    pair.on_empty(on_empty).on_delete(on_delete).with_mode(mode)
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
        Some("cmd") => RawCommand::from_args(params)
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

fn parse_on_delete(mut directive: Directive) -> anyhow::Result<OnDelete> {
    let val = take_single_param(&mut directive).context("Parsing parameter for on_delete")?;

    match val.as_ref() {
        "skip" => Ok(OnDelete::Skip),
        "sync" => Ok(OnDelete::Sync),
        _ => bail!("on_empty must specify either 'skip' or 'sync'"),
    }
}

fn parse_collection_id_segment(config: &mut Scfg) -> anyhow::Result<CollectionIdSegment> {
    let directive = take_single_directive(config, "collection_id_segment")?;

    if let Some(mut directive) = directive {
        let val = take_single_param(&mut directive)
            .context("Parsing parameter for collection_id_segment")?;

        match val.as_ref() {
            "last" => Ok(CollectionIdSegment::Last),
            "second-last" => Ok(CollectionIdSegment::SecondLast),
            _ => bail!("collection_id_segment must specify either 'last' or 'second-last'"),
        }
    } else {
        Ok(CollectionIdSegment::default())
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

fn parse_vdir(mut config: Scfg, item_kind: ItemKind, ro: bool) -> anyhow::Result<Arc<dyn Storage>> {
    let path = take_single_param_from_directive(&mut config, "path")?.into();
    let path = expand_tilde(path).context("Expanding tilde for storage")?;

    if config.remove("encoding").is_some() {
        // I don't want to implement a feature that is potentially unused.
        // If someone really needs this, it's doable.
        error!("Pimsync does not implement 'encoding' for vdir storages.");
        error!("If you need to define a specific encoding, please open an issue.");
        bail!("'encoding' is not implemented for vdir storages.");
    }

    let mut builder = VdirStorage::builder(path)?;

    if let Some(mut fileext) = take_single_directive(&mut config, "fileext")? {
        let fileext = take_single_param(&mut fileext)?;
        // v0.X series expected the leading dot. This is not ideal and should be deprecated.
        let fileext = fileext.strip_prefix('.').unwrap_or(&fileext).to_string();
        builder = builder.with_extension(fileext);
    }

    Ok(into_arc(builder.build(item_kind), ro))
}

type RawHttpsClient = HyperClient<HttpsConnector<HttpConnector>, String>;
type RawUnixClient = HyperClient<UnixConnector, String>;

type HttpClient =
    SetRequestHeader<Either<AddAuthorization<RawHttpsClient>, RawHttpsClient>, HeaderValue>;
type UnixHttpClient =
    SetRequestHeader<Either<AddAuthorization<RawUnixClient>, RawUnixClient>, HeaderValue>;

type NetworkWebDav = WebDavClient<HttpClient>;
type UnixSocketWebDav = WebDavClient<UnixHttpClient>;

async fn parse_carddav(mut config: Scfg, ro: bool) -> anyhow::Result<Arc<dyn Storage>> {
    let url: Uri = take_single_param_from_directive(&mut config, "url")?
        .parse()
        .context("Parsing carddav url")?;
    let collection_id_segment = parse_collection_id_segment(&mut config)?;
    let socket = take_single_directive(&mut config, "socket")?;

    if let Some(mut socket_directive) = socket {
        let socket_path =
            take_single_param(&mut socket_directive).context("Parsing socket directive")?;
        let webdav = parse_socket_webdav_client(config, &socket_path, url)?;
        let client = CardDavClient::new(webdav);
        let mut builder = CardDavStorage::builder(client);
        builder = builder.with_collection_id_segment(collection_id_segment);
        Ok(into_arc(builder.build().await?, ro))
    } else {
        let webdav = parse_webdav_client(config, url)?;
        let client = CardDavClient::bootstrap_via_service_discovery(webdav).await?;
        let mut builder = CardDavStorage::builder(client);
        builder = builder.with_collection_id_segment(collection_id_segment);
        Ok(into_arc(builder.build().await?, ro))
    }
}

async fn parse_caldav(mut config: Scfg, ro: bool) -> anyhow::Result<Arc<dyn Storage>> {
    let url: Uri = take_single_param_from_directive(&mut config, "url")?
        .parse()
        .context("Parsing caldav url")?;
    let collection_id_segment = parse_collection_id_segment(&mut config)?;
    let socket = take_single_directive(&mut config, "socket")?;

    if let Some(mut socket_directive) = socket {
        let socket_path =
            take_single_param(&mut socket_directive).context("Parsing socket directive")?;
        let webdav = parse_socket_webdav_client(config, &socket_path, url)?;
        let client = CalDavClient::new(webdav);
        let mut builder = CalDavStorage::builder(client);
        builder = builder.with_collection_id_segment(collection_id_segment);
        Ok(into_arc(builder.build().await?, ro))
    } else {
        let webdav = parse_webdav_client(config, url)?;
        let client = CalDavClient::bootstrap_via_service_discovery(webdav).await?;
        let mut builder = CalDavStorage::builder(client);
        builder = builder.with_collection_id_segment(collection_id_segment);
        Ok(into_arc(builder.build().await?, ro))
    }
}

/// Parse options common to CalDAV and CardDAV and build the inner `WebDavClient`.
fn parse_webdav_client(config: Scfg, url: Uri) -> anyhow::Result<NetworkWebDav> {
    let http_client = parse_http_client(config)?;
    Ok(WebDavClient::new(url, http_client))
}

/// Parse options common to all HTTP clients.
fn parse_http_client(mut config: Scfg) -> anyhow::Result<HttpClient> {
    let auth = parse_auth(&mut config).context("Parsing carddav storage auth")?;
    let connector = parse_tls_config(&mut config)?;
    let user_agent = parse_user_agent(&mut config)?;

    let raw_client = HyperClient::builder(TokioExecutor::new()).build(connector);

    let auth_client = match auth {
        Some((username, password)) => {
            let auth_layer = AddAuthorizationLayer::basic(&username, &password).as_sensitive(true);
            Either::Left(auth_layer.layer(raw_client))
        }
        None => Either::Right(raw_client),
    };

    let client = ServiceBuilder::new()
        .layer(SetRequestHeaderLayer::overriding(USER_AGENT, user_agent))
        .service(auth_client);

    Ok(client)
}

fn parse_socket_webdav_client(
    mut config: Scfg,
    socket_path: &str,
    url: Uri,
) -> anyhow::Result<UnixSocketWebDav> {
    let auth = parse_auth(&mut config).context("Parsing storage auth")?;
    let user_agent = parse_user_agent(&mut config)?;

    let connector = crate::proxy::UnixConnector::new(socket_path);
    let raw_client = HyperClient::builder(TokioExecutor::new()).build(connector);

    let auth_client = match auth {
        Some((username, password)) => {
            let auth_layer = AddAuthorizationLayer::basic(&username, &password).as_sensitive(true);
            Either::Left(auth_layer.layer(raw_client))
        }
        None => Either::Right(raw_client),
    };

    let client = ServiceBuilder::new()
        .layer(SetRequestHeaderLayer::overriding(USER_AGENT, user_agent))
        .service(auth_client);

    Ok(WebDavClient::new(url, client))
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

fn parse_webcal(mut config: Scfg, ro: bool) -> anyhow::Result<Arc<dyn Storage>> {
    if ro {
        warn!("The read_only flag has no effect for Webcal; it is always read only.");
    }

    let url = take_single_param_from_directive(&mut config, "url")
        .context("Webcal storage must define a url")?
        .parse()?;
    let connector = parse_tls_config(&mut config)?;
    let auth = parse_auth(&mut config).context("Parsing carddav storage auth")?;
    let user_agent = parse_user_agent(&mut config)?;

    let collection_id = take_single_param_from_directive(&mut config, "collection_id")?
        .parse()
        .context("Parsing webcal url")?;

    let raw_client = HyperClient::builder(TokioExecutor::new()).build(connector);

    let auth_client = match auth {
        Some((username, password)) => {
            let auth_layer = AddAuthorizationLayer::basic(&username, &password).as_sensitive(true);
            Either::Left(auth_layer.layer(raw_client))
        }
        None => Either::Right(raw_client),
    };

    let client = ServiceBuilder::new()
        .layer(SetRequestHeaderLayer::overriding(USER_AGENT, user_agent))
        .service(auth_client);

    let builder = WebCalStorage::builder(client, url, collection_id);
    Ok(Arc::new(builder.build()))
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

/// Parse TLS configuration directives, if any, or return the default.
fn parse_tls_config(config: &mut Scfg) -> anyhow::Result<HttpsConnector<HttpConnector>> {
    let mut root_store = None;
    let mut fp_verifier = None;
    let mut client_auth = None;

    if let Some(mut tls_root) = take_single_directive(config, "tls_root")? {
        let path: PathBuf = take_single_param(&mut tls_root)
            .context("Parsing tls_root directive")?
            .parse()
            .context("tls_root must specify a valid path")?;

        let mut store = RootCertStore::empty();
        let certs: Vec<_> = CertificateDer::pem_file_iter(&path)
            .with_context(|| format!("opening {}", path.display()))?
            .collect::<Result<_, _>>()
            .with_context(|| format!("parsing {}", path.display()))?;
        for cert in certs {
            store.add(cert)?;
        }
        root_store = Some(store);
    }

    if let Some(mut fp) = take_single_directive(config, "tls_fingerprint")? {
        let fingerprint =
            take_single_param(&mut fp).context("Parsing tls_fingerprint directive")?;
        fp_verifier = Some(FingerprintVerifier::new(&fingerprint)?);
    }

    if let Some(mut auth_cert) = take_single_directive(config, "auth_cert")? {
        let params = auth_cert.take_params();
        let mut params = params.iter();

        let first = params
            .next()
            .context("auth_cert must specify at least one parameter")?;
        let cert = if let Some(second) = params.next() {
            let (crt_path, key_path): (PathBuf, PathBuf) = (first.parse()?, second.parse()?);
            let crt = CertificateDer::pem_file_iter(&crt_path)
                .with_context(|| format!("opening {}", crt_path.display()))?
                .collect::<Result<Vec<_>, _>>()
                .with_context(|| format!("parsing {}", crt_path.display()))?;
            let key = PrivateKeyDer::from_pem_file(&key_path)
                .with_context(|| format!("loading key from {}", key_path.display()))?;
            (crt, key)
        } else {
            let combined_path: PathBuf = first.parse()?;
            cert_and_key_from_pemfile(&combined_path)?
        };

        client_auth = Some(cert);
    }

    let tls_config = ClientConfig::builder();
    // FIXME: loads and parses certs again for each client.
    let tls_config = match (root_store, fp_verifier) {
        (None, None) => tls_config.with_native_roots()?,
        (None, Some(verifier)) => DangerousClientConfigBuilder { cfg: tls_config }
            .with_custom_certificate_verifier(Arc::from(verifier)),
        (Some(root_store), None) => tls_config.with_root_certificates(root_store),
        (Some(root_store), Some(fp_verifier)) => {
            let verifier = FingerprintAndWebPkiVerifier::new(fp_verifier, root_store)?;
            DangerousClientConfigBuilder { cfg: tls_config }
                .with_custom_certificate_verifier(Arc::from(verifier))
        }
    };

    let tls_config = match client_auth {
        None => tls_config.with_no_client_auth(),
        Some((certs, key)) => tls_config.with_client_auth_cert(certs, key)?,
    };

    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls_config)
        .https_or_http()
        .enable_http1()
        .build();
    Ok(connector)
}

#[cfg(feature = "jmap")]
async fn parse_jmap(
    mut config: Scfg,
    item_kind: ItemKind,
    ro: bool,
) -> anyhow::Result<Arc<dyn Storage>> {
    warn!(
        "THE JMAP IMPLEMENTATION IS EXPERIMENTAL ANY MAY HAVE BUGS WHICH COULD LEAD TO DATA LOSS!",
    );
    let url = take_single_param_from_directive(&mut config, "url")?;
    let mut http_client = parse_http_client(config)?;

    let url = url.parse().context("Parsing JMAP url")?;
    let (session_url, session_resource) = discover_session_resource(&mut http_client, &url).await?;
    let api_url = session_resource
        .api_url()
        .context("JMAP server returned no api_url")?
        .parse()
        .context("JMAP server returned an invalid api_url")?;
    let client = JmapClient::new(http_client, session_url, api_url);
    let builder = JmapStorage::builder(client);
    Ok(into_arc(builder.build(item_kind), ro))
}

/// Parse a given file as a configuration file.
///
/// If `enabled_pairs` is not `None`, only pairs with a matching name will be loaded.
pub(crate) fn parse_config(
    raw_config: &str,
    mut enabled_pairs: FilterNames,
) -> anyhow::Result<Config> {
    // TODO: The Scfg crate crates multiple copies of each string in the entire configuration file.
    //       I want a high-level API like the Scfg crate, but the zero-copy approach from scfg-scanner.
    let mut parser = raw_config
        .parse::<Scfg>()
        .context("Parsing configuration file")?;

    // TODO: should use Cow<str>, not String as keys.
    let mut pairs = HashMap::<String, Scfg>::new();
    let mut storages = HashMap::<String, Scfg>::new();

    resolve_cmd_inplace(&mut parser, "status_path").context("resolving status path")?;
    let status_path = take_single_param_from_directive(&mut parser, "status_path")?;

    let mut enabled_storages = HashSet::new();

    if let Some(directives) = parser.remove("pair") {
        for mut directive in directives {
            let name = take_single_param(&mut directive).context("Parsing pair directive")?;

            if pairs.keys().any(|existing| *existing == name) {
                bail!("Duplicate definition for pair {name}.");
            }

            // Skip disabled pairs.
            if !enabled_pairs.wants(&name) {
                continue; // Skip if not in enabled list.
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

    if let Some(missing) = enabled_pairs.next_missing() {
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
            resolve_storage_cmds(&mut child)
                .with_context(|| format!("resolving commands for storage {name}"))?;
            storages.insert(name, child);
        }
    }

    // TODO: assert that parser is now empty (e.g.: no superfluous values).

    Ok(Config {
        status_path: Utf8PathBuf::from(status_path),
        pairs,
        storages,
    })
}

/// Parse named storages from a configuration file, ignoring all else.
pub(crate) async fn parse_storages(
    raw_config: &str,
    mut enabled_storages: FilterNames,
) -> anyhow::Result<Vec<NamedStorage>> {
    let mut parser = raw_config
        .parse::<Scfg>()
        .context("Parsing configuration file")?;
    let mut tasks = JoinSet::new();

    // First read all configs, running all cmd directives before doing IO.
    if let Some(directives) = parser.remove("storage") {
        for mut directive in directives {
            let name = take_single_param(&mut directive).context("Parsing storage directive")?;
            if !enabled_storages.wants(&name) {
                debug!("Skipping storage {name}; not enabled.");
                continue;
            }

            let mut child = directive
                .take_child()
                .context("storage must define a block")?;
            resolve_storage_cmds(&mut child)?;
            tasks.spawn(async move {
                init_storage(child, &name)
                    .await
                    .with_context(|| format!("initialising storage {name}"))
                    .map(|(storage, _)| NamedStorage { name, storage })
            });
        }
    }

    if let Some(missing) = enabled_storages.next_missing() {
        bail!("Missing storage definition for: {missing}");
    }

    let mut storages = Vec::new();
    while let Some(res) = tasks.join_next().await {
        match res {
            Ok(Ok(ns)) => storages.push(ns),
            Ok(Err(err)) => bail!(err),
            Err(joinerr) => bail!(joinerr),
        }
    }
    Ok(storages)
}

/// List all pair names defined in the configuration file.
pub(crate) fn list_pair_names(raw_config: &str) -> anyhow::Result<Vec<String>> {
    let mut parser = raw_config
        .parse::<Scfg>()
        .context("Parsing configuration file")?;

    let mut names = Vec::new();
    if let Some(directives) = parser.remove("pair") {
        for mut directive in directives {
            let name = take_single_param(&mut directive).context("Parsing pair directive")?;
            names.push(name);
        }
    }
    Ok(names)
}

/// List all storage names defined in the configuration file.
pub(crate) fn list_storage_names(raw_config: &str) -> anyhow::Result<Vec<String>> {
    let mut parser = raw_config
        .parse::<Scfg>()
        .context("Parsing configuration file")?;

    let mut names = Vec::new();
    if let Some(directives) = parser.remove("storage") {
        for mut directive in directives {
            let name = take_single_param(&mut directive).context("Parsing storage directive")?;
            names.push(name);
        }
    }
    Ok(names)
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
        "vdir/icalendar" | "vdir/vcard" => {
            resolve_cmd_inplace(storage, "path").context("resolving path for storage")
        }
        "carddav" | "caldav" | "jmap/icalendar" | "jmap/vcard" => {
            resolve_cmd_inplace(storage, "url").context("resolving url for storage")?;
            resolve_cmd_inplace(storage, "username").context("resolving username for storage")?;
            resolve_cmd_inplace(storage, "password").context("resolving password for storage")
        }
        "webcal" => resolve_cmd_inplace(storage, "url").context("resolving url for storage"),
        _ => bail!("Unknown storage type: {type_}"),
    }
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

fn into_arc<S: Storage + 'static>(storage: S, read_only: bool) -> Arc<dyn Storage> {
    if read_only {
        Arc::new(ReadOnlyStorage::from(storage))
    } else {
        Arc::new(storage)
    }
}

#[cfg(test)]
mod test {
    use scfg::Scfg;

    use crate::{ConflictResolution, RawCommand, config::take_single_param_from_directive};

    use super::{
        default_user_agent, parse_conflict_resolution, resolve_cmd_inplace, take_single_directive,
    };

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
        let mut parser = "conflict_resolution cmd nvim -d".parse::<Scfg>().unwrap();
        let directive = take_single_directive(&mut parser, "conflict_resolution")
            .unwrap()
            .unwrap();
        let got = parse_conflict_resolution(directive).unwrap();
        assert_eq!(
            got,
            ConflictResolution::Cmd(RawCommand {
                command: "nvim".to_string(),
                args: vec!["-d".to_string(),],
            })
        );
    }

    #[test]
    fn test_inplace_resolution_plain() {
        let mut parser = "username alice@example.com".parse::<Scfg>().unwrap();
        resolve_cmd_inplace(&mut parser, "username").unwrap();
        let got = take_single_param_from_directive(&mut parser, "username").unwrap();
        assert_eq!(got, "alice@example.com",);
    }

    #[test]
    fn test_inplace_resolution_cmd() {
        let mut parser = "username {\n cmd echo alice@example.com\n}"
            .parse::<Scfg>()
            .unwrap();
        resolve_cmd_inplace(&mut parser, "username").unwrap();
        let got = take_single_param_from_directive(&mut parser, "username").unwrap();
        assert_eq!(got, "alice@example.com",);
    }

    #[test]
    fn test_inplace_resolution_shell() {
        let mut parser = "username {\n shell echo john@example.com | sed s/john/alice/\n}"
            .parse::<Scfg>()
            .unwrap();
        resolve_cmd_inplace(&mut parser, "username").unwrap();
        let got = take_single_param_from_directive(&mut parser, "username").unwrap();
        assert_eq!(got, "alice@example.com",);
    }

    #[test]
    fn test_empty_password_command_fails() {
        let mut parser = "password {\n cmd echo -n ''\n}".parse::<Scfg>().unwrap();
        let result = resolve_cmd_inplace(&mut parser, "password");
        let expected = "Password command returned an empty string.";
        assert!(result.unwrap_err().to_string().contains(expected));
    }
}
