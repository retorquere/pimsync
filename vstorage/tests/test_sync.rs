// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use camino::Utf8PathBuf;
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use std::fmt::Write;
use std::sync::Arc;
use vstorage::base::{IcsItem, Storage};
use vstorage::sync::declare::{DeclaredMapping, StoragePair};
use vstorage::sync::plan::Plan;
use vstorage::sync::status::StatusDatabase;
use vstorage::vdir::VdirStorage;

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

    Ok(entry.into())
}

async fn create_populated_storage(path: Utf8PathBuf) -> Arc<dyn Storage<IcsItem>> {
    std::fs::create_dir(&path).unwrap();
    let storage = VdirStorage::<IcsItem>::new(path, "ics".into());

    let first = storage.create_collection("first-calendar").await.unwrap();
    let item = &minimal_icalendar("First calendar event one")
        .unwrap()
        .into();
    storage.add_item(first.href(), item).await.unwrap();

    let item = &minimal_icalendar("First calendar event two")
        .unwrap()
        .into();
    storage.add_item(first.href(), item).await.unwrap();
    drop(first);

    let second = storage.create_collection("second-calendar").await.unwrap();
    let item = &minimal_icalendar("Second calendar event one")
        .unwrap()
        .into();
    storage.add_item(second.href(), item).await.unwrap();

    let item = &minimal_icalendar("Second calendar event two")
        .unwrap()
        .into();
    storage.add_item(second.href(), item).await.unwrap();
    drop(second);

    let third = storage.create_collection("third-calendar").await.unwrap();
    let item = &minimal_icalendar("Third calendar event one")
        .unwrap()
        .into();
    storage.add_item(third.href(), item).await.unwrap();
    drop(third);

    Arc::new(storage)
}

async fn create_empty_storage(path: Utf8PathBuf) -> Arc<dyn Storage<IcsItem>> {
    std::fs::create_dir(&path).unwrap();
    let storage = VdirStorage::<IcsItem>::new(path, "ics".into());
    Arc::new(storage)
}

#[tokio::test]
async fn test_sync_simple_case() {
    let populated_path = {
        let mut p = std::env::temp_dir();
        p.push(random_string(12));
        Utf8PathBuf::try_from(p).unwrap()
    };
    let empty_path = {
        let mut p = std::env::temp_dir();
        p.push(random_string(12));
        Utf8PathBuf::try_from(p).unwrap()
    };
    let populated = create_populated_storage(populated_path.clone()).await;
    let empty = create_empty_storage(empty_path.clone()).await;

    let first_mapping = DeclaredMapping::direct("first-calendar".parse().unwrap());
    let second_mapping = DeclaredMapping::direct("second-calendar".parse().unwrap());
    let mut pair = StoragePair::<IcsItem>::new(populated, empty)
        .with_mapping(first_mapping)
        .with_mapping(second_mapping);
    let plan = Plan::new(&mut pair, None).await.unwrap();
    // dbg!(&plan);
    // TODO: I'll need to trace! the point where each actions is decided.
    let status = StatusDatabase::open_or_create(":memory:").unwrap();
    plan.execute(&status, drop).await.unwrap();

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

    std::fs::remove_dir_all(populated_path).unwrap();
    std::fs::remove_dir_all(empty_path).unwrap();
}
