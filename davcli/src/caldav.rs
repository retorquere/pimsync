// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use hyper::client::HttpConnector;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use libdav::{auth::Auth, CalDavClient};
use log::info;

use crate::cli::Server;

type Client = CalDavClient<HttpsConnector<HttpConnector>>;

#[derive(Parser)]
pub struct CalDavArgs {
    #[command(flatten)]
    pub(crate) server: Server,

    #[command(subcommand)]
    command: CalDavCommand,
}

#[derive(Subcommand)]
pub(crate) enum CalDavCommand {
    /// Perform discovery and print results
    Discover,
    /// Find calendars under the calendar home set.
    FindCalendars,
    /// List calendar components under a given calendar collection.
    ListCalendarComponents {
        collection_href: String,
    },
    Tree,
    /// Fetches a single calendar component.
    Get {
        resource_href: String,
    },
    Delete {
        #[arg(long)]
        force: bool,
        href: String,
    },
}

impl Server {
    async fn caldav_client(&self) -> anyhow::Result<Client> {
        let password = std::env::var("DAVCLI_PASSWORD")
            .context("failed to determine password")?
            .into();
        let https = HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http1()
            .build();
        CalDavClient::builder()
            .with_uri(self.server_url.clone())
            .with_auth(Auth::Basic {
                username: self.username.clone(),
                password: Some(password),
            })
            .build(https)
            .await
            .map_err(anyhow::Error::from)
    }
}

impl CalDavArgs {
    #[tokio::main(flavor = "current_thread")]
    pub(crate) async fn execute(self) -> anyhow::Result<()> {
        let client = self.server.caldav_client().await?;

        match self.command {
            CalDavCommand::Discover => discover(client),
            CalDavCommand::FindCalendars => list_collections(client).await?,
            CalDavCommand::ListCalendarComponents { collection_href } => {
                list_resources(&client, collection_href).await?;
            }
            CalDavCommand::Tree => tree(client).await?,
            CalDavCommand::Get { resource_href } => get(client, resource_href).await?,
            CalDavCommand::Delete { force, href } => {
                if !force {
                    bail!("Must force deletion (no etag support in davcli)");
                }
                delete(&client, href).await?;
            }
        };

        Ok(())
    }
}

fn discover(client: Client) {
    println!("Discovery successful.");
    println!("- Context path: {}", &client.context_path());
    match client.calendar_home_set() {
        Some(home_set) => println!("- Calendar home set: {home_set}"),
        None => println!("- Calendar home set not found."),
    }
}

async fn get(client: Client, href: String) -> anyhow::Result<()> {
    let target_url = client
        .calendar_home_set()
        .context("No calendar home set available")?
        .to_string();

    let response = client
        .get_resources(target_url, &[href])
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
