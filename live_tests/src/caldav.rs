// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::{bail, ensure, Context};
use http::StatusCode;
use libdav::dav::{mime_types, WebDavError};
use std::fmt::Write;

use crate::{random_string, TestData};

pub(crate) async fn test_create_and_delete_collection(test_data: &TestData) -> anyhow::Result<()> {
    let orig_calendar_count = test_data.calendar_count().await?;

    let new_collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data.caldav.create_calendar(&new_collection).await?;

    let new_calendar_count = test_data.calendar_count().await?;

    ensure!(orig_calendar_count + 1 == new_calendar_count);

    // Get the etag of the newly created calendar:
    // ASSERTION: this validates that a collection with a matching href was created.
    let calendars = test_data
        .caldav
        .find_calendars(&test_data.calendar_home_set)
        .await?;
    let etag = calendars
        .into_iter()
        .find(|collection| collection.href == new_collection)
        .context("created calendar was not returned when finding calendars")?
        .etag;

    // Try deleting with the wrong etag.
    test_data
        .caldav
        .delete(&new_collection, "wrong-etag")
        .await
        .unwrap_err();

    let Some(etag) = etag else {
        bail!("deletion is only supported on servers which provide etags")
    };

    // Delete the calendar
    test_data.caldav.delete(new_collection, etag).await?;

    let third_calendar_count = test_data.calendar_count().await?;
    ensure!(orig_calendar_count == third_calendar_count);

    Ok(())
}

pub(crate) async fn test_create_and_force_delete_collection(
    test_data: &TestData,
) -> anyhow::Result<()> {
    let orig_calendar_count = test_data.calendar_count().await?;

    let new_collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data.caldav.create_calendar(&new_collection).await?;

    let after_creationg_calendar_count = test_data.calendar_count().await?;
    ensure!(orig_calendar_count + 1 == after_creationg_calendar_count);

    // Force-delete the collection
    test_data.caldav.force_delete(&new_collection).await?;

    let after_deletion_calendar_count = test_data.calendar_count().await?;
    ensure!(orig_calendar_count == after_deletion_calendar_count);

    Ok(())
}

pub(crate) async fn test_setting_and_getting_displayname(
    test_data: &TestData,
) -> anyhow::Result<()> {
    let new_collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data.caldav.create_calendar(&new_collection).await?;

    let first_name = "panda-events";
    test_data
        .caldav
        .set_collection_displayname(&new_collection, Some(first_name))
        .await
        .context("setting collection displayname")?;

    let value = test_data
        .caldav
        .get_collection_displayname(&new_collection)
        .await
        .context("getting collection displayname")?;

    ensure!(value == Some(String::from(first_name)));

    let new_name = "🔥🔥🔥<lol>";
    test_data
        .caldav
        .set_collection_displayname(&new_collection, Some(new_name))
        .await
        .context("setting collection displayname")?;

    let value = test_data
        .caldav
        .get_collection_displayname(&new_collection)
        .await
        .context("getting collection displayname")?;

    ensure!(value == Some(String::from(new_name)));

    test_data.caldav.force_delete(&new_collection).await?;

    Ok(())
}

pub(crate) async fn test_setting_and_getting_colour(test_data: &TestData) -> anyhow::Result<()> {
    let new_collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data.caldav.create_calendar(&new_collection).await?;

    let colour = "#ff00ff";
    test_data
        .caldav
        .set_calendar_colour(&new_collection, Some(colour))
        .await
        .context("setting collection colour")?;

    let value = test_data
        .caldav
        .get_calendar_colour(&new_collection)
        .await
        .context("getting collection colour")?;

    match value {
        Some(c) => ensure!(c.eq_ignore_ascii_case(colour) || c.eq_ignore_ascii_case("#FF00FFFF")),
        None => bail!("Set a colour but then got colour None"),
    }

    test_data.caldav.force_delete(&new_collection).await?;

    Ok(())
}

