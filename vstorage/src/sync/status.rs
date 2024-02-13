use std::path::Path;

use log::debug;
use sqlite::{BindableWithIndex, Connection, ConnectionThreadSafe, OpenFlags, State};

use crate::{base::ItemRef, CollectionId, Etag, Href};

use super::plan::{ResolvedCollection, ResolvedMapping};

const SCHEMA_VERSION: i64 = 2;

/// Error interacting with status database.
#[derive(thiserror::Error, Debug)]
#[allow(clippy::module_name_repetitions)]
pub enum StatusError {
    #[error("IO error operating with status database: {0}")]
    Io(#[from] sqlite::Error),
}

pub type Result<T> = std::result::Result<T, StatusError>;

#[derive(Clone, Copy)]
pub enum Side {
    A,
    B,
}

impl Side {
    #[must_use]
    pub fn opposite(self) -> Side {
        match self {
            Side::A => Side::B,
            Side::B => Side::A,
        }
    }
}

impl BindableWithIndex for Side {
    fn bind<T>(self, statement: &mut sqlite::Statement, index: T) -> sqlite::Result<()>
    where
        T: sqlite::ParameterIndex,
    {
        match self {
            Side::A => 0,
            Side::B => 1,
        }
        .bind(statement, index)
    }
}

/// State for an item at some point in time.
#[derive(PartialEq, Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct ItemState {
    pub(super) href: Href,
    pub(super) uid: String,
    pub(super) etag: Etag, // TODO: optional?
    pub(super) hash: String,
}

impl ItemState {
    /// Create an `ItemRef` by copying the `href` and `etag`.
    #[must_use]
    pub fn to_item_ref(&self) -> ItemRef {
        ItemRef {
            href: self.href.clone(),
            etag: self.etag.clone(),
        }
    }
}

/// Connection to an on-disk status database.
#[allow(clippy::module_name_repetitions)]
pub struct StatusDatabase {
    conn: ConnectionThreadSafe,
}

impl StatusDatabase {
    // NOTE: side is stored as boolean, 0=a, 1=b.

    /// Open the database in readonly mode.
    ///
    /// Returns `None` if the database does not exist.
    ///
    /// # Errors
    ///
    /// Returns `Error::Io` if sqlite fails to open the database.
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Option<StatusDatabase>> {
        let flags = OpenFlags::new().with_read_only().with_full_mutex();
        match Connection::open_thread_safe_with_flags(path, flags) {
            Ok(conn) => Ok(Some(StatusDatabase { conn })),
            Err(e) if e.code == Some(14) => Ok(None),
            Err(e) => Err(StatusError::Io(e)),
        }
    }

    /// Open or creates the database in read-write mode.
    ///
    /// # Errors
    ///
    /// Returns `Error::Io` if sqlite fails to open or create the database.
    pub fn open_or_create(path: impl AsRef<Path>) -> Result<StatusDatabase> {
        let db = StatusDatabase {
            conn: Connection::open_thread_safe(path)?,
        };

        db.init_schema()?;

        Ok(db)
    }

    /// This function is idempotent and safe to call more than once.
    ///
    /// In case of interruption during the first initialisation, later attempts will finalise
    /// creating tables and indexes.
    fn init_schema(&self) -> Result<()> {
        debug!("Initialising status database");
        self.conn.execute(
            r#"CREATE TABLE IF NOT EXISTS meta (
                "version" INTEGER PRIMARY KEY
            )"#,
        )?;

        let mut q = self
            .conn
            .prepare("INSERT OR IGNORE INTO meta (version) VALUES (?)")?;
        q.bind((1, SCHEMA_VERSION))?;
        q.next()?;
        drop(q);

