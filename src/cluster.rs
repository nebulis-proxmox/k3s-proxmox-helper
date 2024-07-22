use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, Path, State},
    routing::get,
    Json, Router,
};
use mktemp::Temp;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::{error::AppResult, models::ProxmoxData, CONFIG};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IpamEntry {
    pub zone: String,
    pub hostname: Option<String>,
    pub vmid: Option<String>,
    pub vnet: String,
    pub ip: String,
    pub mac: Option<String>,
    pub subnet: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct NodeEntry {
    pub cpu: f64,
    pub maxcpu: i32,
    pub mem: i64,
    pub maxmem: i64,
    pub disk: i64,
    pub maxdisk: i64,
    pub uptime: i64,
    pub status: String,
    pub node: String,
    pub level: String,
    pub ssl_fingerprint: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VirtualMachineEntry {
    pub status: String,
    pub vmid: i64,
    pub name: String,
    pub template: Option<u8>,
}

pub(crate) async fn get_nodes(
    client: reqwest::Client,
) -> anyhow::Result<ProxmoxData<Vec<NodeEntry>>> {
    Ok(client
        .get(format!("{}/api2/json/nodes", &CONFIG.proxmox_api_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

pub(crate) async fn get_ipams_for_node<S: AsRef<str>>(
    client: reqwest::Client,
    node: S,
) -> anyhow::Result<ProxmoxData<Vec<IpamEntry>>> {
    Ok(client
        .get(format!(
            "{}/api2/json/cluster/sdn/ipams/{}/status",
            &CONFIG.proxmox_api_url,
            node.as_ref()
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

pub(crate) async fn get_all_vms_for_node<S: AsRef<str>>(
    client: reqwest::Client,
    node: S,
) -> anyhow::Result<ProxmoxData<Vec<VirtualMachineEntry>>> {
    Ok(client
        .get(format!(
            "{}/api2/json/nodes/{}/qemu",
            &CONFIG.proxmox_api_url,
            node.as_ref()
        ))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn get_nodes_infos(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(client): State<reqwest::Client>,
) -> AppResult<Json<Vec<IpamEntry>>> {
    let nodes = get_nodes(client.clone()).await?.data;
    let mut ipams = vec![];

    for node in nodes {
        let vms = get_all_vms_for_node(client.clone(), &node.node).await?.data;

        ipams.extend(
            get_ipams_for_node(client.clone(), &node.node)
                .await?
                .data
                .into_iter()
                .filter(
                    |entry| entry.vnet == "vnet1", /*CONFIG.k3s_internal_network_interface*/
                )
                .filter(|entry| addr.ip().to_string() != entry.ip)
                .filter(|entry| entry.vmid.is_some())
                .filter(|entry| {
                    vms.iter()
                        .find(|v| {
                            entry
                                .vmid
                                .clone()
                                .is_some_and(|vmid| vmid == v.vmid.to_string())
                        })
                        .is_some_and(|v| v.template.is_none() && v.status == "running")
                }),
        );
    }

    Ok(Json(ipams))
}

async fn get_node_token(
    Path(vm_id): Path<String>,
    State(client): State<reqwest::Client>,
) -> AppResult<String> {
    let nodes = get_nodes(client.clone()).await?.data;

    for node in nodes {
        let ipams = get_ipams_for_node(client.clone(), &node.node).await?.data;

        for ipam in ipams {
            if ipam.vmid.is_some_and(|ipam_vmid| ipam_vmid == vm_id) {
                let temp = Temp::new_dir()?;

                let token_path = temp.join("token").as_path().display().to_string().clone();

                Command::new("scp")
                    .arg("-o")
                    .arg("StrictHostKeyChecking=no")
                    .arg("-o")
                    .arg("UserKnownHostsFile=/dev/null")
                    .arg(format!(
                        "root@{}:/var/lib/rancher/k3s/server/token",
                        ipam.ip
                    ))
                    .arg(&token_path)
                    .output()
                    .await?;

                let token = std::fs::read_to_string(&token_path)?;

                return Ok(token);
            }
        }
    }

    Err(anyhow::Error::msg("VM not found").into())
}

async fn get_current_node_id(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(client): State<reqwest::Client>,
) -> AppResult<String> {
    let nodes = get_nodes(client.clone()).await?.data;

    for node in nodes {
        let ipams = get_ipams_for_node(client.clone(), &node.node).await?.data;

        if let Some(ip) = ipams
            .iter()
            .filter(|ipam| ipam.vmid.is_some())
            .find(|ipam| addr.ip().to_string() == ipam.ip)
        {
            return Ok(ip.vmid.clone().unwrap());
        }
    }

    Err(anyhow::Error::msg("VM not found").into())
}

pub(crate) fn create_router() -> Router<reqwest::Client> {
    Router::new()
        .route("/nodes", get(get_nodes_infos))
        .route("/current", get(get_current_node_id))
        .route("/:vmid/token", get(get_node_token))
}
