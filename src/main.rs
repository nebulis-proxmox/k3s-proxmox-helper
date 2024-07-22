use std::{collections::HashMap, future::IntoFuture, net::SocketAddr, sync::Arc};

use anyhow::Context;
use axum::{routing::get, Router};
use clap::Parser;
use config::Config;
use models::ProxmoxData;
use network_interface::NetworkInterfaceConfig;
use once_cell::sync::Lazy;
use reqwest::cookie::Jar;
use serde::Deserialize;
mod certificates;
mod cluster;
mod config;
mod error;
mod models;

static CONFIG: Lazy<Config> = Lazy::new(|| Config::parse());

fn get_exposed_address() -> anyhow::Result<(std::net::IpAddr, u16)> {
    let network_interfaces = network_interface::NetworkInterface::show()?;

    let interface_to_listen = network_interfaces
        .iter()
        .find(|interface| interface.name == CONFIG.k3s_internal_network_interface)
        .context(format!(
            "Network interface {} not found",
            CONFIG.k3s_internal_network_interface
        ))?;

    let address_to_listen = interface_to_listen
        .addr
        .iter()
        .find(|addr| addr.ip().is_ipv4())
        .context("No IPv4 address found")?
        .ip();

    Ok((address_to_listen, CONFIG.port))
}

#[derive(Clone, Deserialize)]
struct ProxmoxTicket {
    #[serde(rename = "username")]
    _username: String,
    ticket: String,
    #[serde(rename = "CSRFPreventionToken")]
    _csrf_prevention_token: String,
}

async fn generate_pve_ticket() -> anyhow::Result<ProxmoxData<ProxmoxTicket>> {
    let mut params = HashMap::new();

    params.insert("username", &CONFIG.proxmox_api_user);
    params.insert("password", &CONFIG.proxmox_api_password);

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api2/json/access/ticket",
            &CONFIG.proxmox_api_url
        ))
        .form(&params)
        .send()
        .await?
        .error_for_status()?;

    Ok(response.json().await?)
}

async fn renew_ticket(ticket: &ProxmoxData<ProxmoxTicket>) -> anyhow::Result<()> {
    println!("Renewing ticket");

    let mut params = HashMap::new();

    params.insert("username", &CONFIG.proxmox_api_user);
    params.insert("password", &ticket.data.ticket);

    reqwest::Client::new()
        .post(format!(
            "{}/api2/json/access/ticket",
            &CONFIG.proxmox_api_url
        ))
        .form(&params)
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    let pve_ticket = generate_pve_ticket().await?;

    let address_to_listen = get_exposed_address()?;

    let cookie_jar = Jar::default();
    cookie_jar.add_cookie_str(
        &format!("PVEAuthCookie={}", pve_ticket.data.ticket),
        &CONFIG.proxmox_api_url.parse()?,
    );

    let client = reqwest::ClientBuilder::new()
        .cookie_provider(Arc::new(cookie_jar))
        .build()?;

    let app = Router::new()
        .nest("/cluster", cluster::create_router())
        .nest("/certificates", certificates::create_router())
        .route("/", get(|| async { "Hello, World!" }))
        .with_state(client);

    let listener = tokio::net::TcpListener::bind(address_to_listen).await?;

    println!("Listening on {}", listener.local_addr()?);

    let axum_handle = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .into_future();
    tokio::pin!(axum_handle);

    loop {
        tokio::select! {
            _ = &mut axum_handle => {
                break;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(600)) => {
                renew_ticket(&pve_ticket).await?;
            }
        }
    }

    Ok(())
}
