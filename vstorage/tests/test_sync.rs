// Copyright 2023-2025 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use camino::Utf8PathBuf;
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use std::fmt::Write;
use std::fs::create_dir;
use std::sync::Arc;
use vstorage::base::{CreateItemOptions, Storage};
use vstorage::sync::declare::{OnDelete, OnEmpty, StoragePair, SyncedCollection};
use vstorage::sync::execute::Executor;
use vstorage::sync::plan::{CollectionAction, ItemAction, Plan};
use vstorage::sync::status::{Side, StatusDatabase};
use vstorage::vdir::VdirStorage;
use vstorage::ItemKind;

fn temporary_path() -> Utf8PathBuf {
    let mut p = std::env::temp_dir();
    p.push(random_string(12));
    Utf8PathBuf::try_from(p).unwrap()
}

fn random_string(len: usize) -> String {
    thread_rng()
        .sample_iter(Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn minimal_icalendar(summary: &str) -> anyhow::Result<String> {
    let mut entry = String::new();
    let uid = random_string(12);

    entry.push_str("BEGIN:VCALENDAR\r\n");
    entry.push_str("VERSION:2.0\r\n");
    entry.push_str("PRODID:-//hacksw/handcal//NONSGML v1.0//EN\r\n");
    entry.push_str("BEGIN:VEVENT\r\n");
    write!(entry, "UID:{uid}\r\n")?;
    entry.push_str("DTSTAMP:19970610T172345Z\r\n");
    entry.push_str("DTSTART:19970714T170000Z\r\n");
    write!(entry, "SUMMARY:{summary}\r\n")?;
    entry.push_str("END:VEVENT\r\n");
    entry.push_str("END:VCALENDAR\r\n");

    Ok(entry)
}

/// Create a storage with three calendars each one with a single event.
async fn create_populated_storage(path: Utf8PathBuf) -> Arc<dyn Storage> {
    create_dir(&path).unwrap();
    let storage = VdirStorage::new(path, "ics".into(), ItemKind::Calendar);

    let first = storage.create_collection("first-calendar").await.unwrap();
    let item = &minimal_icalendar("First calendar event one")
        .unwrap()
        .into();
    let opts = CreateItemOptions::default();
    storage
        .create_item(first.href(), item, opts.clone())
        .await
        .unwrap();

    let item = &minimal_icalendar("First calendar event two")
        .unwrap()
        .into();
    storage
        .create_item(first.href(), item, opts.clone())
        .await
        .unwrap();
    drop(first);

    let second = storage.create_collection("second-calendar").await.unwrap();
    let item = &minimal_icalendar("Second calendar event one")
        .unwrap()
        .into();
    storage
        .create_item(second.href(), item, opts.clone())
        .await
        .unwrap();

    let item = &minimal_icalendar("Second calendar event two")
        .unwrap()
        .into();
    storage
        .create_item(second.href(), item, opts.clone())
        .await
        .unwrap();
    drop(second);

    let third = storage.create_collection("third-calendar").await.unwrap();
    let item = &minimal_icalendar("Third calendar event one")
        .unwrap()
        .into();
    storage.create_item(third.href(), item, opts).await.unwrap();
    drop(third);

    Arc::new(storage)
}

async fn create_empty_storage(path: Utf8PathBuf) -> Arc<dyn Storage> {
    create_dir(&path).unwrap();
    let storage = VdirStorage::new(path, "ics".into(), ItemKind::Calendar);
    Arc::new(storage)
}

// TODO: maybe "second calendar" should exist in side B for these to make more sense.

#[tokio::test]
async fn test_sync_only_declared_mappings() {
    let populated_path = temporary_path();
    let empty_path = temporary_path();
    let populated = create_populated_storage(populated_path.clone()).await;
    let empty = create_empty_storage(empty_path.clone()).await;

    let first_mapping = SyncedCollection::direct("first-calendar".parse().unwrap());
    let second_mapping = SyncedCollection::direct("second-calendar".parse().unwrap());
    // third-calendar is not synced.

    let pair = StoragePair::new(populated, empty)
        .with_mapping(first_mapping)
        .with_mapping(second_mapping);
    let plan = Plan::new(&pair, None).await.unwrap();
    // TODO: inspect plan
    let status = StatusDatabase::open_or_create(":memory:").unwrap();
    Executor::new(drop).plan(plan, &status).await.unwrap();

    let first = std::fs::read_dir(empty_path.join("first-calendar"))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(first.len(), 2);
    for item in first {
        let data = std::fs::read_to_string(item.path()).unwrap();
        let found = data.find("First calendar event");
        assert!(found.is_some());
    }

    let second = std::fs::read_dir(empty_path.join("second-calendar"))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(second.len(), 2);
    for item in second {
        let data = std::fs::read_to_string(item.path()).unwrap();
        let found = data.find("Second calendar event");
        assert!(found.is_some());
    }

    let _third = std::fs::read_dir(empty_path.join("third-calendar")).unwrap_err();

    std::fs::remove_dir_all(populated_path).unwrap();
    std::fs::remove_dir_all(empty_path).unwrap();
}

#[tokio::test]
async fn test_sync_from_a() {
    let populated_path = temporary_path();
    let empty_path = temporary_path();
    let populated = create_populated_storage(populated_path.clone()).await;
    let empty = create_empty_storage(empty_path.clone()).await;

    let pair = StoragePair::new(populated, empty).with_all_from_a();
    let plan = Plan::new(&pair, None).await.unwrap();
    // TODO: inspect plan
    let status = StatusDatabase::open_or_create(":memory:").unwrap();
    Executor::new(drop).plan(plan, &status).await.unwrap();

    let first = std::fs::read_dir(empty_path.join("first-calendar"))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>();

    // Note that "third-calendar" is not present in the "empty" one.
    assert_eq!(first.len(), 2);

    for item in first {
        let data = std::fs::read_to_string(item.path()).unwrap();
        let found = data.find("First calendar event");
        assert!(found.is_some());
    }

    let second = std::fs::read_dir(empty_path.join("second-calendar"))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(second.len(), 2);
    for item in second {
        let data = std::fs::read_to_string(item.path()).unwrap();
        let found = data.find("Second calendar event");
        assert!(found.is_some());
    }

    let third = std::fs::read_dir(empty_path.join("third-calendar"))
        .unwrap()
        .map(|r| r.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(third.len(), 1);
    for item in third {
        let data = std::fs::read_to_string(item.path()).unwrap();
        let found = data.find("Third calendar event");
        assert!(found.is_some());
    }

    std::fs::remove_dir_all(populated_path).unwrap();
    std::fs::remove_dir_all(empty_path).unwrap();
}

#[tokio::test]
async fn test_sync_from_b() {
    let populated_path = temporary_path();
    let empty_path = temporary_path();
    let populated = create_populated_storage(populated_path.clone()).await;
    let empty = create_empty_storage(empty_path.clone()).await;

    let pair = StoragePair::new(populated, empty).with_all_from_b();
    let plan = Plan::new(&pair, None).await.unwrap();
    // TODO: inspect plan
    let status = StatusDatabase::open_or_create(":memory:").unwrap();
    Executor::new(drop).plan(plan, &status).await.unwrap();

    let _first = std::fs::read_dir(empty_path.join("first-calendar")).unwrap_err();
    let _second = std::fs::read_dir(empty_path.join("second-calendar")).unwrap_err();
    let _third = std::fs::read_dir(empty_path.join("third-calendar")).unwrap_err();

    std::fs::remove_dir_all(populated_path).unwrap();
    std::fs::remove_dir_all(empty_path).unwrap();
}

#[tokio::test]
async fn test_sync_none() {
    let populated_path = temporary_path();
    let empty_path = temporary_path();
    let populated = create_populated_storage(populated_path.clone()).await;
    let empty = create_empty_storage(empty_path.clone()).await;

    let pair = StoragePair::new(populated, empty);
    let plan = Plan::new(&pair, None).await.unwrap();
    // TODO: inspect plan
    let status = StatusDatabase::open_or_create(":memory:").unwrap();
    Executor::new(drop).plan(plan, &status).await.unwrap();

    let _first = std::fs::read_dir(empty_path.join("first-calendar")).unwrap_err();
    let _second = std::fs::read_dir(empty_path.join("second-calendar")).unwrap_err();
    let _third = std::fs::read_dir(empty_path.join("third-calendar")).unwrap_err();

    std::fs::remove_dir_all(populated_path).unwrap();
    std::fs::remove_dir_all(empty_path).unwrap();
}
// TODO: create in both sides, sync once, then:
// - delete in a, check deletion after syn
// - delete in b, check deletion after syn
// - update in a, check deletion after syn
// - update in b, check deletion after syn
// - create new in a...

#[tokio::test]
async fn test_empty_on_empty_skip() {
    let path_a = temporary_path();
    let path_b = temporary_path();
    create_dir(&path_a).unwrap();
    create_dir(&path_b).unwrap();
    let storage_a = Arc::new(VdirStorage::new(path_a, "ics".into(), ItemKind::Calendar));
    let storage_b = Arc::new(VdirStorage::new(path_b, "ics".into(), ItemKind::Calendar));

    let first = storage_a.create_collection("first-calendar").await.unwrap();
    let item = &minimal_icalendar("First calendar event one")
        .unwrap()
        .into();
    let opts = CreateItemOptions::default();
    let item_ver = storage_a
        .create_item(first.href(), item, opts)
        .await
        .unwrap();

    let pair = StoragePair::new(storage_a.clone(), storage_b.clone())
        .with_all_from_a()
        .on_empty(OnEmpty::Skip);
    let status = StatusDatabase::open_or_create(":memory:").unwrap();
    let plan = Plan::new(&pair, Some(&status)).await.unwrap();
    Executor::new(drop).plan(plan, &status).await.unwrap();

    // At this point both storages and the status DB are all in sync.

    storage_a
        .delete_item(&item_ver.href, &item_ver.etag)
        .await
        .unwrap();

    let plan = Plan::new(&pair, Some(&status)).await.unwrap();
    assert!(plan.collection_plans.is_empty());
}

#[tokio::test]
async fn test_empty_on_empty_sync() {
    let path_a = temporary_path();
    let path_b = temporary_path();
    create_dir(&path_a).unwrap();
    create_dir(&path_b).unwrap();
    let storage_a = Arc::new(VdirStorage::new(path_a, "ics".into(), ItemKind::Calendar));
    let storage_b = Arc::new(VdirStorage::new(path_b, "ics".into(), ItemKind::Calendar));

    let first = storage_a.create_collection("first-calendar").await.unwrap();
    let item = &minimal_icalendar("First calendar event one")
        .unwrap()
        .into();
    let opts = CreateItemOptions::default();
    let item_ver = storage_a
        .create_item(first.href(), item, opts)
        .await
        .unwrap();

    let pair = StoragePair::new(storage_a.clone(), storage_b.clone())
        .with_all_from_a()
        .on_empty(OnEmpty::Sync);
    let status = StatusDatabase::open_or_create(":memory:").unwrap();
    let plan = Plan::new(&pair, Some(&status)).await.unwrap();
    Executor::new(drop).plan(plan, &status).await.unwrap();

    // At this point both storages and the status DB are all in sync.

    storage_a
        .delete_item(&item_ver.href, &item_ver.etag)
        .await
        .unwrap();

    let plan = Plan::new(&pair, Some(&status)).await.unwrap();
    assert_eq!(plan.collection_plans.len(), 1);
    // Plan should be empty due to protection:
    let action = plan
        .collection_plans
        .first()
        .unwrap()
        .items
        .first()
        .unwrap();
    assert!(matches!(action, ItemAction::Delete { side: Side::B, .. }));
}

#[tokio::test]
async fn test_empty_on_delete_skip() {
    let path_a = temporary_path();
    let path_b = temporary_path();
    create_dir(&path_a).unwrap();
    create_dir(&path_b).unwrap();
    let storage_a = Arc::new(VdirStorage::new(path_a, "ics".into(), ItemKind::Calendar));
    let storage_b = Arc::new(VdirStorage::new(path_b, "ics".into(), ItemKind::Calendar));

    let first = storage_a.create_collection("first-calendar").await.unwrap();

    let pair = StoragePair::new(storage_a.clone(), storage_b.clone())
        .with_all_from_a()
        .with_all_from_b()
        .on_delete(OnDelete::Skip);
    let status = StatusDatabase::open_or_create(":memory:").unwrap();
    let plan = Plan::new(&pair, Some(&status)).await.unwrap();
    Executor::new(drop).plan(plan, &status).await.unwrap();

    // At this point both storages and the status DB are all in sync.

    storage_a.delete_collection(first.href()).await.unwrap();

    let plan = Plan::new(&pair, Some(&status)).await.unwrap();
    assert!(plan.collection_plans.is_empty());
}

#[tokio::test]
async fn test_empty_on_delete_sync() {
    let path_a = temporary_path();
    let path_b = temporary_path();
    create_dir(&path_a).unwrap();
    create_dir(&path_b).unwrap();
    let storage_a = Arc::new(VdirStorage::new(path_a, "ics".into(), ItemKind::Calendar));
    let storage_b = Arc::new(VdirStorage::new(path_b, "ics".into(), ItemKind::Calendar));

    let first = storage_a.create_collection("first-calendar").await.unwrap();

    let pair = StoragePair::new(storage_a.clone(), storage_b.clone())
        .with_all_from_a()
        .with_all_from_b()
        .on_delete(OnDelete::Sync);
    let status = StatusDatabase::open_or_create(":memory:").unwrap();
    let plan = Plan::new(&pair, Some(&status)).await.unwrap();
    Executor::new(drop).plan(plan, &status).await.unwrap();

    // At this point both storages and the status DB are all in sync.

    storage_a.delete_collection(first.href()).await.unwrap();

    let plan = Plan::new(&pair, Some(&status)).await.unwrap();
    assert_eq!(plan.collection_plans.len(), 1);
    // Plan should be empty due to protection:
    let action = &plan.collection_plans.first().unwrap().action;
    assert!(matches!(action, CollectionAction::Delete(_, Side::B)));
}
