// Copyright 2023-2024 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::{bail, ensure, Context};
use http::StatusCode;
use libdav::{
    dav::{mime_types, WebDavError},
    names,
};
use std::fmt::Write;

use crate::{random_string, TestData};

pub(crate) async fn test_create_and_delete_addressbook(test_data: &TestData) -> anyhow::Result<()> {
    let orig_addressbook_count = test_data.addressbook_count().await?;

    let new_collection = format!(
        "{}{}/",
        test_data.first_address_book_home_set()?.path(),
        &random_string(16)
    );
    test_data
        .carddav
        .create_addressbook(&new_collection)
        .await?;

    ensure!(orig_addressbook_count + 1 == test_data.addressbook_count().await?);

    // Get the etag of the newly created addressbook:
    // ASSERTION: this validates that a collection with a matching href was created.
    let addressbook = test_data
        .carddav
        .find_addressbooks(test_data.first_address_book_home_set()?)
        .await?;
    let etag = addressbook
        .into_iter()
        .find(|collection| collection.href == new_collection)
        .context("created addressbook was not returned when finding addressbooks")?
        .etag;

    // Try deleting with the wrong etag.
    test_data
        .carddav
        .delete(&new_collection, "wrong-etag")
        .await
        .unwrap_err();

    let Some(etag) = etag else {
        bail!("deletion is only supported on servers which provide etags")
    };

    // Delete the addressbook
    test_data.carddav.delete(new_collection, etag).await?;

    ensure!(orig_addressbook_count == test_data.addressbook_count().await?);

    Ok(())
}

fn minimal_vcard() -> anyhow::Result<Vec<u8>> {
    let mut entry = String::new();
    let uid = random_string(12);

    entry.push_str("BEGIN:VCARD\r\n");
    entry.push_str("VERSION:3.0\r\n");
    entry.push_str("PRODID:-//Apple Inc.//iOS 13.5.1//EN\r\n");
    entry.push_str("N:Anderson;Thomas;;;\r\n");
    entry.push_str("FN:Thomas \\\"Neo\\\" Anderson\r\n");
    entry.push_str("TEL;type=CELL;type=VOICE;type=pref:+54 9 11 1234-1234\r\n");
    entry.push_str("REV:2020-06-19T16:43:43Z\r\n");
    write!(entry, "UID:{uid}\r\n")?;
    entry.push_str("X-IMAGETYPE:PHOTO\r\n");
    entry.push_str("END:VCARD\r\n");

    Ok(entry.into())
}

pub(crate) async fn test_create_and_delete_resource(test_data: &TestData) -> anyhow::Result<()> {
    let collection = format!(
        "{}{}/",
        test_data.first_address_book_home_set()?.path(),
        &random_string(16)
    );
    test_data.carddav.create_addressbook(&collection).await?;

    let resource = format!("{}{}.vcf", collection, &random_string(12));
    let content = minimal_vcard()?;

    test_data
        .carddav
        .create_resource(&resource, content.clone(), mime_types::ADDRESSBOOK)
        .await?;

    let items = test_data.carddav.list_resources(&collection).await?;
    ensure!(items.len() == 1);

    let updated_entry = String::from_utf8(content)?
        .replace("Thomas \\\"Neo\\\" Anderson", "Neo")
        .as_bytes()
        .to_vec();

    // ASSERTION: deleting with a wrong etag fails.
    test_data
        .carddav
        .delete(&resource, "wrong-lol")
        .await
        .unwrap_err();

    // ASSERTION: creating conflicting resource fails.
    test_data
        .carddav
        .create_resource(&resource, updated_entry.clone(), mime_types::ADDRESSBOOK)
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
        .context("no item matched requested href")?
        .context("matching item was missing etag")?;

    // ASSERTION: updating with wrong etag fails
    match test_data
        .carddav
        .update_resource(
            &resource,
            updated_entry.clone(),
            &resource,
            mime_types::ADDRESSBOOK,
        )
        .await
        .unwrap_err()
    {
        WebDavError::BadStatusCode(StatusCode::PRECONDITION_FAILED) => {}
        _ => panic!("updating entry with the wrong etag did not return the wrong error type"),
    }

    // ASSERTION: updating with correct etag work
    test_data
        .carddav
        .update_resource(&resource, updated_entry, &etag, mime_types::ADDRESSBOOK)
        .await?;

    // ASSERTION: deleting with outdated etag fails
    test_data
        .carddav
        .delete(&resource, &etag)
        .await
        .unwrap_err();

    let items = test_data.carddav.list_resources(&collection).await?;
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
        .context("no item matched requested href")?
        .context("matching item was missing etag")?;

    // ASSERTION: deleting with correct etag works
    test_data.carddav.delete(&resource, &etag).await?;

    ensure!(test_data.carddav.list_resources(&collection).await?.len() == 0);
    Ok(())
}

pub(crate) async fn test_setting_and_getting_addressbook_displayname(
    test_data: &TestData,
) -> anyhow::Result<()> {
    let new_collection = format!(
        "{}{}/",
        test_data.first_address_book_home_set()?.path(),
        &random_string(16)
    );
    test_data
        .carddav
        .create_addressbook(&new_collection)
        .await?;

    let first_name = "panda-events";
    test_data
        .carddav
        .set_property(&new_collection, &names::DISPLAY_NAME, Some(first_name))
        .await
        .context("setting collection displayname")?;

    let value = test_data
        .carddav
        .get_property(&new_collection, &names::DISPLAY_NAME)
        .await
        .context("getting collection displayname")?;

    ensure!(value == Some(String::from(first_name)));

    let new_name = "🔥🔥🔥<lol>";
    test_data
        .carddav
        .set_property(&new_collection, &names::DISPLAY_NAME, Some(new_name))
        .await
        .context("setting collection displayname")?;

    let value = test_data
        .carddav
        .get_property(&new_collection, &names::DISPLAY_NAME)
        .await
        .context("getting collection displayname")?;

    ensure!(value == Some(String::from(new_name)));

    test_data.carddav.force_delete(&new_collection).await?;

    Ok(())
}

pub(crate) async fn test_check_carddav_support(test_data: &TestData) -> anyhow::Result<()> {
    test_data
        .carddav
        .check_support(test_data.carddav.base_url())
        .await?;

    Ok(())
}
