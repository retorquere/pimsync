// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Implements reading/writing entries from a local [`vdir`].
//!
//! - The `href` for an items is its filename relative to its parent directory.
//! - The `href` for a collection is its absolute path. This may change in future.
//!
//! [`vdir`]: https://vdirsyncer.pimutils.org/en/stable/vdir.html
use async_trait::async_trait;
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use futures_util::future::{select, BoxFuture, Either};
use futures_util::{StreamExt as _, TryStreamExt as _};
use inotify::{EventMask, Inotify, WatchMask};
use libdav::xmlutils::normalise_newlines;
use log::error;
use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::fs::Metadata;
use std::marker::PhantomData;
use std::os::unix::prelude::MetadataExt;
use std::path::Path;
use std::pin::pin;
use std::time::Duration;
use tokio::fs::{create_dir, metadata, read_dir, read_to_string, remove_dir, remove_file, File};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt};
use tokio::sync::oneshot::{self, Sender};
use tokio::sync::RwLock;
use tokio::time::{Instant, Interval};

use crate::atomic::AtomicFile;
use crate::base::{
    AddressBookProperty, CalendarProperty, Collection, FetchedItem, Item, ItemRef, ListedProperty,
    Storage,
};
use crate::disco::{DiscoveredCollection, Discovery};
use crate::watch::{Event, EventKind, SpecificEvent, StorageMonitor};
use crate::{CollectionId, Error, ErrorKind, Etag, Href, Result};

/// A `vdir` filesystem directory containing zero or more directories.
///
/// Each child directory is treated as [`Collection`]. Nested subdirectories are not supported.
///
/// # Hrefs
///
/// Internally, all `href`s are paths relative to the base directory.
// TODO: add link to spec here.
pub struct VdirStorage<I: Item> {
    /// The path to a directory containing a storage.
    ///
    /// Each top-level subdirectory will be treated as a separate collection, and individual files
    /// inside these are each treated as an `Item`.
    pub path: Utf8PathBuf,
    /// Filename extension for items in a storage. Files with matching extension are treated a
    /// items for a collection, and all other files are ignored.
    pub extension: String,
    i: PhantomData<I>,
    // Discretionary locks to prevent multiple tasks from operating on the same file concurrently.
    file_locks: FileLocker,
}

const SAFE_FILENAME_CHARS: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-+";

