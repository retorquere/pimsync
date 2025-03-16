//! Store and query status between synchronisations.
use std::{fs::create_dir_all, path::Path};

use log::{debug, error};
use sqlite::{Connection, ConnectionThreadSafe, OpenFlags, State};

use crate::{
    base::{Item, ItemRef},
    util::{ItemHash, ItemHashError},
    Etag, Href,
};

// const SCHEMA_VERSION: i64 = 3;

/// Error interacting with status database.
#[derive(thiserror::Error, Debug)]
pub enum StatusError {
    #[error("Error interacting with sqlite backend: {0}")]
    Sqlite(#[from] sqlite::Error),
    #[error("UPDATE did no affect any rows")]
    NoUpdate,
    #[error("Could not create parent directories")]
    ParentDirs(#[source] std::io::Error),
    #[error("Status DB contained an invalid hash")]
    InvalidHash(#[from] ItemHashError),
}

#[derive(thiserror::Error, Debug)]
#[error("Finding stale mapping: {0}")]
pub struct FindStaleMappingsError(#[from] sqlite::Error);

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

/// State for an item at some point in time.
#[derive(PartialEq, Clone, Debug)]
pub struct ItemState<I: Item> {
    pub href: Href,
    pub uid: String,
    pub etag: Etag,
    pub hash: ItemHash,
    pub data: Option<I>,
}

impl<I: Item> PartialEq<ItemRef> for ItemState<I> {
    fn eq(&self, other: &ItemRef) -> bool {
        self.href.eq(&other.href) && self.etag.eq(&other.etag)
    }
}

impl<I: Item> ItemState<I> {
    /// Create an `ItemRef` by copying the `href` and `etag`.
    #[must_use]
    pub fn to_item_ref(&self) -> ItemRef {
        ItemRef {
            href: self.href.clone(),
            etag: self.etag.clone(),
        }
    }
}

impl<I: Item> From<ItemState<I>> for ItemRef {
    fn from(value: ItemState<I>) -> Self {
        ItemRef {
            href: value.href,
            etag: value.etag,
        }
    }
}

/// The status for an item as retrieved from a [`StatusDatabase`].
pub(super) struct StatusForItem {
    pub(super) hash: ItemHash,
    pub(super) a: ItemRef,
    pub(super) b: ItemRef,
}

impl StatusForItem {
    #[must_use]
    pub fn into_item_refs(self) -> (ItemRef, ItemRef) {
        (self.a, self.b)
    }
}

pub(super) struct PropertyStatus {
    // The name of the property
    pub(super) property: String,
    pub(super) value: String,
}

/// A unique ID used for a mapping between two collections.
///
/// This is an opaque identifier, and can only be obtained from a status database.
#[derive(Clone, Debug, PartialEq, Copy)]
pub struct MappingUid(i64);

/// Connection to an on-disk status database.
pub struct StatusDatabase {
    conn: ConnectionThreadSafe,
}

impl StatusDatabase {
    // NOTE: side is stored as boolean, 0=a, 1=b.

    /// Open the database if it does not exists.
    ///
    /// Returns `None` if the database does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`StatusError::Sqlite`] if sqlite fails to open the database.
    pub fn open(path: impl AsRef<Path>) -> Result<Option<StatusDatabase>, StatusError> {
        let flags = OpenFlags::new().with_read_write().with_full_mutex();
        match Connection::open_thread_safe_with_flags(path, flags) {
            Ok(conn) => Ok(Some(StatusDatabase { conn })),
            Err(e) if e.code == Some(14) => Ok(None),
            Err(e) => Err(StatusError::Sqlite(e)),
        }
    }

    /// Open or creates the database in read-write mode.
    ///
    /// # Errors
    ///
    /// Returns [`StatusError::Sqlite`] if sqlite fails to open or create the database.
    pub fn open_or_create(path: impl AsRef<Path>) -> Result<StatusDatabase, StatusError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            create_dir_all(parent).map_err(StatusError::ParentDirs)?;
        }

        let db = StatusDatabase {
            conn: Connection::open_thread_safe(path)?,
        };

        db.init_schema()?;
        db.conn.execute("PRAGMA foreign_keys = ON")?;

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

        self.conn
            .execute("INSERT OR IGNORE INTO meta (version) VALUES (2)")?;

        self.conn.execute(concat!(
            "CREATE TABLE IF NOT EXISTS collections (",
            " uid INTEGER PRIMARY KEY AUTOINCREMENT,",
            " id_a TEXT,", // Removed in schema 2.
            " href_a TEXT NOT NULL,",
            " id_b TEXT,", // Removed in schema 2.
            " href_b TEXT NOT NULL",
            ")",
        ))?;

        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS href_a ON collections(href_a)")?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS href_b ON collections(href_b)")?;

        // TODO: Etag should also be NOT NULL
        self.conn.execute(concat!(
            "CREATE TABLE IF NOT EXISTS items (",
            " ident TEXT NOT NULL,",
            " mapping_uid TEXT NOT NULL,",
            " hash TEXT NOT NULL,",
            " href_a TEXT NOT NULL,",
            " etag_a TEXT,",
            " href_b TEXT NOT NULL,",
            " etag_b TEXT,",
            " FOREIGN KEY(mapping_uid) REFERENCES collections(uid)",
            ")"
        ))?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_ident ON items(ident, mapping_uid)")?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_href ON items(href_a)")?;
        self.conn
            .execute("CREATE UNIQUE INDEX IF NOT EXISTS by_href ON items(href_b)")?;

        self.conn.execute(concat!(
            "CREATE TABLE IF NOT EXISTS properties (",
            " mapping_uid INTEGER NOT NULL,",
            " href_a TEXT NOT NULL,",
            " href_b TEXT NOT NULL,",
            " property TEXT NOT NULL,",
            " value TEXT NOT NULL,",
            " FOREIGN KEY(mapping_uid) REFERENCES collections(uid)",
            ")"
        ))?;
        self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS by_uid ON properties(mapping_uid, property)",
        )?;
        self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS by_href_a ON properties(href_a, property)",
        )?;
        self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS by_href_b ON properties(href_b, property)",
        )?;

        if self.get_schema_version()? < 3 {
            self.conn.execute("BEGIN TRANSACTION")?;
            self.conn
                .execute("ALTER TABLE collections DROP COLUMN id_a")?;
            self.conn
                .execute("ALTER TABLE collections DROP COLUMN id_b")?;
            self.conn.execute("INSERT INTO meta (version) VALUES (3)")?;
            self.conn.execute("COMMIT TRANSACTION")?;
        }
        Ok(())
    }

    /// Returns an `ItemState`, if it exists AND has an Etag.
    pub(super) fn get_item_by_href<I: Item>(
        &self,
        side: Side,
        href: &str,
        mapping_uid: MappingUid,
    ) -> Result<Option<ItemState<I>>, StatusError> {
        let query = vec![
            &format!("SELECT ident, href_{side} AS href, hash, etag_{side} AS etag"),
            " FROM items",
            &format!(" WHERE href = ? AND etag_{side} IS NOT NULL AND mapping_uid = ?"),
        ]
        .into_iter()
        .collect::<String>();
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, href))?;
        statement.bind((2, mapping_uid.0))?;

        if let Ok(State::Row) = statement.next() {
            let Some(etag) = statement.read::<Option<String>, _>("etag")? else {
                return Ok(None);
            };
            Ok(Some(ItemState {
                href: statement.read::<String, _>("href")?,
                uid: statement.read::<String, _>("ident")?,
                etag: Etag::from(etag),
                hash: statement.read::<String, _>("hash")?.parse()?,
                data: None,
            }))
        } else {
            Ok(None)
        }
    }

    pub(super) fn get_item_hash_by_uid(
        &self,
        mapping_uid: MappingUid,
        uid: &str,
    ) -> Result<Option<StatusForItem>, StatusError> {
        let query = concat!(
            "SELECT hash, etag_a, etag_b, href_a, href_b",
            " FROM items WHERE ident = ? AND mapping_uid = ?"
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, uid))?;
        statement.bind((2, mapping_uid.0))?;

        if let Ok(State::Row) = statement.next() {
            Ok(Some(StatusForItem {
                hash: statement.read::<String, _>("hash")?.parse()?,
                a: ItemRef {
                    href: statement.read::<String, _>("href_a")?,
                    etag: statement.read::<String, _>("etag_a")?.into(),
                },
                b: ItemRef {
                    etag: statement.read::<String, _>("etag_b")?.into(),
                    href: statement.read::<String, _>("href_b")?,
                },
            }))
        } else {
            Ok(None)
        }
    }

    pub(super) fn all_uids(&self, mapping: MappingUid) -> Result<Vec<String>, StatusError> {
        let query = "SELECT DISTINCT ident FROM items WHERE mapping_uid = ?";

        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, mapping.0))?;

        let mut results = Vec::new();
        while let Ok(State::Row) = statement.next() {
            results.push(statement.read::<String, _>(0)?);
        }

        Ok(results)
    }

    /// Get the (internal) UID for a collection mapping.
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

    pub(super) fn remove_collection(&self, mapping_uid: MappingUid) -> Result<(), StatusError> {
        let query = "DELETE FROM collections WHERE uid = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, mapping_uid.0))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn add_collection(
        &self,
        href_a: &str,
        href_b: &str,
    ) -> Result<MappingUid, StatusError> {
        let query = concat!(
            "INSERT INTO collections(href_a, href_b)",
            " VALUES (?, ?)",
            " RETURNING uid",
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, href_a))?;
        statement.bind((2, href_b))?;

        if statement.next()? == State::Row {
            Ok(MappingUid(statement.read::<i64, _>("uid")?))
        } else {
            unreachable!("INSERT INTO .. RETURNING must always return a value.");
        }
    }

    pub(super) fn get_or_add_collection(
        &self,
        href_a: &str,
        href_b: &str,
    ) -> Result<MappingUid, StatusError> {
        let query = concat!(
            "INSERT OR IGNORE INTO collections(href_a, href_b)",
            " VALUES (?, ?)",
            " RETURNING uid",
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, href_a))?;
        statement.bind((2, href_b))?;
        // If a uid was returned (just inserted) return that.
        if statement.next()? == State::Row {
            return Ok(MappingUid(statement.read::<i64, _>("uid")?));
        };

        let query = "SELECT uid FROM collections WHERE href_a = ? AND href_b = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, href_a))?;
        statement.bind((2, href_b))?;

        if let State::Row = statement.next()? {
            Ok(MappingUid(statement.read::<i64, _>("uid")?))
        } else {
            // FIXME: reachable if existing an existing HREF conflicted only one row.
            unreachable!("uid missing for mapping immediately after INSERT");
        }
    }

    pub(super) fn insert_item(
        &self,
        mapping_uid: MappingUid,
        uid: &str,
        hash: &ItemHash,
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
        statement.bind((3, hash.to_string().as_str()))?;
        statement.bind((4, ref_a.href.as_str()))?;
        statement.bind((5, ref_a.etag.as_ref()))?;
        statement.bind((6, ref_b.href.as_str()))?;
        statement.bind((7, ref_b.etag.as_ref()))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn update_item(
        &self,
        new_hash: &ItemHash,
        old_a: &ItemRef,
        old_b: &ItemRef,
        new_a: &ItemRef,
        new_b: &ItemRef,
    ) -> Result<(), StatusError> {
        // Items in other collections may have the same UID, so update by href.
        let query = concat!(
            "UPDATE items",
            " SET hash = :hash, etag_a = :new_etag_a, etag_b = :new_etag_b, href_a = :new_href_a, href_b = :new_href_b",
            " WHERE href_a = :old_href_a AND href_b = :old_href_b AND etag_a = :old_etag_a AND etag_b = :old_etag_b"
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((":hash", new_hash.to_string().as_str()))?;

        statement.bind((":new_href_a", new_a.href.as_str()))?;
        statement.bind((":new_href_b", new_b.href.as_str()))?;
        statement.bind((":new_etag_a", Some(new_a.etag.as_str())))?;
        statement.bind((":new_etag_b", Some(new_b.etag.as_str())))?;

        statement.bind((":old_href_a", old_a.href.as_str()))?;
        statement.bind((":old_href_b", old_b.href.as_str()))?;
        statement.bind((":old_etag_a", Some(old_a.etag.as_str())))?;
        statement.bind((":old_etag_b", Some(old_b.etag.as_str())))?;
        statement.next()?;

        if self.conn.change_count() == 0 {
            error!("update_item did not affect any rows! old_a: {old_a:?}, old_b: {old_b:?}");
            Err(StatusError::NoUpdate)
        } else {
            Ok(())
        }
    }

    pub(super) fn delete_item(
        &self,
        mapping_uid: MappingUid,
        uid: &str,
        // FIXME: should take etag, in case another instance raced us and updated the item?
    ) -> Result<(), StatusError> {
        let query = "DELETE FROM items WHERE mapping_uid = ? AND ident = ?";
        let mut statement = self.conn.prepare(query)?;
        statement.bind((1, mapping_uid.0))?;
        statement.bind((2, uid))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn list_properties_for_collection(
        &self,
        mapping_uid: MappingUid,
    ) -> Result<Vec<PropertyStatus>, StatusError> {
        let query = concat!(
            "SELECT property, value",
            " FROM properties",
            " WHERE mapping_uid = :mapping_uid"
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((":mapping_uid", mapping_uid.0))?;

        let mut results = Vec::new();
        while let Ok(State::Row) = statement.next() {
            results.push(PropertyStatus {
                property: statement.read::<String, _>("property")?,
                value: statement.read::<String, _>("value")?,
            });
        }
        Ok(results)
    }

    pub(super) fn set_property(
        &self,
        mapping_uid: MappingUid,
        href_a: &str,
        href_b: &str,
        property: &str,
        value: &str,
    ) -> Result<(), StatusError> {
        let query = concat!(
            "INSERT OR REPLACE INTO properties (mapping_uid, href_a, href_b, property, value) ",
            "VALUES (:mapping_uid, :href_a, :href_b, :property, :value)",
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((":mapping_uid", mapping_uid.0))?;
        statement.bind((":href_a", href_a))?;
        statement.bind((":href_b", href_b))?;
        statement.bind((":property", property))?;
        statement.bind((":value", value))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn delete_property(
        &self,
        mapping_uid: MappingUid,
        href_a: &str,
        href_b: &str,
        property: &str,
    ) -> Result<(), StatusError> {
        let query = concat!(
            "DELETE FROM properties",
            " WHERE mapping_uid = :mapping_uid AND href_a = :href_a AND href_b = :href_b",
            " AND property = :property",
        );
        let mut statement = self.conn.prepare(query)?;
        statement.bind((":mapping_uid", mapping_uid.0))?;
        statement.bind((":href_a", href_a))?;
        statement.bind((":href_b", href_b))?;
        statement.bind((":property", property))?;
        statement.next()?;
        Ok(())
    }

    pub(super) fn find_stale_mappings(
        &self,
        active: impl Iterator<Item = MappingUid>,
    ) -> Result<Vec<MappingUid>, FindStaleMappingsError> {
        let params = active
            .map(|uid| uid.0.to_string())
            .collect::<Vec<_>>()
            .join(",");

        // No support for arrays. See: https://github.com/stainless-steel/sqlite/issues/38
        // The replacement String is constructed inside this function and its input is an i64.
        let query = "SELECT uid FROM collections WHERE uid NOT IN (?)".replace('?', &params);
        let mut statement = self.conn.prepare(query)?;

        let mut results = Vec::new();
        while let Ok(State::Row) = statement.next() {
            results.push(MappingUid(statement.read::<i64, _>("uid")?));
        }
        Ok(results)
    }

    pub(super) fn flush_stale_mappings(
        &self,
        active: impl IntoIterator<Item = MappingUid>,
    ) -> Result<(), StatusError> {
        let params = active
            .into_iter()
            .map(|uid| uid.0.to_string())
            .collect::<Vec<_>>()
            .join(",");

        // No support for arrays. See: https://github.com/stainless-steel/sqlite/issues/38
        // The replacement String is constructed inside this function and its input is an i64.
        self.conn
            .execute("DELETE FROM items WHERE mapping_uid IN (?)".replace('?', &params))?;
        self.conn
            .execute("DELETE FROM properties WHERE mapping_uid IN (?)".replace('?', &params))?;
        self.conn
            .execute("DELETE FROM collections WHERE uid IN (?)".replace('?', &params))?;

        Ok(())
    }

    fn get_schema_version(&self) -> Result<i64, StatusError> {
        let mut statement = self.conn.prepare("SELECT MAX(version) AS ver FROM meta")?;

        if let State::Row = statement.next()? {
            Ok(statement.read::<i64, _>("ver")?)
        } else {
            Ok(0)
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{base::ItemRef, calendar::IcsItem, sync::status::StatusError, Etag};

    use super::{MappingUid, Side, StatusDatabase};

    #[test]
    fn test_open_non_existant() {
        let db = StatusDatabase::open("/doesnotexist/status/my.db").unwrap();
        assert!(db.is_none());
    }

    #[test]
    fn test_insert_and_get_item() {
        let db = StatusDatabase::open_or_create(":memory:").unwrap();
        let mapping_uid = MappingUid(1);
        let uid = "07da74e5-0a32-482a-bbdd-13fd1e45cce3";
        let hash = "133ee989293f92736301280c6f14c89d521200c17dcdcecca30cd20705332d44"
            .parse()
            .unwrap();
        let item_a = ItemRef {
            href: "/collections/work/item.ics".into(),
            etag: "123".into(),
        };
        let item_b = ItemRef {
            href: "work/item.ics".into(),
            etag: "abc000".into(),
        };
        db.get_or_add_collection("/collections/work/", "work")
            .unwrap();
        db.insert_item(mapping_uid, uid, &hash, &item_a, &item_b)
            .unwrap();

        let item_a_fetched = db
            .get_item_by_href::<IcsItem>(Side::A, &item_a.href, mapping_uid)
            .unwrap()
            .unwrap();
        assert_eq!(item_a_fetched.uid, uid);
        assert_eq!(item_a_fetched.hash, hash);
        assert_eq!(item_a_fetched.href, item_a.href);
        assert_eq!(item_a_fetched.etag, item_a.etag);

        let item_b_fetched = db
            .get_item_by_href::<IcsItem>(Side::B, &item_b.href, mapping_uid)
            .unwrap()
            .unwrap();
        assert_eq!(item_b_fetched.uid, uid);
        assert_eq!(item_b_fetched.hash, hash);
        assert_eq!(item_b_fetched.href, item_b.href);
        assert_eq!(item_b_fetched.etag, item_b.etag);

        let item_status = db
            .get_item_hash_by_uid(mapping_uid, uid)
            .unwrap()
            .expect("status should return items that were just inserted");
        assert_eq!(item_status.hash, hash);

        let all = db.all_uids(mapping_uid).unwrap();
        let all_expected = vec![uid];
        assert_eq!(all, all_expected);

        db.delete_item(mapping_uid, uid).unwrap();
        assert!(db
            .get_item_by_href::<IcsItem>(Side::A, &item_a.href, mapping_uid)
            .unwrap()
            .is_none());
        assert!(db
            .get_item_by_href::<IcsItem>(Side::B, &item_b.href, mapping_uid)
            .unwrap()
            .is_none());
        assert!(db.get_item_hash_by_uid(mapping_uid, uid).unwrap().is_none());
        assert!(db.all_uids(mapping_uid).unwrap().is_empty());
    }
    #[test]
    fn test_insert_update_and_get_item() {
        let db = StatusDatabase::open_or_create(":memory:").unwrap();
        let mapping_uid = MappingUid(1);
        let uid = "07da74e5-0a32-482a-bbdd-13fd1e45cce3";
        let hash = "0000000000000000000000000000000000000000000000000000000000000000"
            .parse()
            .unwrap();
        let item_a = ItemRef {
            href: "/collections/work/item.ics".into(),
            etag: "123".into(),
        };
        let item_b = ItemRef {
            href: "work/item.ics".into(),
            etag: "abc000".into(),
        };
        db.get_or_add_collection("/collections/work/", "work")
            .unwrap();
        db.insert_item(mapping_uid, uid, &hash, &item_a, &item_b)
            .unwrap();

        let updated_hash = "1111111111111111111111111111111111111111111111111111111111111111"
            .parse()
            .unwrap();
        let updated_etag_a = Etag::from("456");
        let updated_etag_b = Etag::from("def111");
        db.update_item(
            &updated_hash,
            &item_a,
            &item_b,
            &ItemRef {
                etag: updated_etag_a.clone(),
                href: item_a.href.clone(),
            },
            &ItemRef {
                etag: updated_etag_b.clone(),
                href: item_b.href.clone(),
            },
        )
        .unwrap();

        let item_a_fetched = db
            .get_item_by_href::<IcsItem>(Side::A, &item_a.href, mapping_uid)
            .unwrap()
            .unwrap();
        assert_eq!(item_a_fetched.uid, uid);
        assert_eq!(item_a_fetched.hash, updated_hash);
        assert_eq!(item_a_fetched.href, item_a.href);
        assert_eq!(item_a_fetched.etag, updated_etag_a);

        let item_b_fetched = db
            .get_item_by_href::<IcsItem>(Side::B, &item_b.href, mapping_uid)
            .unwrap()
            .unwrap();
        assert_eq!(item_b_fetched.uid, uid);
        assert_eq!(item_b_fetched.hash, updated_hash);
        assert_eq!(item_b_fetched.href, item_b.href);
        assert_eq!(item_b_fetched.etag, updated_etag_b);

        let item_status = db
            .get_item_hash_by_uid(mapping_uid, uid)
            .unwrap()
            .expect("status should return items that were just inserted");
        assert_eq!(item_status.hash, updated_hash);

        let all = db.all_uids(mapping_uid).unwrap();
        let all_expected = vec![uid];
        assert_eq!(all, all_expected);
    }
    #[test]
    fn test_wrong_update() {
        let db = StatusDatabase::open_or_create(":memory:").unwrap();
        let mapping_uid = MappingUid(1);
        let uid = "07da74e5-0a32-482a-bbdd-13fd1e45cce3";
        let hash = "2222222222222222222222222222222222222222222222222222222222222222"
            .parse()
            .unwrap();
        let item_a = ItemRef {
            href: "/collections/work/item.ics".into(),
            etag: "123".into(),
        };
        let item_b = ItemRef {
            href: "work/item.ics".into(),
            etag: "abc000".into(),
        };
        db.get_or_add_collection("/collections/work/", "work")
            .unwrap();
        db.insert_item(mapping_uid, uid, &hash, &item_a, &item_b)
            .unwrap();

        let updated_hash = "3333333333333333333333333333333333333333333333333333333333333333"
            .parse()
            .unwrap();
        let updated_etag_a = "456".into();
        let updated_etag_b = "def111".into();
        let err = db
            .update_item(
                &updated_hash,
                &ItemRef {
                    href: "not/correct.ics".into(),
                    etag: item_a.etag,
                },
                &item_b,
                &ItemRef {
                    href: "not/correct.ics".into(),
                    etag: updated_etag_a,
                },
                &ItemRef {
                    href: item_b.href.clone(),
                    etag: updated_etag_b,
                },
            )
            .unwrap_err();
        assert!(matches!(err, StatusError::NoUpdate));
    }

    #[test]
    fn test_add_and_get_collection() {
        let db = StatusDatabase::open_or_create(":memory:").unwrap();
        let href_a = "/collections/guests";
        let href_b = "guests";
        let mapping_uid = db.get_or_add_collection(href_a, href_b).unwrap();

        let gotten_uid = db
            .get_mapping_uid(&href_a.to_string(), &href_b.to_string())
            .unwrap()
            .expect("should obtain mapping that was just inserted");
        assert_eq!(mapping_uid, gotten_uid);

        db.remove_collection(mapping_uid).unwrap();

        let gotten_uid = db
            .get_mapping_uid(&href_a.to_string(), &href_b.to_string())
            .unwrap();
        assert!(gotten_uid.is_none());
    }
}
