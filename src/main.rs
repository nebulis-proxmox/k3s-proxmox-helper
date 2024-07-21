use anyhow::Context;
use axum::{routing::get, Router};
use clap::Parser;
use config::Config;
use network_interface::NetworkInterfaceConfig;
use once_cell::sync::Lazy;
mod certificates;
mod config;
mod error;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let address_to_listen = get_exposed_address()?;

    let app= Router::new()
        .nest("/certificates", certificates::create_router())
        .route("/", get(|| async { "Hello, World!" }));

    let listener = tokio::net::TcpListener::bind(address_to_listen).await?;

    println!("Listening on {}", listener.local_addr()?);

    axum::serve(listener, app).await?;

    Ok(())
}