#[async_trait]
impl<I: Item> Storage<I> for VdirStorage<I>
where
    I::Property: PropertyWithFilename,
{
    async fn check(&self) -> Result<()> {
        let meta = metadata(&self.path)
            .await
            .map_err(|e| Error::new(ErrorKind::DoesNotExist, e))?;

        if meta.is_dir() {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::NotAStorage,
                "path is not a directory",
            ))
        }
    }

    async fn discover_collections(&self) -> Result<Discovery> {
        let mut entries = read_dir(&self.path).await?;

        let mut collections = Vec::<_>::new();
        while let Some(entry) = entries.next_entry().await? {
            if !metadata(entry.path()).await?.is_dir() {
                continue;
            }
            let href = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::new(ErrorKind::InvalidData, "collection id is not utf8"))?;
            if href.starts_with('.') {
                continue;
            }
            let id = href
                .parse()
                .map_err(|e| Error::new(ErrorKind::InvalidData, e))?;
            collections.push(DiscoveredCollection::new(href, id));
        }

        Ok(collections.into())
    }

    async fn create_collection(&self, href: &str) -> Result<Collection> {
        let path = build_collection_path(&self.path, href)?;
        create_dir(&path).await?;

        Ok(Collection::new(href.to_string()))
    }

    async fn destroy_collection(&self, href: &str) -> Result<()> {
        let path = build_collection_path(&self.path, href)?;
        remove_dir(path).await.map_err(Error::from)
    }

    async fn list_items(&self, collection_href: &str) -> Result<Vec<ItemRef>> {
        let mut read_dir = read_dir(build_collection_path(&self.path, collection_href)?).await?;

        let mut items = Vec::new();
        let extension = OsStr::new(self.extension.as_str());
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if path.extension() != Some(extension) {
                continue;
            }
            let href = href_for_path(&self.path, &path)?;
            let etag = etag_for_path(path).await?;

            items.push(ItemRef { href, etag });
        }

        Ok(items)
    }

    async fn get_item(&self, href: &str) -> Result<(I, Etag)> {
        let path = build_item_path(&self.path, &self.extension, href)?;

        let mut file = File::open(&path).await?;
        let mut buf = String::new();
        file.read_to_string(&mut buf).await?;

        let item = I::from(normalise_newlines(&buf));
        let etag = etag_for_metadata(&file.metadata().await?);

        Ok((item, etag))
    }

    async fn get_many_items(&self, hrefs: &[&str]) -> Result<Vec<FetchedItem<I>>> {
        futures_util::stream::iter(hrefs)
            .then(|href| async move {
                self.get_item(href).await.map(|(item, etag)| FetchedItem {
                    href: String::from(*href),
                    item,
                    etag,
                })
            })
            .try_collect()
            .await
    }

    async fn get_all_items(&self, collection_href: &str) -> Result<Vec<FetchedItem<I>>> {
        let mut read_dir = read_dir(build_collection_path(&self.path, collection_href)?).await?;

        let mut items = Vec::new();
        let extension = OsStr::new(self.extension.as_str());
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if path.extension() != Some(extension) {
                continue;
            }

            let mut file = File::open(&path).await?;
            let mut buf = String::new();
            file.read_to_string(&mut buf).await?;

            let item = I::from(normalise_newlines(&buf));
            let etag = etag_for_metadata(&file.metadata().await?);

            items.push(FetchedItem {
                href: href_for_path(&self.path, &path)?,
                item,
                etag,
            });
        }

        Ok(items)
    }

    async fn set_property(&self, href: &str, meta: I::Property, value: &str) -> Result<()> {
        let filename = meta.filename();
        let path = build_collection_path(&self.path, href)?.join(filename);
        let file_lock = self.file_locks.lock_file(path.as_str()).await;

        let mut file = AtomicFile::new(&path)?;
        file.write_all(value.as_bytes()).await?;
        file.commit()?;
        self.file_locks.release_file(file_lock).await;
        Ok(())
    }

    async fn unset_property(&self, href: &str, meta: I::Property) -> Result<()> {
        let filename = meta.filename();
        let path = build_collection_path(&self.path, href)?.join(filename);
        let file_lock = self.file_locks.lock_file(path.as_str()).await;
        remove_file(filename).await?;
        self.file_locks.release_file(file_lock).await;
        Ok(())
    }

    async fn get_property(&self, href: &str, meta: I::Property) -> Result<Option<String>> {
        let filename = meta.filename();

        let path = build_collection_path(&self.path, href)?.join(filename);
        match read_to_string(path).await {
            Ok(value) => Ok(Some(value)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::from(e)),
        }
    }

    async fn add_item(&self, collection_href: &str, item: &I) -> Result<ItemRef> {
        // No lock is used for creating a new file; races are only possible when it already exists.
        let basename = item
            .ident()
            .chars()
            // TODO: We only need to remove a few "illegal" characters, so this is a bit too strict.
            .filter(|c| SAFE_FILENAME_CHARS.contains(*c))
            .collect::<String>();

        let filename = format!("{}.{}", basename, self.extension);
        let relpath = Utf8PathBuf::from(collection_href).join(filename);

        let absolute_path = build_item_path(&self.path, &self.extension, relpath.as_str())?;
        let mut file = AtomicFile::new(&absolute_path)?;
        file.write_all(item.as_str().as_bytes()).await?;
        file.commit_new()?;

        let item_ref = ItemRef {
            href: relpath.into_string(),
            // FIXME: etag calculation is subject to races. Should use `fstat` here
            etag: etag_for_path(absolute_path).await?,
        };
        Ok(item_ref)
    }

    async fn update_item(&self, href: &str, etag: &Etag, item: &I) -> Result<Etag> {
        let filename = build_item_path(&self.path, &self.extension, href)?;

        let actual_etag = etag_for_path(&filename).await?;
        if *etag != actual_etag {
            return Err(Error::new(ErrorKind::InvalidData, "wrong etag"));
        }

        let file_lock = self.file_locks.lock_file(filename.as_str()).await;
        let mut file = AtomicFile::new(&filename)?;
        file.write_all(item.as_str().as_bytes()).await?;
        file.commit()?;
        // FIXME: etag calculation is subject to races. Should use `fstat` here
        let etag = etag_for_path(&filename).await?;
        self.file_locks.release_file(file_lock).await;
        Ok(etag)
    }

    /// # Quirks
    ///
    /// Checking the etag is vulnerable to TOCTOU race conditions. Filesystem APIs do not provide
    /// facilities to work around this.
    async fn delete_item(&self, href: &str, etag: &Etag) -> Result<()> {
        let filename = build_item_path(&self.path, &self.extension, href)?;

        let actual_etag = etag_for_path(&filename).await?;
        if *etag != actual_etag {
            return Err(Error::new(ErrorKind::InvalidData, "wrong etag"));
        }

        let file_lock = self.file_locks.lock_file(filename.as_str()).await;
        remove_file(&filename).await?;
        self.file_locks.release_file(file_lock).await;

        Ok(())
    }

    fn href_for_collection_id(&self, id: &CollectionId) -> Result<Href> {
        Ok(id.to_string())
    }

    async fn list_properties(
        &self,
        collection_href: &str,
    ) -> Result<Vec<ListedProperty<I::Property>>> {
        let mut props = Vec::new();
        for property in I::Property::known_properties() {
            let prop_value = self.get_property(collection_href, property.clone()).await?;
            if let Some(value) = prop_value {
                props.push(ListedProperty {
                    property: property.clone(),
                    value,
                });
            };
        }
        Ok(props)
    }

    /// Monitor the storage for changes.
    ///
    /// Due to limitations of the `inotify` subsystem, it is possible that some events are silently
    /// lost. To compensate for these, the monitor shall yield a [`Event::General`] every
    /// `interval`.
    async fn monitor(&self, interval: Duration) -> Result<Box<dyn StorageMonitor>> {
        VdirMonitor::new(self, interval).map(|m| Box::new(m) as Box<dyn StorageMonitor>)
    }
}

