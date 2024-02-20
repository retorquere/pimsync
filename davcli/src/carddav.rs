// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::Context;
use hyper::client::HttpConnector;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use libdav::{auth::Auth, CardDavClient};
use log::info;

use crate::cli::ServerCommand;

type Client = CardDavClient<HttpsConnector<HttpConnector>>;

async fn carddav_client() -> anyhow::Result<Client> {
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
    let builder = CardDavClient::builder()
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
    let client = carddav_client().await?;

    match command {
        ServerCommand::Discover => discover(&client),
        ServerCommand::FindCollections => list_collections(client).await?,
        ServerCommand::ListItems { collection_href } => {
            list_resources(&client, collection_href).await?;
        }
        ServerCommand::Get { resource_href } => get(client, resource_href).await?,
        ServerCommand::Delete { .. } => todo!(),
        ServerCommand::Tree { .. } => todo!(),
        ServerCommand::Create { .. } => todo!(),
    };

    Ok(())
}

fn discover(client: &Client) {
    println!("Discovery successful.");
    println!("- Context path: {}", client.base_url());
    match client.addressbook_home_set() {
        Some(home_set) => println!("- Address book home set: {home_set}"),
        None => println!("- Address book home set not found."),
    }
}

async fn list_collections(client: Client) -> anyhow::Result<()> {
    let response = client.find_addressbooks(None).await?;
    for collection in response {
        println!("{}", collection.href);
    }

    Ok(())
}

async fn get(client: Client, href: String) -> anyhow::Result<()> {
    let collection = match href.rfind('/') {
        Some(i) => &href[0..i],
        None => "/",
    }
    .to_string();

    let response = client
        .get_address_book_resources(collection, &[href])
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
