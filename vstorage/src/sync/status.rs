//! Types for storing status between synchronisations.
use std::path::Path;

use log::{debug, error};
use sqlite::{Connection, ConnectionThreadSafe, OpenFlags, State};

use crate::{base::ItemRef, CollectionId, Etag, Href};

const SCHEMA_VERSION: i64 = 2;

/// Error interacting with status database.
#[derive(thiserror::Error, Debug)]
pub enum StatusError {
    #[error("IO error operating with status database: {0}")]
    Io(#[from] sqlite::Error),
    #[error("UPDATE did no affect any rows")]
    NoUpdate,
}

/// Storages are synchronised between two "sides", 'a' or 'b'.
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

/// State for an item at some point in time in a single collection.
#[derive(PartialEq, Clone, Debug)]
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
pub(super) struct MappingUid(i64);

/// Connection to an on-disk status database.
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
    /// Returns [`StatusError::Io`] if sqlite fails to open the database.
    pub fn open_readonly(path: impl AsRef<Path>) -> Result<Option<StatusDatabase>, StatusError> {
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
    /// Returns [`StatusError::Io`] if sqlite fails to open or create the database.
    pub fn open_or_create(path: impl AsRef<Path>) -> Result<StatusDatabase, StatusError> {
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
    fn init_schema(&self) -> Result<(), StatusError> {
        debug!("Ensuring that status database is initialised.");
        self.conn
            .execute("CREATE TABLE IF NOT EXISTS meta (version INTEGER PRIMARY KEY)")?;

        let mut q = self
            .conn
            .prepare("INSERT OR IGNORE INTO meta (version) VALUES (?)")?;
        q.bind((1, SCHEMA_VERSION))?;
        q.next()?;

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
        // TODO: duplicate ids are not allowed.
        // FIXME: this also needs to be addressed in the discovery layer.

        self.conn.execute(concat!(
            "CREATE TABLE IF NOT EXISTS items (",
            " ident TEXT NOT NULL,",
            " mapping_uid TEXT NOT NULL,",
            " hash TEXT NOT NULL,",
            " href_a TEXT NOT NULL,",
            " etag_a TEXT NOT NULL,",
            " href_b TEXT NOT NULL,",
            " etag_b TEXT NOT NULL,",
            " FOREIGN KEY(mapping_uid) REFERENCES collections(uid)",
            ")"
        ))?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_ident ON items(ident, mapping_uid)")?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_href ON items(href_a)")?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_href ON items(href_b)")?;

        // TODO: table for properties
        Ok(())
    }

    pub(super) fn get_item_by_href(
        &self,
        side: Side,
        href: &str,
    ) -> Result<Option<ItemState>, StatusError> {
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

    pub(super) fn get_item_hash_by_uid(
        &self,
        mapping_uid: &MappingUid,
        uid: &str,
    ) -> Result<Option<String>, StatusError> {
        let query = concat!("SELECT hash FROM items WHERE ident = ? AND mapping_uid = ?");
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, uid))?;
        statement.bind((2, mapping_uid.0))?;

        if let Ok(State::Row) = statement.next() {
            Ok(Some(statement.read::<String, _>("hash")?))
        } else {
            Ok(None)
        }
    }

