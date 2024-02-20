// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use std::io::Read;

use anyhow::{bail, Context};
use hyper::client::HttpConnector;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use libdav::{auth::Auth, CalDavClient};
use log::info;

use crate::cli::ServerCommand;

type Client = CalDavClient<HttpsConnector<HttpConnector>>;

async fn caldav_client() -> anyhow::Result<Client> {
    let base_url = std::env::var("DAVCLI_BASE_URL")
        .context("failed to determine password")?
        .try_into()
        .context("parsing DAVCLI_BASE_URL")?;
    let username = std::env::var("DAVCLI_USERNAME").context("failed to determine password")?;
    let password = std::env::var("DAVCLI_PASSWORD")
        .context("failed to determine password")?
        .into();

    let https = HttpsConnectorBuilder::new()
        .with_native_roots()?
        .https_or_http()
        .enable_http1()
        .build();
    let builder = CalDavClient::builder()
        .with_uri(base_url)
        .with_auth(Auth::Basic {
            username,
            password: Some(password),
        })
        .bootstrap(https)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(builder.build())
}

#[tokio::main(flavor = "current_thread")]
pub(crate) async fn execute(command: ServerCommand) -> anyhow::Result<()> {
    let client = caldav_client().await?;

    match command {
        ServerCommand::Discover => discover(&client),
        ServerCommand::FindCollections => list_collections(client).await?,
        ServerCommand::ListItems { collection_href } => {
            list_resources(&client, collection_href).await?;
        }
        ServerCommand::Tree => tree(client).await?,
        ServerCommand::Get { resource_href } => get(client, resource_href).await?,
        ServerCommand::Create { resource_href } => create(client, resource_href).await?,
        ServerCommand::Delete { force, href } => {
            if !force {
                bail!("Must force deletion (no etag support in davcli)");
            }
            delete(&client, href).await?;
        }
    };

    Ok(())
}

fn discover(client: &Client) {
    println!("Discovery successful.");
    println!("- Context path: {}", client.base_url());
    match client.calendar_home_set() {
        Some(home_set) => println!("- Calendar home set: {home_set}"),
        None => println!("- Calendar home set not found."),
    }
}

async fn get(client: Client, href: String) -> anyhow::Result<()> {
    let collection = match href.rfind('/') {
        Some(i) => &href[0..i],
        None => "/",
    }
    .to_string();

    let response = client
        .get_calendar_resources(collection, &[href])
        .await?
        .into_iter()
        .next()
        .context("Server returned a response with no resources")?;

    let raw = &response
        .content
        .as_ref()
        .map_err(|code| anyhow::anyhow!("Server returned error code: {0}", code))?
        .data;

    println!("{raw}");

    Ok(())
}

async fn create(client: Client, href: String) -> anyhow::Result<()> {
    let mut data = Vec::new();
    let mut stdin = std::io::stdin().lock();
    stdin.read_to_end(&mut data).context("reading from stdin")?;

    let response = client
        .create_resource(&href, data, b"text/calendar")
        .await
        .context("sending request to create resource")?;

    if let Some(etag) = response {
        println!("Etag: {etag}");
    } else {
        println!("No etag");
    }

    Ok(())
}

async fn tree(client: Client) -> anyhow::Result<()> {
    let response = client.find_calendars(None).await?;
    for collection in response {
        println!("{}", collection.href);
        list_resources(&client, collection.href).await?;
    }

    Ok(())
}

async fn list_collections(client: Client) -> anyhow::Result<()> {
    let response = client.find_calendars(None).await?;
    for collection in response {
        println!("{}", collection.href);
    }

    Ok(())
}

async fn list_resources(client: &Client, href: String) -> anyhow::Result<()> {
    let resources = client.list_resources(&href).await?;
    if resources.is_empty() {
        info!("No items in collection");
    } else {
        for resource in resources {
            println!("{}", resource.href);
        }
    }

    Ok(())
}

async fn delete(client: &Client, href: String) -> anyhow::Result<()> {
    client
        .force_delete(&href)
        .await
        .map_err(anyhow::Error::from)
}