impl<I: Item> VdirStorage<I> {
    #[must_use]
    pub fn new(path: Utf8PathBuf, extension: String) -> Self {
        Self {
            path,
            extension,
            i: PhantomData,
            file_locks: FileLocker::default(),
        }
    }
}

/// Joins an href to the storage's path.
///
/// This method does safety checks to ensure that malicious input cannot write outside the
/// storage's directory.
///
/// # Errors
///
/// - If the input is an invalid directory name
/// - If the resulting path is not a child of the storage's directory.
fn build_collection_path(root: &Utf8Path, collection_href: &str) -> Result<Utf8PathBuf> {
    let href = Utf8Path::new(collection_href);
    let mut components = href.components();
    if !matches!(components.next(), Some(Utf8Component::Normal(_))) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "collection href must be a valid directory name",
        ));
    };
    if components.next().is_some() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "collection href must contain exactly one component",
        ));
    };

    Ok(root.join(href))
}

/// Build path to an item with the given href.
///
/// Validates that extension matches.
fn build_item_path(root: &Utf8Path, extension: &str, href: &str) -> Result<Utf8PathBuf> {
    let href = Utf8Path::new(href);

    let mut components = href.components();
    if !matches!(components.next(), Some(Utf8Component::Normal(_))) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "first component of item href must be a regular filename",
        ));
    };
    if let Some(Utf8Component::Normal(name)) = components.next() {
        let name = Utf8Path::new(name);
        if name.extension() != Some(extension) {
            Err(Error::new(
                ErrorKind::InvalidInput,
                "item href does not have an extension matching this storage",
            ))?;
        }
    } else {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "second component of item href must be a regular filename",
        ));
    }
    if components.next().is_some() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "item href cannot contain more than two components",
        ));
    };

    Ok(root.join(href))
}

/// Returns the href for a path.
///
/// # Panics
///
/// If `path` is not a grandchild of the storage's path.
fn href_for_path(root: &Utf8Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)
        // This never takes external input. If this panics, we have a bug.
        .expect("path of item must include storage path as prefix")
        .to_str()
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "Filename is not valid UTF-8"))
        .map(str::to_string)
}

