use clap::Parser;


#[derive(Debug, Clone, Parser)]
pub(crate) struct Config {
    #[clap(long, default_value = "/srv/k8s/certificates")]
    pub certificates_path: String,

    #[clap(long, default_value = "vnet1")]
    pub k3s_internal_network_interface: String,

    #[clap(long, default_value = "3000")]
    pub port: u16,
}