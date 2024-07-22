use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use anyhow::Context;
use axum::{routing::get, Router};
use clap::Parser;
use cluster::IpamEntry;
use config::Config;
use models::ProxmoxData;
use network_interface::NetworkInterfaceConfig;
use once_cell::sync::Lazy;
use reqwest::cookie::Jar;
use serde::Deserialize;
use tokio::{net::TcpStream, sync::watch};
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

async fn setup_webserver(client: reqwest::Client) -> anyhow::Result<()> {
    let address_to_listen = get_exposed_address()?;

    let app = Router::new()
        .nest("/cluster", cluster::create_router())
        .nest("/certificates", certificates::create_router())
        .route("/", get(|| async { "Hello, World!" }))
        .with_state(client);

    let listener = tokio::net::TcpListener::bind(address_to_listen).await?;

    println!("Listening on {}", listener.local_addr()?);

    Ok(axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?)
}

async fn synchronize_ipams(
    tx: watch::Sender<Vec<IpamEntry>>,
    client: reqwest::Client,
) -> anyhow::Result<()> {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;

        let nodes = cluster::get_nodes(client.clone()).await?.data;

        let mut ipams = vec![];

        for node in nodes {
            ipams.extend(
                cluster::get_ipams_for_node(client.clone(), &node.node)
                    .await?
                    .data
                    .into_iter()
                    .filter(|ipam| {
                        ipam.hostname
                            .clone()
                            .is_some_and(|hostname| hostname.starts_with("k3s-server"))
                    }),
            );
        }

        tx.send(ipams)?;
    }
}

async fn proxy_k8s_servers(rx: watch::Receiver<Vec<IpamEntry>>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", 6443)).await?;

    loop {
        let (mut ingress, _) = listener.accept().await?;

        let ipams = rx.borrow().clone();

        tokio::spawn(async move {
            let mut ipam_idx = 0;

            let egress = loop {
                if ipam_idx >= ipams.len() {
                    break None;
                }

                let ipam = &ipams[ipam_idx];

                if let Ok(connection) = TcpStream::connect((ipam.ip.as_str(), 6443)).await {
                    break Some(connection);
                } else {
                    ipam_idx += 1;
                }
            };

            let mut egress = if let Some(egress) = egress {
                egress
            } else {
                panic!("Impossible to connect to any k3s-server");
            };

            match tokio::io::copy_bidirectional(&mut ingress, &mut egress).await {
                Ok((to_egress, to_ingress)) => {
                    println!(
                        "Connection ended gracefully ({to_egress} bytes from client, {to_ingress} bytes from server)"
                    );
                }
                Err(err) => {
                    println!("Error while proxying: {}", err);
                }
            }
        });
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    let pve_ticket = generate_pve_ticket().await?;

    let cookie_jar = Jar::default();
    cookie_jar.add_cookie_str(
        &format!("PVEAuthCookie={}", pve_ticket.data.ticket),
        &CONFIG.proxmox_api_url.parse()?,
    );

    let client = reqwest::ClientBuilder::new()
        .cookie_provider(Arc::new(cookie_jar))
        .build()?;

    let (tx, rx) = watch::channel(Vec::new());

    let axum_handle = setup_webserver(client.clone());
    tokio::pin!(axum_handle);

    let synchronize_ipams_handle = synchronize_ipams(tx, client.clone());
    tokio::pin!(synchronize_ipams_handle);

    let proxy_k8s_servers_handle = proxy_k8s_servers(rx);
    tokio::pin!(proxy_k8s_servers_handle);

    loop {
        tokio::select! {
            _ = &mut axum_handle => {
                break;
            }
            _ = &mut synchronize_ipams_handle => {
                break;
            }
            _ = &mut proxy_k8s_servers_handle => {
                break;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(600)) => {
                renew_ticket(&pve_ticket).await?;
            }
        }
    }

    Ok(())
}
