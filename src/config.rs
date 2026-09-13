use crate::model::{Host, Service};
use crate::paths::{normalize_relative, validate_absolute};
use nix::libc;
use nix::sys::stat::Mode;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct UnderlabConfig {
    pub hosts: Vec<DeployHostConfig>,
    targets: Vec<DeployTargetConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DeployHostConfig {
    pub host: Host,
    pub ssh_host: String,
    pub ssh_user: String,
    pub staging_directory: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeployTargetConfig {
    /// The service being deployed
    pub service: Service,
    /// The host the service is being deployed on
    pub host: Host,
    /// The root directory on the host the deployment goes to
    pub deploy_root: PathBuf,
    /// Docker volumes that must exist before deployment
    pub volumes: Vec<DeployVolume>,
    /// Configuration files to populate
    pub files: Vec<ConfigFile>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeployVolume {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConfigFile {
    pub repo_path: PathBuf,
    pub artifact_path: PathBuf,
    pub deploy_path: PathBuf,
    pub mode: u16,
}

impl UnderlabConfig {
    pub fn load() -> anyhow::Result<Self> {
        let config_bytes = std::fs::read("config/targets.ron")?;
        let config = ron::de::from_bytes::<UnderlabConfig>(&config_bytes)?;
        config.validate()
    }

    fn validate(mut self) -> anyhow::Result<Self> {
        let mut hosts = HashSet::new();
        for host in &self.hosts {
            if host.ssh_host.trim().is_empty() {
                anyhow::bail!("ssh_host must not be empty");
            }
            if host.ssh_user.trim().is_empty() {
                anyhow::bail!("ssh_user must not be empty");
            }
            validate_absolute("staging_directory", &host.staging_directory)?;
            if !hosts.insert(host.host.clone()) {
                anyhow::bail!("duplicate SSH configuration for host {}", host.host);
            }
        }

        let mut targets = HashSet::new();
        for target in &mut self.targets {
            validate_absolute("deploy_root", &target.deploy_root)?;
            if !hosts.contains(&target.host) {
                anyhow::bail!("missing SSH configuration for host {}", target.host);
            }
            if !targets.insert(target.name()) {
                anyhow::bail!("duplicate deployment target: {}", target.name());
            }

            let mut volumes = HashSet::new();
            for volume in &target.volumes {
                if volume.name.is_empty()
                    || !volume.name.chars().all(|character| {
                        character.is_ascii_alphanumeric() || "_.-".contains(character)
                    })
                    || !volumes.insert(&volume.name)
                {
                    anyhow::bail!("invalid or duplicate Docker volume name: {}", volume.name);
                }
            }

            let mut artifact_paths = HashSet::new();
            let mut deploy_paths = HashSet::new();
            for file in &mut target.files {
                file.repo_path = normalize_relative("repo_path", &file.repo_path)?;
                file.artifact_path = normalize_relative("artifact_path", &file.artifact_path)?;
                file.deploy_path = normalize_relative("deploy_path", &file.deploy_path)?;
                if !artifact_paths.insert(&file.artifact_path) {
                    anyhow::bail!("duplicate artifact path: {}", file.artifact_path.display());
                }
                if !deploy_paths.insert(&file.deploy_path) {
                    anyhow::bail!("duplicate deployment path: {}", file.deploy_path.display());
                }
                Mode::from_bits(file.mode as libc::mode_t)
                    .ok_or_else(|| anyhow::anyhow!("invalid file mode"))?;
            }
        }

        Ok(self)
    }

    fn valid_targets(&self) -> Vec<String> {
        self.targets
            .iter()
            .map(DeployTargetConfig::name)
            .collect::<Vec<String>>()
    }

    pub fn lookup(&self, query: &str) -> anyhow::Result<&DeployTargetConfig> {
        let selected: Option<&DeployTargetConfig> =
            self.targets.iter().find(|target| target.name() == query);
        match selected {
            Some(selected) => Ok(selected),
            None => {
                let names = self.valid_targets().join(", ");
                Err(anyhow::anyhow!(
                    "unable to find target {query}; available targets: {names}"
                ))
            }
        }
    }

    pub fn host(&self, host: &Host) -> anyhow::Result<&DeployHostConfig> {
        self.hosts
            .iter()
            .find(|config| &config.host == host)
            .ok_or(anyhow::anyhow!(
                "unable to find SSH configuration for host {host}"
            ))
    }

    pub fn targets(&self) -> impl Iterator<Item = &DeployTargetConfig> {
        self.targets.iter()
    }
}

impl DeployTargetConfig {
    pub fn name(&self) -> String {
        format!("{}/{}", self.host, self.service).to_ascii_lowercase()
    }

    pub fn copy_config_files(&self, path: &Path) -> anyhow::Result<()> {
        for file in &self.files {
            let dest_path = path.join(&file.artifact_path);
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&file.repo_path, dest_path)?;
        }

        Ok(())
    }
}
