// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::Context;
use http::Uri;
use hyper::client::HttpConnector;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use libdav::{
    auth::Auth, carddav_service_for_url, dav::WebDavClient, sd::find_context_path_via_bootstrap,
    CardDavClient,
};
use log::info;

use crate::cli::ServerCommand;

type Client = CardDavClient<HttpsConnector<HttpConnector>>;

/// Create a new client. Note that bootstrap is not performed implicitly.
fn carddav_client() -> anyhow::Result<Client> {
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
    let auth = Auth::Basic {
        username,
        password: Some(password),
    };
    let webdav = WebDavClient::new(base_url, auth, https);
    let client = CardDavClient::new(webdav);

    Ok(client)
}

#[tokio::main(flavor = "current_thread")]
pub(crate) async fn execute(command: ServerCommand) -> anyhow::Result<()> {
    let client = carddav_client()?;

    match command {
        ServerCommand::Discover => discover(client).await?,
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

async fn discover(mut client: Client) -> anyhow::Result<()> {
    let service = carddav_service_for_url(client.base_url())?;
    println!("- Base url: {}", client.base_url());
    match find_context_path_via_bootstrap(&client, service).await? {
        Some(context_path) => {
            println!("- Resolved context path: {context_path}");
            client.dav_client.base_url = context_path;
        }
        None => {
            println!("- Context path not found; using given URL");
        }
    };
    match client.find_current_user_principal().await? {
        Some(principal) => {
            println!("- Current user principal: {principal}");
            match client.find_address_book_home_set(&principal).await? {
                Some(home_set) => println!("- Address book home set: {home_set}"),
                None => println!("- Address book home set not found."),
            }
        }
        None => println!("- Curent user principal not found."),
    };
    Ok(())
}

async fn url_for_finding_address_books(client: &Client) -> anyhow::Result<Uri> {
    let url = match client.find_current_user_principal().await? {
        Some(principal) => {
            let home_set = client.find_address_book_home_set(&principal).await?;
            home_set.unwrap_or(client.base_url().clone())
        }
        None => client.base_url().clone(),
    };
    Ok(url)
}

async fn list_collections(client: Client) -> anyhow::Result<()> {
    let url = url_for_finding_address_books(&client).await?;
    let response = client.find_addressbooks(&url).await?;
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