fn minimal_icalendar() -> anyhow::Result<Vec<u8>> {
    let mut entry = String::new();
    let uid = random_string(12);

    entry.push_str("BEGIN:VCALENDAR\r\n");
    entry.push_str("VERSION:2.0\r\n");
    entry.push_str("PRODID:-//hacksw/handcal//NONSGML v1.0//EN\r\n");
    entry.push_str("BEGIN:VEVENT\r\n");
    write!(entry, "UID:{uid}\r\n")?;
    entry.push_str("DTSTAMP:19970610T172345Z\r\n");
    entry.push_str("DTSTART:19970714T170000Z\r\n");
    entry.push_str("SUMMARY:hello\\, testing\r\n");
    entry.push_str("END:VEVENT\r\n");
    entry.push_str("END:VCALENDAR\r\n");

    Ok(entry.into())
}

fn funky_calendar_event() -> anyhow::Result<Vec<u8>> {
    let mut entry = String::new();
    let uid = random_string(12);

    entry.push_str("BEGIN:VCALENDAR\r\n");
    entry.push_str("VERSION:2.0\r\n");
    entry.push_str("PRODID:-//hacksw/handcal//NONSGML v1.0//EN\r\n");
    entry.push_str("BEGIN:VEVENT\r\n");
    write!(entry, "UID:{uid}\r\n")?;
    entry.push_str("DTSTAMP:19970610T172345Z\r\n");
    entry.push_str("DTSTART:19970714T170000Z\r\n");
    entry.push_str("SUMMARY:eine Testparty mit Bären\r\n");
    entry.push_str("END:VEVENT\r\n");
    entry.push_str("END:VCALENDAR\r\n");

    Ok(entry.into())
}

pub(crate) async fn test_create_and_delete_resource(test_data: &TestData) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data.caldav.create_calendar(&collection).await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    let content = minimal_icalendar()?;

    test_data
        .caldav
        .create_resource(&resource, content.clone(), mime_types::CALENDAR)
        .await?;

    let items = test_data.caldav.list_resources(&collection).await?;
    ensure!(items.len() == 1);

    let updated_entry = String::from_utf8(content)?
        .replace("hello", "goodbye")
        .as_bytes()
        .to_vec();

    // ASSERTION: deleting with a wrong etag fails.
    test_data
        .caldav
        .delete(&resource, "wrong-lol")
        .await
        .unwrap_err();

    // ASSERTION: creating conflicting resource fails.
    test_data
        .caldav
        .create_resource(&resource, updated_entry.clone(), mime_types::CALENDAR)
        .await
        .unwrap_err();

    // ASSERTION: item with matching href exists.
    let etag = items
        .into_iter()
        .find_map(|i| {
            if i.href == resource {
                Some(i.details.etag)
            } else {
                None
            }
        })
        .context("todo")?
        .context("todo")?;

    // ASSERTION: updating with wrong etag fails
    match test_data
        .caldav
        .update_resource(
            &resource,
            updated_entry.clone(),
            &resource,
            mime_types::CALENDAR,
        )
        .await
        .unwrap_err()
    {
        WebDavError::BadStatusCode(StatusCode::PRECONDITION_FAILED) => {}
        _ => panic!("updating entry with the wrong etag did not return the wrong error type"),
    }

    // ASSERTION: updating with correct etag work
    test_data
        .caldav
        .update_resource(&resource, updated_entry, &etag, mime_types::CALENDAR)
        .await?;

    // ASSERTION: deleting with outdated etag fails
    test_data.caldav.delete(&resource, &etag).await.unwrap_err();

    let items = test_data.caldav.list_resources(&collection).await?;
    ensure!(items.len() == 1);

    let etag = items
        .into_iter()
        .find_map(|i| {
            if i.href == resource {
                Some(i.details.etag)
            } else {
                None
            }
        })
        .context("todo")?
        .context("todo")?;

    // ASSERTION: deleting with correct etag works
    test_data.caldav.delete(&resource, &etag).await?;

    let items = test_data.caldav.list_resources(&collection).await?;
    ensure!(items.len() == 0);
    Ok(())
}

pub(crate) async fn test_create_and_fetch_resource(test_data: &TestData) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data.caldav.create_calendar(&collection).await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    let event_data = minimal_icalendar()?;
    test_data
        .caldav
        .create_resource(&resource, event_data.clone(), mime_types::CALENDAR)
        .await?;

    let items = test_data.caldav.list_resources(&collection).await?;
    ensure!(items.len() == 1);

    let fetched = test_data
        .caldav
        .get_calendar_resources(&collection, &[&items[0].href])
        .await?;
    ensure!(fetched.len() == 1);
    assert_eq!(fetched[0].href, resource);

    let fetched_data = &fetched[0].content.as_ref().unwrap().data;
    // TODO: compare normalised items here!
    ensure!(fetched_data.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
    ensure!(fetched_data.contains("SUMMARY:hello\\, testing\r\n"));
    Ok(())
}

