use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use strum::Display;

/// docker-compose services.
#[derive(Debug, Display, Serialize, Deserialize, Clone)]
pub enum Service {
    /// RSS reader
    Miniflux,
    /// Let's Encrypt TLS certs + HTTP Host Routing
    Traefik,
    /// Documents
    Paperless,
    /// Pet Camera
    Frigate,
    /// Pastebin
    Wastebin,
    /// Notebook
    Plumio,
    /// OpenTelemetry
    OtelCollector,
    /// Homepage
    Homarr,
    /// Shell History
    Atuin,
    /// Qbittorrent + Jackett + Gluetun
    Torrents,
}

/// Infrastructure running containers.
#[derive(Debug, Display, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
#[allow(clippy::upper_case_acronyms)]
pub enum Host {
    /// Living Room Raspberry Pi
    Pi,
    /// Synology NAS
    Synology,
    /// VPS
    VPS,
}

impl Host {
    pub fn target_triple(&self) -> &'static str {
        match self {
            Host::Pi => "aarch64-unknown-linux-musl",
            Host::Synology => "x86_64-unknown-linux-musl",
            Host::VPS => "x86_64-unknown-linux-musl",
        }
    }

    pub fn ensure_target(&self) -> anyhow::Result<()> {
        let target = self.target_triple();
        let installed = Command::new("rustup")
            .arg("target")
            .arg("list")
            .arg("--installed")
            .output()?;
        if !installed.status.success() {
            anyhow::bail!(
                "failed to list Rust targets: {}",
                String::from_utf8_lossy(&installed.stderr)
            );
        }
        if String::from_utf8_lossy(&installed.stdout)
            .lines()
            .any(|installed_target| installed_target == target)
        {
            return Ok(());
        }

        let output = Command::new("rustup")
            .arg("target")
            .arg("add")
            .arg(target)
            .output()?;
        if !output.status.success() {
            anyhow::bail!(
                "failed to install Rust target {target}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    pub fn prepare_binary(&self) -> anyhow::Result<PathBuf> {
        let platform = self.target_triple();
        let output = Command::new("cargo")
            .arg("zigbuild")
            .arg("--release")
            .arg("--locked")
            .arg("--target")
            .arg(platform)
            .arg("--bin")
            .arg("underlab")
            .output()?;

        if !output.status.success() {
            anyhow::bail!(
                "failed to build: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(PathBuf::from("target")
            .join(platform)
            .join("release")
            .join("underlab"))
    }
}