    pub(super) fn all_uids(&self, mapping: &MappingUid) -> Result<Vec<String>, StatusError> {
        let query = "SELECT DISTINCT ident FROM items WHERE mapping_uid = ?";

        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, mapping.0))?;

        let mut results = Vec::new();
        while let Ok(State::Row) = statement.next() {
            results.push(statement.read::<String, _>(0)?);
        }

        Ok(results)
    }

    pub(super) fn get_mapping_uid(
        &self,
        href_a: &Href,
        href_b: &Href,
    ) -> Result<Option<MappingUid>, StatusError> {
        let query = "SELECT uid FROM collections WHERE href_a = ? AND href_b = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, href_a.as_str()))?;
        statement.bind((2, href_b.as_str()))?;

        if let State::Row = statement.next()? {
            Ok(Some(MappingUid(statement.read::<i64, _>("uid")?)))
        } else {
            Ok(None)
        }
    }

    pub(super) fn remove_collection(&self, mapping_uid: &MappingUid) -> Result<(), StatusError> {
        let query = "DELETE FROM collections WHERE uid = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, mapping_uid.0))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn get_or_add_collection(
        &self,
        href_a: &str,
        href_b: &str,
        id_a: Option<&CollectionId>,
        id_b: Option<&CollectionId>,
    ) -> Result<MappingUid, StatusError> {
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
            Ok(MappingUid(statement.read::<i64, _>("uid")?))
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
    ) -> Result<(), StatusError> {
        let query = concat!(
            "INSERT INTO items(ident, mapping_uid, hash, href_a, etag_a, href_b, etag_b)",
            " VALUES (?, ?, ?, ?, ?, ?, ?)"
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, uid))?;
        statement.bind((2, mapping_uid.0))?;
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
    ) -> Result<(), StatusError> {
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

        if self.conn.change_count() == 0 {
            error!("update_item did not affect any rows! href_a: {href_a}, href_b: {href_b}");
            Err(StatusError::NoUpdate)
        } else {
            Ok(())
        }
    }

    pub(super) fn delete_item(
        &self,
        mapping_uid: &MappingUid,
        uid: &str,
    ) -> Result<(), StatusError> {
        let query = "DELETE FROM items WHERE mapping_uid = ? AND ident = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, mapping_uid.0))?;
        statement.bind((2, uid))?;
        statement.next()?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::{base::ItemRef, sync::status::StatusError, CollectionId};

    use super::{MappingUid, Side, StatusDatabase};

    #[test]
    fn test_writing_in_readonly_mode() {
        let db = StatusDatabase::open_readonly(":memory:").unwrap();
        let err = db.unwrap().init_schema().unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("attempt to write a readonly database"));
    }

    #[test]
    fn test_insert_and_get_item() {
        let db = StatusDatabase::open_or_create(":memory:").unwrap();
        let mapping_uid = MappingUid(1);
        let uid = "07da74e5-0a32-482a-bbdd-13fd1e45cce3";
        let hash = "HASH";
        let item_a = ItemRef {
            href: "/collections/work/item.ics".into(),
            etag: "123".into(),
        };
        let item_b = ItemRef {
            href: "work/item.ics".into(),
            etag: "abc000".into(),
        };
        db.insert_item(&mapping_uid, uid, hash, &item_a, &item_b)
            .unwrap();

        let item_a_fetched = db.get_item_by_href(Side::A, &item_a.href).unwrap().unwrap();
        assert_eq!(item_a_fetched.uid, uid);
        assert_eq!(item_a_fetched.hash, hash);
        assert_eq!(item_a_fetched.href, item_a.href);
        assert_eq!(item_a_fetched.etag, item_a.etag);

        let item_b_fetched = db.get_item_by_href(Side::B, &item_b.href).unwrap().unwrap();
        assert_eq!(item_b_fetched.uid, uid);
        assert_eq!(item_b_fetched.hash, hash);
        assert_eq!(item_b_fetched.href, item_b.href);
        assert_eq!(item_b_fetched.etag, item_b.etag);

        let fetched_hash = db
            .get_item_hash_by_uid(&mapping_uid, uid)
            .unwrap()
            .expect("status should return items that were just inserted");
        assert_eq!(fetched_hash, hash);

        let all = db.all_uids(&mapping_uid).unwrap();
        let all_expected = vec![uid];
        assert_eq!(all, all_expected);

        db.delete_item(&mapping_uid, uid).unwrap();
        assert!(db
            .get_item_by_href(Side::A, &item_a.href)
            .unwrap()
            .is_none());
        assert!(db
            .get_item_by_href(Side::B, &item_b.href)
            .unwrap()
            .is_none());
        assert!(db
            .get_item_hash_by_uid(&mapping_uid, uid)
            .unwrap()
            .is_none());
        assert!(db.all_uids(&mapping_uid).unwrap().is_empty());
    }
    #[test]
    fn test_insert_update_and_get_item() {
        let db = StatusDatabase::open_or_create(":memory:").unwrap();
        let mapping_uid = MappingUid(1);
        let uid = "07da74e5-0a32-482a-bbdd-13fd1e45cce3";
        let hash = "HASH";
        let item_a = ItemRef {
            href: "/collections/work/item.ics".into(),
            etag: "123".into(),
        };
        let item_b = ItemRef {
            href: "work/item.ics".into(),
            etag: "abc000".into(),
        };
        db.insert_item(&mapping_uid, uid, hash, &item_a, &item_b)
            .unwrap();

        let updated_hash = "ANOTHERHASH";
        let updated_etag_a = "456".into();
        let updated_etag_b = "def111".into();
        db.update_item(
            updated_hash,
            &updated_etag_a,
            &item_a.href,
            &updated_etag_b,
            &item_b.href,
        )
        .unwrap();

        let item_a_fetched = db.get_item_by_href(Side::A, &item_a.href).unwrap().unwrap();
        assert_eq!(item_a_fetched.uid, uid);
        assert_eq!(item_a_fetched.hash, updated_hash);
        assert_eq!(item_a_fetched.href, item_a.href);
        assert_eq!(item_a_fetched.etag, updated_etag_a);

        let item_b_fetched = db.get_item_by_href(Side::B, &item_b.href).unwrap().unwrap();
        assert_eq!(item_b_fetched.uid, uid);
        assert_eq!(item_b_fetched.hash, updated_hash);
        assert_eq!(item_b_fetched.href, item_b.href);
        assert_eq!(item_b_fetched.etag, updated_etag_b);

        let fetched_hash = db
            .get_item_hash_by_uid(&mapping_uid, uid)
            .unwrap()
            .expect("status should return items that were just inserted");
        assert_eq!(fetched_hash, updated_hash);

        let all = db.all_uids(&mapping_uid).unwrap();
        let all_expected = vec![uid];
        assert_eq!(all, all_expected);
    }
    #[test]
    fn test_wrong_update() {
        let db = StatusDatabase::open_or_create(":memory:").unwrap();
        let mapping_uid = MappingUid(1);
        let uid = "07da74e5-0a32-482a-bbdd-13fd1e45cce3";
        let hash = "HASH";
        let item_a = ItemRef {
            href: "/collections/work/item.ics".into(),
            etag: "123".into(),
        };
        let item_b = ItemRef {
            href: "work/item.ics".into(),
            etag: "abc000".into(),
        };
        db.insert_item(&mapping_uid, uid, hash, &item_a, &item_b)
            .unwrap();

        let updated_hash = "ANOTHERHASH";
        let updated_etag_a = "456".into();
        let updated_etag_b = "def111".into();
        let err = db
            .update_item(
                updated_hash,
                &updated_etag_a,
                &"not/correct.ics",
                &updated_etag_b,
                &item_b.href,
            )
            .unwrap_err();
        assert!(matches!(err, StatusError::NoUpdate));
    }

    #[test]
    fn test_add_and_get_collection() {
        let db = StatusDatabase::open_or_create(":memory:").unwrap();
        let collection_id = "guests".parse::<CollectionId>().unwrap();
        let href_a = "/collections/guests";
        let href_b = "guests";
        let mapping_uid = db
            .get_or_add_collection(href_a, href_b, Some(&collection_id), Some(&collection_id))
            .unwrap();

        let gotten_uid = db
            .get_mapping_uid(&href_a.to_string(), &href_b.to_string())
            .unwrap()
            .expect("should obtain mapping that was just inserted");
        assert_eq!(mapping_uid, gotten_uid);

        db.remove_collection(&mapping_uid).unwrap();

        let gotten_uid = db
            .get_mapping_uid(&href_a.to_string(), &href_b.to_string())
            .unwrap();
        assert!(gotten_uid.is_none());
    }
}