pub(crate) async fn test_create_and_fetch_resource_with_non_ascii_data(
    test_data: &TestData,
) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data.caldav.create_calendar(&collection).await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    let event_data = funky_calendar_event()?;
    test_data
        .caldav
        .create_resource(&resource, event_data.clone(), mime_types::CALENDAR)
        .await?;

    let items = test_data.caldav.list_resources(&collection).await?;
    ensure!(items.len() == 1);

    let mut fetched = test_data
        .caldav
        .get_calendar_resources(&collection, &[&items[0].href])
        .await?;
    ensure!(fetched.len() == 1);
    assert_eq!(fetched[0].href, resource);

    let fetched_data = fetched.pop().unwrap().content.unwrap().data;

    ensure!(fetched_data.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"));
    ensure!(fetched_data.contains("SUMMARY:eine Testparty mit Bären"));

    // TODO: compare normalised items here!
    // Need to do a semantic comparison of the send data vs fetched data. E.g.: to items should be
    // considered the same if only the order of its properties has changed.

    // This only compares length until the above is implemented.
    // Some servers move around the UID:, but the total length ends up being the same.
    assert_eq!(
        fetched_data.len(),
        String::from_utf8(event_data).unwrap().len()
    );
    Ok(())
}

pub(crate) async fn test_create_and_fetch_resource_with_weird_characters(
    test_data: &TestData,
) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data.caldav.create_calendar(&collection).await?;

    let mut count = 0;
    for symbol in ":?# []@!$&'()*+,;=<>".chars() {
        let resource = format!("{}weird-{}-{}.ics", collection, symbol, &random_string(6));
        test_data
            .caldav
            .create_resource(&resource, minimal_icalendar()?, mime_types::CALENDAR)
            .await
            .context(format!("failed to create resource with '{symbol}'"))?;
        count += 1;

        let items = test_data
            .caldav
            .list_resources(&collection)
            .await
            .context(format!("failed listing resource (when testing '{symbol}')"))?;
        ensure!(items.len() == count);
        ensure!(
            items.iter().any(|i| i.href == resource),
            format!("created item must be present when listing (testing '{symbol}')")
        );

        let fetched = test_data
            .caldav
            .get_calendar_resources(&collection, &[&resource])
            .await
            .context(format!("failed to get resource with '{symbol}'"))?;
        ensure!(fetched.len() == 1);
        assert_eq!(fetched[0].href, resource);
    }

    Ok(())
}

pub(crate) async fn test_fetch_missing(test_data: &TestData) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.calendar_home_set.path(),
        &random_string(16)
    );
    test_data.caldav.create_calendar(&collection).await?;

    let resource = format!("{}{}.ics", collection, &random_string(12));
    test_data
        .caldav
        .create_resource(&resource, minimal_icalendar()?, mime_types::CALENDAR)
        .await?;

    let missing = format!("{}{}.ics", collection, &random_string(8));
    let fetched = test_data
        .caldav
        .get_calendar_resources(&collection, &[&resource, &missing])
        .await?;
    log::debug!("{:?}", &fetched);
    // Nextcloud omits missing entries, rather than return 404, so we might have just one result.
    match fetched.len() {
        1 => {}
        2 => {
            // ASSERTION: one of the two entries is the 404 one
            fetched
                .iter()
                .find(|r| r.content == Err(StatusCode::NOT_FOUND))
                .context("no entry was missing, but one was expected")?;
        }
        _ => bail!("bogus amount of resources found"),
    }
    // ASSERTION: one entry is the matching resource
    fetched
        .iter()
        .find(|r| r.content.is_ok())
        .context("no entry was found, but one was expected")?;
    Ok(())
}

pub(crate) async fn test_check_caldav_support(test_data: &TestData) -> anyhow::Result<()> {
    test_data
        .caldav
        .check_support(test_data.caldav.base_url())
        .await?;

    Ok(())
}
