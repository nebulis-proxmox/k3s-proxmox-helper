use clap::Parser;

#[derive(Debug, Clone, Parser)]
pub(crate) struct Config {
    #[clap(long, env, default_value = "/srv/k8s/certificates")]
    pub certificates_path: String,

    #[clap(long, env, default_value = "vnet1")]
    pub k3s_internal_network_interface: String,

    #[clap(long, env, default_value = "3000")]
    pub port: u16,

    #[clap(long, env, default_value = "https://localhost:8006")]
    pub proxmox_api_url: String,

    #[clap(long, env)]
    pub proxmox_api_user: String,

    #[clap(env)]
    pub proxmox_api_password: String,
}
