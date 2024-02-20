// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Types for granular error management.
//!
//! These types represent non-fatal errors which may occurs during synchronisation.

use super::{
    execute::ExecutionError,
    plan::{CollectionAction, ItemAction},
};

/// An error synchronising two items between storages.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct SyncError {
    action: SomeAction,
    error: ExecutionError,
}

impl SyncError {
    #[must_use]
    pub fn item(action: ItemAction, error: ExecutionError) -> Self {
        Self {
            action: SomeAction::Item(Box::from(action)),
            error,
        }
    }

    #[must_use]
    pub fn collection(action: CollectionAction, alias: String, error: ExecutionError) -> Self {
        Self {
            action: SomeAction::Collection { action, alias },
            error,
        }
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error executing {}: {}", self.action, self.error)
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[derive(Debug)]
pub enum SomeAction {
    Item(Box<ItemAction>),
    // TODO: this is missing the details of the collection itself (e.g.: alias?).
    Collection {
        action: CollectionAction,
        alias: String,
    },
}

impl std::fmt::Display for SomeAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SomeAction::Item(action) => {
                write!(f, "item action '{action}'")
            }
            SomeAction::Collection { action, alias } => {
                write!(f, "collection action '{action}' for '{alias}'")
            }
        }
    }
}

impl std::fmt::Display for ItemAction {
    /// This function is mostly implemented to be used for error reporting.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemAction::SaveToStatus { a, .. } => write!(f, "save to status (uid: {})", a.uid),
            ItemAction::ClearStatus { uid } => write!(f, "clear from status (uid: {uid})"),
            ItemAction::CreateInA { source } => write!(
                f,
                "create in storage a (uid: {}, from: {})",
                source.uid, source.href
            ),
            ItemAction::CreateInB { source } => write!(
                f,
                "create in storage b (uid: {}, from: {})",
                source.uid, source.href
            ),
            ItemAction::UpdateInA { source, target } => write!(
                f,
                "update in storage a (uid: {}, into: {})",
                source.uid, target.href
            ),
            ItemAction::UpdateInB { source, target } => write!(
                f,
                "update in storage b (uid: {}, into: {})",
                source.uid, target.href
            ),
            ItemAction::DeleteInA { target } => {
                write!(
                    f,
                    "delete in storage a (uid: {}, href: {})",
                    target.uid, target.href
                )
            }
            ItemAction::DeleteInB { target } => {
                write!(
                    f,
                    "delete in storage b (uid: {}, href: {})",
                    target.uid, target.href
                )
            }
            ItemAction::Conflict { uid } => {
                write!(f, "conflict (uid: {uid})")
            }
        }
    }
}

impl std::fmt::Display for CollectionAction {
    /// Only the action itself is displayed.
    ///
    /// This function is mostly implemented to be used for error reporting.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollectionAction::NoAction(_) => write!(f, "no action"),
            CollectionAction::SaveToStatus => write!(f, "save to status"),
            CollectionAction::CreateInA => write!(f, "create in storage a"),
            CollectionAction::CreateInB => write!(f, "create in storage b"),
            CollectionAction::CreateInBoth => write!(f, "create in both storages"),
            CollectionAction::Delete(_, side) => write!(f, "delete from {side}"),
        }
    }
}

#[cfg(test)]
mod test {
    use std::backtrace::Backtrace;

    use crate::sync::{
        error::SomeAction,
        execute::ExecutionError,
        plan::{CollectionAction, ItemAction},
        status::ItemState,
    };

    use super::SyncError;

    #[test]
    fn test_syncerror_item_display() {
        let err = SyncError {
            action: SomeAction::Item(Box::from(ItemAction::CreateInA {
                source: ItemState {
                    href: "/path/to/some/file.vcf".into(),
                    uid: "d99ed506-dceb-49f2-a1c9-efa63c68acd0".into(),
                    etag: "123890".into(),
                    hash: "AAAAAZZZZZ".into(),
                },
            })),
            error: ExecutionError::Storage(crate::Error {
                kind: crate::ErrorKind::AccessDenied,
                source: Some(Box::from(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Not enough mana",
                ))),
                backtrace: Backtrace::capture(),
            }),
        };
        let msg = err.to_string();
        let expected = concat!(
            "Error executing item action 'create in storage a (uid: d99ed506-dceb-49f2-a1c9-efa63c68acd0, from: /path/to/some/file.vcf)': ",
            "storage operation returned error: ",
            "access to the resource was denied: ",
            "Not enough mana"
        );
        assert_eq!(msg, expected);
    }

    #[test]
    fn test_syncerror_collection_display() {
        let err = SyncError {
            action: SomeAction::Collection {
                action: CollectionAction::CreateInB,
                alias: "guests".into(),
            },
            error: ExecutionError::Storage(crate::Error {
                kind: crate::ErrorKind::AccessDenied,
                source: Some(Box::from(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Creating new collections is forbidden",
                ))),
                backtrace: Backtrace::capture(),
            }),
        };
        let msg = err.to_string();
        let expected = concat!(
            "Error executing collection action 'create in storage b' for 'guests': ",
            "storage operation returned error: ",
            "access to the resource was denied: ",
            "Creating new collections is forbidden"
        );
        assert_eq!(msg, expected);
    }
}