/// Values are a queue of tasks waiting to operate on the same file.
#[derive(Default)]
struct FileLocker(RwLock<HashMap<Box<str>, VecDeque<Sender<()>>>>);

impl FileLocker {
    async fn lock_file<'a>(&self, filepath: &'a str) -> FileLock<'a> {
        let mut locks = self.0.write().await;
        if let Some(ref mut self_waiter) = locks.get_mut(&Box::from(filepath)) {
            let (tx, rx) = oneshot::channel();
            self_waiter.push_back(tx);

            drop(locks);
            rx.await
                .expect("Previous locker of file must release successfully.");
        } else {
            locks.insert(Box::from(filepath), VecDeque::new());
        }
        FileLock(filepath)
    }

    async fn release_file(&self, lock: FileLock<'_>) {
        let mut locks = self.0.write().await;
        if let Some(ref mut self_waiter) = locks.get_mut(lock.0) {
            if let Some(waiter) = self_waiter.pop_front() {
                waiter
                    .send(())
                    .expect("Waiter for file lock must remain alive.");
            } else {
                locks.remove(lock.0);
            }
        }
        drop(locks);
    }
}

/// (Internal) lock handle on a file.
struct FileLock<'a>(&'a str);

/// Monitor for a [`VdirStorage`] instance.
///
/// # Quirks
///
/// Due to underlying limitations of inotify(7), it's possible that some events are
/// missed. When this happens, an [`Event::General`] is returned.
pub struct VdirMonitor {
    extension: String,
    timer: Interval,
    events: inotify::EventStream<Vec<u8>>,
}

impl VdirMonitor {
    /// Create a new monitor for `storage`.
    ///
    /// # Errors
    ///
    /// If an error occurs setting up the underlying filesystem watcher.
    pub fn new<I: Item>(storage: &VdirStorage<I>, interval: Duration) -> Result<VdirMonitor> {
        // TODO: errors don't help understand the root cause.
        let inotify = Inotify::init().map_err(|err| Error::new(ErrorKind::Io, err))?;
        inotify
            .watches()
            .add(
                &storage.path,
                WatchMask::MODIFY
                    | WatchMask::CREATE
                    | WatchMask::DELETE
                    | WatchMask::CLOSE_WRITE
                    | WatchMask::MOVE,
            )
            .map_err(|err| Error::new(ErrorKind::Io, err))?;
        let buf = Vec::with_capacity(4096); // XXX: Does inotify grow it if necessary?
        let events = inotify
            .into_event_stream(buf)
            .map_err(|err| Error::new(ErrorKind::Io, err))?;

        let mut timer = tokio::time::interval_at(Instant::now() + interval, interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        Ok(VdirMonitor {
            extension: storage.extension.clone(),
            timer,
            events,
        })
    }
}

impl StorageMonitor for VdirMonitor {
    fn next_event(&mut self) -> BoxFuture<Event> {
        Box::pin(async {
            loop {
                let next = pin!(self.events.next());
                // TODO: It should be possible to disable the timer.
                let timeout = pin!(self.timer.tick());
                match select(next, timeout).await {
                    Either::Left((None, _)) => unreachable!("End of stream for inotify events."),
                    Either::Left((Some(Ok(event)), _)) => {
                        if event.mask.contains(EventMask::Q_OVERFLOW) {
                            // TODO: drain the whole queue as well.
                            return Event::General;
                        }
                        let Some(name) = event.name else {
                            return Event::General;
                        };
                        if !name.as_encoded_bytes().ends_with(self.extension.as_bytes()) {
                            continue; // Irrelevant file
                        }
                        let path = match name.into_string() {
                            Ok(s) => s,
                            Err(os) => {
                                error!("Event for non-UTF8 file: {}.", os.to_string_lossy());
                                continue;
                            }
                        };
                        return if event.mask.contains(EventMask::DELETE) {
                            Event::Specific(SpecificEvent {
                                href: path,
                                kind: EventKind::Delete,
                            })
                        } else {
                            Event::Specific(SpecificEvent {
                                href: path,
                                kind: EventKind::Change,
                            })
                        };
                    }
                    Either::Left((Some(Err(err)), _)) => {
                        error!("Error reading from inotify: {err}");
                        // TODO: re-initialise inotify?
                        return Event::General;
                    }
                    Either::Right(..) => return Event::General,
                }
            }
        })
    }
}

async fn etag_for_path(path: impl AsRef<Path>) -> Result<Etag> {
    let metadata = &metadata(path).await?;
    Ok(etag_for_metadata(metadata))
}

fn etag_for_metadata(metadata: &Metadata) -> Etag {
    format!("{};{}", metadata.mtime(), metadata.ino()).into()
}

/// Helper to synchronise collection properties into filesystem.
///
/// This trait should only be required when implementing a new [`Item`] type that should work with
/// the existing [`VdirStorage`] implementation.
///
/// In order for the `Item`'s properties to synchronise to the filesystem, it should implement this
/// trait.
pub trait PropertyWithFilename: 'static {
    fn filename(&self) -> &'static str;

