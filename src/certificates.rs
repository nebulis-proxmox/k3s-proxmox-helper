use std::path::PathBuf;

use axum::{routing::post, Json, Router};
use mktemp::Temp;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::{error::AppResult, CONFIG};

#[derive(Deserialize)]
pub(crate) struct GenerateCertificateRequest {
    certificate_type: String,
}

#[derive(Serialize)]
pub(crate) struct GenerateCertificateResponse {
    private_key: String,
    certificate_pem: String,
    certificate_chain: String,
}

#[axum::debug_handler]
pub(crate) async fn generate_certificate(
    Json(request): Json<GenerateCertificateRequest>,
) -> AppResult<Json<GenerateCertificateResponse>> {
    let temp_dir = Temp::new_dir()?;

    let ca_paths = PathBuf::from(&CONFIG.certificates_path);

    let private_key_path = temp_dir
        .join("private.key")
        .as_path()
        .display()
        .to_string()
        .clone();

    Command::new("openssl")
        .args(&[
            "ecparam",
            "-name",
            "prime256v1",
            "-genkey",
            "-noout",
            "-out",
            &private_key_path,
        ])
        .output()
        .await?;

    let private_key = std::fs::read_to_string(&private_key_path)?;

    let certificate_type = request.certificate_type.replace("/", "-");
    let timestamp = chrono::Utc::now().timestamp();
    let subject_name = format!("/CN=k3s-{certificate_type}@{timestamp}");

    let csr_path = temp_dir
        .join("certificate.csr")
        .as_path()
        .display()
        .to_string()
        .clone();

    Command::new("openssl")
        .args(&[
            "req",
            "-new",
            "-nodes",
            "-subj",
            &subject_name,
            "-key",
            &private_key_path,
            "-out",
            &csr_path,
        ])
        .output()
        .await?;

    let certificate_path = temp_dir
        .join("certificate.pem")
        .as_path()
        .display()
        .to_string()
        .clone();

    let intermediate_private_key_path = ca_paths
        .join("intermediate-ca.key")
        .as_path()
        .display()
        .to_string()
        .clone();

    let intermediate_certificate_path = ca_paths
        .join("intermediate-ca.pem")
        .as_path()
        .display()
        .to_string()
        .clone();

    let ca_config_path = ca_paths
        .join(".ca")
        .join("config")
        .as_path()
        .display()
        .to_string()
        .clone();

    Command::new("openssl")
        .args(&[
            "ca",
            "-batch",
            "-notext",
            "-days",
            "3700",
            "-in",
            &csr_path,
            "-out",
            &certificate_path,
            "-keyfile",
            &intermediate_private_key_path,
            "-cert",
            &intermediate_certificate_path,
            "-config",
            &ca_config_path,
            "-extensions",
            "v3_ca",
        ])
        .output()
        .await?;

    let root_ca_pem_path = ca_paths
        .join("root-ca.pem")
        .as_path()
        .display()
        .to_string()
        .clone();

    let root_ca_pem = std::fs::read_to_string(&root_ca_pem_path)?;

    let intermediate_ca_pem_path = ca_paths
        .join("intermediate-ca.pem")
        .as_path()
        .display()
        .to_string()
        .clone();

    let intermediate_ca_pem = std::fs::read_to_string(&intermediate_ca_pem_path)?;

    let certificate_pem = std::fs::read_to_string(&certificate_path)?;

    let certificate_chain = format!("{certificate_pem}{intermediate_ca_pem}{root_ca_pem}");

    Ok(Json(GenerateCertificateResponse {
        private_key,
        certificate_pem,
        certificate_chain,
    }))
}

pub(crate) fn create_router() -> Router {
    Router::new().route("/generate", post(generate_certificate))
}
