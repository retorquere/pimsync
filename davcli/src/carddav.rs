// Copyright 2023 Hugo Osvaldo Barrera
//
// SPDX-License-Identifier: EUPL-1.2

use anyhow::Context;
use clap::{Parser, Subcommand};
use hyper::client::HttpConnector;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use libdav::{auth::Auth, CardDavClient};

use crate::cli::Server;

type Client = CardDavClient<HttpsConnector<HttpConnector>>;

#[derive(Parser)]
pub struct CardDavArgs {
    #[command(flatten)]
    pub(crate) server: Server,

    #[command(subcommand)]
    command: CardDavCommand,
}

#[derive(Subcommand)]
pub(crate) enum CardDavCommand {
    /// Perform discovery and print results
    Discover,
    /// Find address books under the address book home set.
    FindAddressBooks,
}

impl Server {
    async fn carddav_client(&self) -> anyhow::Result<Client> {
        let password = std::env::var("DAVCLI_PASSWORD")
            .context("failed to determine password")?
            .into();
        let https = HttpsConnectorBuilder::new()
            .with_native_roots()
            .https_or_http()
            .enable_http1()
            .build();
        let builder = CardDavClient::builder()
            .with_uri(self.server_url.clone())
            .with_auth(Auth::Basic {
                username: self.username.clone(),
                password: Some(password),
            })
            .bootstrap(https)
            .await
            .map_err(anyhow::Error::from)?;
        Ok(builder.build())
    }
}

impl CardDavArgs {
    #[tokio::main(flavor = "current_thread")]
    pub(crate) async fn execute(&self) -> anyhow::Result<()> {
        let client = self.server.carddav_client().await?;

        match self.command {
            CardDavCommand::Discover => discover(&client),
            CardDavCommand::FindAddressBooks => list_collections(client).await?,
        };

        Ok(())
    }
}

fn discover(client: &Client) {
    println!("Discovery successful.");
    println!("- Context path: {}", &client.base_url());
    match client.addressbook_home_set() {
        Some(home_set) => println!("- Address book home set: {home_set}"),
        None => println!("- Address book home set not found."),
    }
}

async fn list_collections(client: Client) -> anyhow::Result<()> {
    let response = client.find_addresbooks(None).await?;
    for collection in response {
        println!("{}", collection.href);
    }

    Ok(())
}
