// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

//! Types for granular error management.
//!
//! These types represent non-fatal errors which may occurs during synchronisation.

use super::{
    execute::ExecutionError,
    plan::{CollectionAction, ItemAction, PropertyAction},
};

/// An error synchronising two items between storages.
///
/// This error contains details on a non-fatal error that occurred during synchronisation. It
/// contains enough data to provide a meaningful description of what has gone wrong.
///
/// Use the [`std::fmt::Display`] implementation for a quick description.
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

    #[must_use]
    pub fn property(action: PropertyAction, error: ExecutionError) -> Self {
        Self {
            action: SomeAction::Property(action),
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

/// An action that has failed to execute. See [`SyncError`].
#[derive(Debug)]
pub enum SomeAction {
    Item(Box<ItemAction>),
    // TODO: this is missing the details of the collection itself (e.g.: alias?).
    Collection {
        action: CollectionAction,
        alias: String,
    },
    // FIXME: missing details of property. Do I need Property::display ?
    Property(PropertyAction),
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
            SomeAction::Property(action) => {
                write!(f, "property action '{action}'")
            }
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
        status::{ItemState, Side},
    };

    use super::SyncError;

    #[test]
    fn test_syncerror_item_display() {
        let err = SyncError {
            action: SomeAction::Item(Box::from(ItemAction::Create {
                side: Side::A,
                source: ItemState {
                    href: "/path/to/some/file.vcf".into(),
                    uid: "d99ed506-dceb-49f2-a1c9-efa63c68acd0".into(),
                    etag: "123890".into(),
                    hash: "AAAAAZZZZZ".into(),
                    data: None,
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
            "Error executing item action 'create in storage a (uid: d99ed506-dceb-49f2-a1c9-efa63c68acd0)': ",
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
