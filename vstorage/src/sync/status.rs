use std::path::Path;

use log::debug;
use sqlite::{Connection, ConnectionThreadSafe, OpenFlags, State};

use crate::{base::ItemRef, CollectionId, Etag, Href};

use super::plan::ResolvedMapping;

const SCHEMA_VERSION: i64 = 2;

/// Error interacting with status database.
#[derive(thiserror::Error, Debug)]
#[allow(clippy::module_name_repetitions)]
pub enum StatusError {
    #[error("IO error operating with status database: {0}")]
    Io(#[from] sqlite::Error),
}

pub type Result<T> = std::result::Result<T, StatusError>;

#[derive(Clone, Copy, Debug, PartialEq)]
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

    #[must_use]
    pub fn as_char(self) -> char {
        match self {
            Side::A => 'a',
            Side::B => 'b',
        }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_char().fmt(f)
    }
}

/// State for an item at some point in time.
#[derive(PartialEq, Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct ItemState {
    pub(super) href: Href,
    pub(super) uid: String,
    pub(super) etag: Etag,
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

/// A unique ID used for a collection mapping.
#[derive(Clone, Debug, PartialEq)]
pub struct MappingUid(String);

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
        self.conn
            .execute("CREATE TABLE IF NOT EXISTS meta (version INTEGER PRIMARY KEY)")?;

        let mut q = self
            .conn
            .prepare("INSERT OR IGNORE INTO meta (version) VALUES (?)")?;
        q.bind((1, SCHEMA_VERSION))?;
        q.next()?;
        drop(q);

        self.conn.execute(concat!(
            "CREATE TABLE IF NOT EXISTS items (",
            " ident TEXT NOT NULL,",
            // TODO: should be foreign key to collections(uid).
            " mapping_uid TEXT NOT NULL,",
            " hash TEXT NOT NULL,",
            " href_a TEXT NOT NULL,",
            " etag_a TEXT NOT NULL,",
            " href_b TEXT NOT NULL,",
            " etag_b TEXT NOT NULL",
            ")"
        ))?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_ident ON items(ident, mapping_uid)")?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_href ON items(href_a)")?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_href ON items(href_b)")?;

        // TODO: Etag nullable is okay?
        self.conn.execute(concat!(
            "CREATE TABLE IF NOT EXISTS collections (",
            " uid INTEGER PRIMARY KEY AUTOINCREMENT,",
            " id_a TEXT,",
            " href_a TEXT NOT NULL,",
            " id_b TEXT,",
            " href_b TEXT NOT NULL",
            ")",
        ))?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS href_a ON collections(href_a)")?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS href_b ON collections(href_b)")?;

        // TODO: table for properties
        Ok(())
    }

    pub(super) fn get_item_by_href(&self, side: Side, href: &str) -> Result<Option<ItemState>> {
        let query = vec![
            &format!("SELECT ident, href_{side} AS href, hash, etag_{side} AS etag"),
            " FROM items",
            " WHERE href = ?",
        ]
        .into_iter()
        .collect::<String>();
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, href))?;

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

    pub(super) fn get_items_by_uid(
        &self,
        mapping_uid: &MappingUid,
        uid: &str,
    ) -> Result<Option<(ItemState, ItemState)>> {
        let query = concat!(
            "SELECT ident, hash, href_a, etag_a, href_b, etag_b",
            " FROM items",
            " WHERE AND ident = ? AND mapping_uid = ?"
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, uid))?;
        statement.bind((2, mapping_uid.0.as_str()))?;

        if let Ok(State::Row) = statement.next() {
            Ok(Some((
                ItemState {
                    href: statement.read::<String, _>("href_a")?,
                    uid: statement.read::<String, _>("ident")?,
                    etag: statement.read::<String, _>("etag_a")?.into(),
                    hash: statement.read::<String, _>("hash")?,
                },
                ItemState {
                    href: statement.read::<String, _>("href_b")?,
                    uid: statement.read::<String, _>("ident")?,
                    etag: statement.read::<String, _>("etag_b")?.into(),
                    hash: statement.read::<String, _>("hash")?,
                },
            )))
        } else {
            Ok(None)
        }
    }

    pub(super) fn all_uids(&self, mapping: &ResolvedMapping) -> Result<Vec<String>> {
        let query = "SELECT DISTINCT ident FROM items WHERE mapping_uid IN (?, ?)";

        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, mapping.collection(Side::A).href().as_str()))?;
        statement.bind((2, mapping.collection(Side::B).href().as_str()))?;

        let mut results = Vec::new();
        while let Ok(State::Row) = statement.next() {
            results.push(statement.read::<String, _>(0)?);
        }

        Ok(results)
    }

    pub(super) fn get_mapping_uid(&self, mapping: &ResolvedMapping) -> Result<Option<MappingUid>> {
        let query = "SELECT uid FROM collections WHERE href_a = ? AND href_b = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, mapping.collection(Side::A).href().as_str()))?;
        statement.bind((2, mapping.collection(Side::B).href().as_str()))?;

        if let State::Row = statement.next()? {
            Ok(Some(MappingUid(statement.read::<String, _>("href")?)))
        } else {
            Ok(None)
        }
    }

    pub(super) fn remove_collection(&self, mapping_uid: &MappingUid) -> Result<()> {
        let query = "DELETE FROM collections WHERE uid = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, mapping_uid.0.as_str()))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn get_or_add_collection(
        &self,
        href_a: &str,
        href_b: &str,
        id_a: Option<&CollectionId>,
        id_b: Option<&CollectionId>,
    ) -> Result<MappingUid> {
        let query = concat!(
            "INSERT OR IGNORE INTO collections(id_a, href_a, id_b, href_b)",
            " VALUES (?, ?, ?, ?)"
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, id_a.map(CollectionId::as_ref)))?;
        statement.bind((2, href_a))?;
        statement.bind((3, id_b.map(CollectionId::as_ref)))?;
        statement.bind((4, href_b))?;
        statement.next()?;

        let query = "SELECT uid FROM collections WHERE href_a = ? AND href_b = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, href_a))?;
        statement.bind((2, href_b))?;

        if let State::Row = statement.next()? {
            Ok(MappingUid(statement.read::<String, _>("uid")?))
        } else {
            unreachable!("uid missing for mapping immediately after INSERT");
        }
    }

    pub(super) fn insert_item(
        &self,
        mapping_uid: &MappingUid,
        uid: &str,
        hash: &str,
        ref_a: &ItemRef,
        ref_b: &ItemRef,
    ) -> Result<()> {
        let query = concat!(
            "INSERT INTO items(ident, mapping_uid, hash, href_a, etag_a, href_b, etag_b)",
            " VALUES (?, ?, ?, ?, ?, ?, ?)"
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, uid))?;
        statement.bind((2, mapping_uid.0.as_str()))?;
        statement.bind((3, hash))?;

        statement.bind((4, ref_a.href.as_str()))?;
        statement.bind((5, ref_a.etag.as_ref()))?;
        statement.bind((6, ref_b.href.as_str()))?;
        statement.bind((7, ref_b.etag.as_ref()))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn update_item(
        &self,
        hash: &str,
        etag_a: &Etag,
        href_a: &str,
        etag_b: &Etag,
        href_b: &str,
    ) -> Result<()> {
        // Here we update by href to avoid issue with items in other collections with matching UID.
        let query = concat!(
            "UPDATE items SET hash = ?, etag_a = ?, etag_b = ?",
            " WHERE href_a = ? AND href_b = ?"
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, hash))?;
        statement.bind((2, etag_a.as_ref()))?;
        statement.bind((3, etag_b.as_ref()))?;
        statement.bind((4, href_a))?;
        statement.bind((5, href_b))?;
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