        self.conn.execute(
            r#"CREATE TABLE IF NOT EXISTS items (
                "ident" TEXT NOT NULL,
                "side" BOOLEAN NOT NULL,
                "href" TEXT NOT NULL,
                "hash" TEXT NOT NULL,
                "etag" TEXT NOT NULL,
                "collection_href" TEXT NOT NULL
            );"#,
        )?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_ident ON items(ident,side)")?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_href ON items(href,side)")?;

        // TODO: Etag nullable is okay?
        self.conn.execute(
            r#"CREATE TABLE IF NOT EXISTS collections (
                "id" TEXT NOT NULL,
                "side" BOOLEAN NOT NULL,
                "href" TEXT NOT NULL,
                "etag" TEXT
            );"#,
        )?;

        // TODO: table for properties
        Ok(())
    }

    pub(super) fn get_item_by_href(&self, side: Side, href: &str) -> Result<Option<ItemState>> {
        let query = "SELECT ident, href, hash, etag FROM items WHERE side = ? AND href = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, side))?;
        statement.bind((2, href))?;

        if let Ok(State::Row) = statement.next() {
            Ok(Some(ItemState {
                href: statement.read::<String, _>("href")?,
                uid: statement.read::<String, _>("ident")?,
                etag: statement.read::<String, _>("etag")?.into(),
                hash: statement.read::<String, _>("hash")?,
            }))
        } else {
            Ok(None)
        }
    }

    pub(super) fn get_item_by_uid(
        &self,
        side: Side,
        collection_href: &str,
        uid: &str,
    ) -> Result<Option<ItemState>> {
        let query = "SELECT ident, href, hash, etag FROM items WHERE side = ? AND ident = ? AND collection_href = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, side))?;
        statement.bind((2, uid))?;
        statement.bind((3, collection_href))?;

        if let Ok(State::Row) = statement.next() {
            Ok(Some(ItemState {
                href: statement.read::<String, _>("href")?,
                uid: statement.read::<String, _>("ident")?,
                etag: statement.read::<String, _>("etag")?.into(),
                hash: statement.read::<String, _>("hash")?,
            }))
        } else {
            Ok(None)
        }
    }

    pub(super) fn all_uids(&self, mapping: &ResolvedMapping) -> Result<Vec<String>> {
        let mut collections = Vec::new();
        if let Some(href) = mapping.collection(Side::A).href() {
            collections.push(href);
        }
        if let Some(href) = mapping.collection(Side::B).href() {
            collections.push(href);
        }

        let query = match collections.len() {
            0 => return Ok(Vec::new()),
            1 => "SELECT DISTINCT ident FROM items WHERE collection_href = ?",
            2 => "SELECT DISTINCT ident FROM items WHERE collection_href IN (?, ?)",
            _ => unreachable!(),
        };

        let mut statement = self.conn.prepare(query)?;
        statement.bind(collections.as_slice())?;

        let mut results = Vec::new();
        while let Ok(State::Row) = statement.next() {
            results.push(statement.read::<String, _>(0)?);
        }

        Ok(results)
    }

    pub(super) fn collection_exists(&self, collection: &ResolvedCollection) -> Result<bool> {
        let mut statement = match collection {
            ResolvedCollection::Id { id } => {
                let query = "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?)";
                let mut statement = self.conn.prepare(query)?;
                statement.bind((1, id.as_ref()))?;
                statement
            }
            ResolvedCollection::Href { href } => {
                let query = "SELECT EXISTS(SELECT 1 FROM collections WHERE href = ?)";
                let mut statement = self.conn.prepare(query)?;
                statement.bind((1, href.as_str()))?;
                statement
            }
        };

        if let State::Row = statement.next()? {
            Ok(statement.read::<i64, _>(0)? == 1)
        } else {
            unreachable!()
        }
    }

    pub(super) fn remove_collection(&self, side: Side, href: &str) -> Result<()> {
        let query = "DELETE FROM collections WHERE side = ? AND href = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, side))?;
        statement.bind((2, href))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn add_collection(&self, side: Side, id: &CollectionId, href: &str) -> Result<()> {
        // TODO: Etag??
        let query = "INSERT INTO collections VALUES (?, ?, ?, ?)";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, id.as_ref()))?;
        statement.bind((2, side))?;
        statement.bind((3, href))?;
        statement.bind((4, None::<&str>))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn get_collection_href(
        &self,
        side: Side,
        id: &CollectionId,
    ) -> Result<Option<String>> {
        let query = "SELECT href FROM collections WHERE side = ? AND id = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, side))?;
        statement.bind((2, id.as_ref()))?;

        if let State::Row = statement.next()? {
            Ok(Some(statement.read::<String, _>(0)?))
        } else {
            Ok(None)
        }
    }

    pub(super) fn add_item(
        &self,
        side: Side,
        collection_href: &str,
        item: &ItemState,
    ) -> Result<()> {
        let query = "INSERT OR REPLACE INTO items VALUES (?, ?, ?, ?, ?, ?)";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, item.uid.as_str()))?;
        statement.bind((2, side))?;
        statement.bind((3, item.href.as_str()))?;
        statement.bind((4, item.hash.as_str()))?;
        statement.bind((5, item.etag.as_ref()))?;
        statement.bind((6, collection_href))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn update_item(
        &self,
        side: Side,
        etag: &Etag,
        hash: &str,
        href: &str,
    ) -> Result<()> {
        let query = "UPDATE items SET etag = ?, hash = ? WHERE side = ? AND href = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, etag.as_ref()))?;
        statement.bind((1, hash))?;
        statement.bind((1, side))?;
        statement.bind((2, href))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn delete_item(&self, uid: &str) -> Result<()> {
        let query = "DELETE FROM items WHERE uid = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, uid))?;
        statement.next()?;
        Ok(())
    }
}