    fn known_properties() -> &'static [Self]
    where
        Self: Sized;
}

impl PropertyWithFilename for CalendarProperty {
    fn filename(&self) -> &'static str {
        match self {
            CalendarProperty::DisplayName => "displayname",
            CalendarProperty::Colour => "color",
            CalendarProperty::Description => "description",
            CalendarProperty::Order => "order",
        }
    }

    fn known_properties() -> &'static [Self] {
        &[
            CalendarProperty::DisplayName,
            CalendarProperty::Colour,
            CalendarProperty::Description,
            CalendarProperty::Order,
        ]
    }
}

impl PropertyWithFilename for AddressBookProperty {
    fn filename(&self) -> &'static str {
        match self {
            AddressBookProperty::DisplayName => "displayname",
            AddressBookProperty::Description => "description",
        }
    }

    fn known_properties() -> &'static [Self] {
        &[
            AddressBookProperty::DisplayName,
            AddressBookProperty::Description,
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{create_dir_all, write},
        str::FromStr,
    };

    use crate::{
        base::{CalendarProperty, IcsItem, Storage},
        vdir::{build_collection_path, build_item_path, VdirStorage},
        CollectionId, ErrorKind,
    };
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_missing_displayname() {
        let dir = tempdir().unwrap();

        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );
        let collection = storage.create_collection("test").await.unwrap();
        let displayname = storage
            .get_property(
                collection.href(),
                crate::base::CalendarProperty::DisplayName,
            )
            .await
            .unwrap();

        assert!(displayname.is_none());
    }

    #[tokio::test]
    async fn test_path_handling() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let collection_name = "one";
        let collection_path = dir.path().join(collection_name);
        create_dir_all(&collection_path).unwrap();

        let without_prodid = [
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");

        write(collection_path.join("item.ics"), &without_prodid).unwrap();

        let listed_items = storage.list_items(collection_name).await.unwrap();
        assert_eq!(listed_items.len(), 1);
        assert_eq!(listed_items[0].href, "one/item.ics");

        let all_items = storage.get_all_items(collection_name).await.unwrap();
        assert_eq!(all_items.len(), 1);
        assert_eq!(all_items[0].href, "one/item.ics");

        let (_item, etag) = storage.get_item("one/item.ics").await.unwrap();
        // Nothing to assert here.

        let many_items = storage.get_many_items(&["one/item.ics"]).await.unwrap();
        assert_eq!(many_items.len(), 1);
        assert_eq!(many_items[0].href, "one/item.ics");

        storage.delete_item("one/item.ics", &etag).await.unwrap();

        let item = IcsItem::from(without_prodid);
        storage.add_item("one", &item).await.unwrap();

        let all_items = storage.get_all_items(collection_name).await.unwrap();
        assert_eq!(all_items.len(), 1);
    }

    #[tokio::test]
    async fn test_missing_paths() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let missing_collection = "two";
        let err = match storage.list_items(missing_collection).await {
            Ok(items) => panic!("expected error, got {} result.", items.len()),
            Err(e) => e,
        };
        assert_eq!(err.kind, ErrorKind::DoesNotExist);

        // TODO: more tests on the missing collection
    }

    #[tokio::test]
    async fn test_write_read_colour() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let collection_name = "one";
        storage.create_collection(collection_name).await.unwrap();

        storage
            .set_property(collection_name, CalendarProperty::Colour, "#000000")
            .await
            .unwrap();

        let colour = storage
            .get_property(collection_name, CalendarProperty::Colour)
            .await
            .unwrap();

        assert_eq!(colour, Some(String::from("#000000")));
    }

    #[tokio::test]
    async fn test_read_missing_description() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let collection_name = "one";
        storage.create_collection(collection_name).await.unwrap();

        let description = storage
            .get_property(collection_name, CalendarProperty::Description)
            .await
            .unwrap();

        assert_eq!(description, None);
    }

    // TODO: test writing and then checking the file
    // TODO: test writing a file and then getting
    //
    #[tokio::test]
    async fn test_href_for_collection_id() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let collection_id = CollectionId::from_str("one").unwrap();
        let href = storage.href_for_collection_id(&collection_id).unwrap();
        assert_eq!(href, "one");
    }

    #[tokio::test]
    async fn test_build_collection_path_is_safe() {
        let dir = tempdir().unwrap();
        let root = dir.path().try_into().unwrap();

        assert!(build_collection_path(root, "penguins").is_ok());
        assert!(build_collection_path(root, "penguins/").is_ok());
        assert!(build_collection_path(root, "蛙类").is_ok());

        assert!(build_collection_path(root, "/").is_err());
        assert!(build_collection_path(root, "/usr/share/").is_err());
        assert!(build_collection_path(root, "..").is_err());
        assert!(build_collection_path(root, ".").is_err());
        assert!(build_collection_path(root, "../d").is_err());
        assert!(build_collection_path(root, "s/../../").is_err());
    }

    #[tokio::test]
    async fn test_build_item_path_is_safe() {
        let dir = tempdir().unwrap();
        let root = dir.path().try_into().unwrap();
        let extension = "ics";

        assert!(build_item_path(root, extension, "penguins/someitem.ics").is_ok());
        assert!(build_item_path(root, extension, "蛙类/item.ics").is_ok());

        assert!(build_item_path(root, extension, "penguins/someitem.jpeg").is_err());
        assert!(build_item_path(root, extension, "蛙类/item.jpeg").is_err());
        assert!(build_item_path(root, extension, "penguins/someitem").is_err());
        assert!(build_item_path(root, extension, "蛙类/item").is_err());
        assert!(build_item_path(root, extension, "penguins").is_err());
        assert!(build_item_path(root, extension, "蛙类").is_err());
        assert!(build_item_path(root, extension, "penguins/../someitem.ics").is_err());
        assert!(build_item_path(root, extension, "../penguins/someitem.ics").is_err());
        assert!(build_item_path(root, extension, "/").is_err());
        assert!(build_item_path(root, extension, "/usr/share/").is_err());
        assert!(build_item_path(root, extension, "..").is_err());
        assert!(build_item_path(root, extension, ".").is_err());
        assert!(build_item_path(root, extension, "s/../../").is_err());
    }

    #[tokio::test]
    async fn only_safe_chars_in_filenames() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let valid = [
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:11bb6bed-c29b-4999-a627-12dee35f8395",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");
        let item = IcsItem::from(valid);
        storage.create_collection("one").await.unwrap();
        let item_ref = storage.add_item("one", &item).await.unwrap();
        assert_eq!(
            item_ref.href,
            "one/11bb6bed-c29b-4999-a627-12dee35f8395.ics"
        );
    }

    #[tokio::test]
    async fn only_unsafe_chars_in_filenames() {
        let dir = tempdir().unwrap();
        let storage = VdirStorage::<IcsItem>::new(
            dir.path().to_path_buf().try_into().unwrap(),
            "ics".to_string(),
        );

        let valid = [
            "BEGIN:VCALENDAR",
            "BEGIN:VEVENT",
            "DTSTART:19970714T170000Z",
            "DTEND:19970715T035959Z",
            "SUMMARY:Bastille Day Party",
            "UID:these/slashes/are/not/okay",
            "END:VEVENT",
            "END:VCALENDAR",
        ]
        .join("\r\n");
        let item = IcsItem::from(valid);
        storage.create_collection("one").await.unwrap();
        let item_ref = storage.add_item("one", &item).await.unwrap();
        assert_eq!(item_ref.href, "one/theseslashesarenotokay.ics");
    }
}
