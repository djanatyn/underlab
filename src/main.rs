use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod model {
    use serde::{Deserialize, Serialize};
    use std::process::Command;
    use strum::Display;

    /// Infrastructure running containers.
    #[derive(Debug, Display, Serialize, Deserialize, Clone)]
    pub enum Host {
        /// Living Room Raspberry Pi
        Pi,
        /// Synology nAS
        Synology,
        /// VPS
        VPS,
    }

    impl Host {
        pub fn prepare_binary(&self) -> anyhow::Result<()> {
            let platform = match self {
                Host::Pi => "aarch64-unknown-linux-musl",
                Host::Synology => "x86_64-unknown-linux-musl",
                Host::VPS => "x86_64-unknown-linux-musl",
            };

            let output = Command::new("cargo")
                .arg("zigbuild")
                .arg("--release")
                .arg("--locked")
                .arg("--target")
                .arg(platform)
                .arg("--bin")
                .arg("homelab")
                .output()?;

            if !output.status.success() {
                return Err(anyhow::anyhow!(
                    "failed to build: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }

            Ok(())
        }
    }

    /// docker-compose services.
    #[derive(Debug, Display, Serialize, Deserialize, Clone)]
    pub enum Service {
        /// RSS reader
        Miniflux,
        /// Let's Encrypt TLS certs + HTTP Host routing using container labels
        Traefik,
        /// Papers and documents
        Paperless,
        /// Pet Camera
        Frigate,
        /// Pastebin
        Wastebin,
        /// Personal Notebook
        Plumio,
        /// OpenTelemetry
        OtelCollector,
        /// Homepage
        Homarr,
    }
}

mod config {
    use crate::model::{Host, Service};
    use serde::{Deserialize, Serialize};
    use std::path::{Path, PathBuf};
    use strum::Display;

    #[derive(Debug, Deserialize)]
    pub struct HomelabConfig {
        targets: Vec<DeployTargetConfig>,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub struct DeployTargetConfig {
        /// The service being deployed
        pub service: Service,
        /// The host the service is being deployed on
        pub host: Host,
        /// The root directory on the host the deployment goes to
        pub deploy_root: PathBuf,
        /// Configuration files / directories to populate
        pub files: Vec<ConfigArtifact>,
    }

    #[derive(Debug, Display, Serialize, Deserialize, Clone)]
    pub enum ConfigArtifact {
        ConfigFile {
            repo_path: PathBuf,
            artifact_path: PathBuf,
            deploy_path: PathBuf,
            mode: u16,
            owner: u32,
            group: u32,
        },
        Directory {
            deploy_path: PathBuf,
            mode: u16,
            owner: u32,
            group: u32,
        },
    }

    impl HomelabConfig {
        pub fn load() -> anyhow::Result<Self> {
            let config_bytes = std::fs::read("config/targets.ron")?;
            let config = ron::de::from_bytes::<HomelabConfig>(&config_bytes)?;
            Ok(config)
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
    }

    impl ConfigArtifact {
        pub fn build_path_mapping(&self) -> Option<(&PathBuf, &PathBuf)> {
            match self {
                ConfigArtifact::ConfigFile {
                    repo_path,
                    artifact_path,
                    ..
                } => Some((repo_path, artifact_path)),
                ConfigArtifact::Directory { .. } => None,
            }
        }
    }

    impl DeployTargetConfig {
        pub fn name(&self) -> String {
            format!("{}/{}", self.host, self.service).to_ascii_lowercase()
        }

        fn config_files(&self) -> Vec<(&PathBuf, &PathBuf)> {
            self.files
                .iter()
                .filter_map(ConfigArtifact::build_path_mapping)
                .collect()
        }

        pub fn copy_config_files(&self, path: &Path) -> anyhow::Result<()> {
            for (repo_path, artifact_path) in self.config_files() {
                let dest_path = path.join(artifact_path);
                if let Some(parent) = dest_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(repo_path, dest_path)?;
            }

            Ok(())
        }
    }
}

mod manifest {
    use crate::config::{ConfigArtifact, DeployTargetConfig};
    use crate::model::{Host, Service};

    use std::path::{Component, Path, PathBuf};

    use nix::fcntl::{openat, renameat, OFlag, AT_FDCWD};
    use nix::libc;
    use nix::sys::stat::{mkdirat, Mode};
    use nix::unistd::{chown, fchown, write, Gid, Uid};
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;
    use time::format_description::well_known::Rfc2822;
    use time::macros::format_description;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct BuildManifest {
        time: String,
        id: String,
        target: ManifestTarget,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    struct ManifestTarget {
        service: Service,
        host: Host,
        deploy_root: PathBuf,
        files: Vec<ManifestArtifact>,
    }

    #[derive(Debug, Serialize, Deserialize, Clone)]
    enum ManifestArtifact {
        ConfigFile {
            artifact_path: PathBuf,
            deploy_path: PathBuf,
            mode: u16,
            owner: u32,
            group: u32,
        },
        Directory {
            deploy_path: PathBuf,
            mode: u16,
            owner: u32,
            group: u32,
        },
    }

    impl ManifestArtifact {
        fn mode(&self) -> anyhow::Result<Mode> {
            let mode: u16 = match self {
                ManifestArtifact::ConfigFile { mode, .. } => *mode,
                ManifestArtifact::Directory { mode, .. } => *mode,
            };

            Mode::from_bits(mode as libc::mode_t).ok_or(anyhow::anyhow!("invalid mode"))
        }

        /// Deploy artifact to local host.
        fn deploy(&self) -> anyhow::Result<()> {
            match self {
                ManifestArtifact::ConfigFile {
                    artifact_path,
                    deploy_path,
                    mode,
                    owner,
                    group,
                } => {
                    let content = std::fs::read(&artifact_path)?;
                    let fd = openat(
                        AT_FDCWD,
                        dbg!(deploy_path),
                        OFlag::O_CREAT | OFlag::O_WRONLY | OFlag::O_TRUNC,
                        self.mode()?,
                    )?;
                    let _written = write(&fd, &content)?;
                    fchown(
                        &fd,
                        Some(Uid::from_raw(*owner)),
                        Some(Gid::from_raw(*group)),
                    )?;
                }
                ManifestArtifact::Directory {
                    deploy_path,
                    mode,
                    owner,
                    group,
                } => {
                    mkdirat(AT_FDCWD, dbg!(deploy_path), self.mode()?)?;
                    chown(
                        dbg!(deploy_path),
                        Some(Uid::from_raw(*owner)),
                        Some(Gid::from_raw(*group)),
                    )?;
                }
            };

            Ok(())
        }
    }

    pub fn validate_relative(field: &str, path: &Path) -> anyhow::Result<()> {
        if path.is_absolute() {
            anyhow::bail!("{field} must be relative: {}", path.display());
        }

        for component in path.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                _ => anyhow::bail!("{field} must not escape its root: {}", path.display()),
            }
        }

        Ok(())
    }

    pub fn validate_absolute(field: &str, path: &Path) -> anyhow::Result<()> {
        if !path.is_absolute() {
            anyhow::bail!("{field} must be absolute: {}", path.display());
        }

        Ok(())
    }

    impl TryFrom<&ConfigArtifact> for ManifestArtifact {
        type Error = anyhow::Error;

        fn try_from(artifact: &ConfigArtifact) -> anyhow::Result<Self> {
            match artifact {
                ConfigArtifact::ConfigFile {
                    repo_path,
                    artifact_path,
                    deploy_path,
                    mode,
                    owner,
                    group,
                } => {
                    validate_relative("repo_path", repo_path)?;
                    validate_relative("artifact_path", artifact_path)?;
                    validate_relative("deploy_path", deploy_path)?;

                    Ok(ManifestArtifact::ConfigFile {
                        artifact_path: artifact_path.clone(),
                        deploy_path: deploy_path.clone(),
                        mode: *mode,
                        owner: *owner,
                        group: *group,
                    })
                }
                ConfigArtifact::Directory {
                    deploy_path,
                    mode,
                    owner,
                    group,
                } => {
                    validate_relative("deploy_path", deploy_path)?;

                    Ok(ManifestArtifact::Directory {
                        deploy_path: deploy_path.clone(),
                        mode: *mode,
                        owner: *owner,
                        group: *group,
                    })
                }
            }
        }
    }

    impl TryFrom<DeployTargetConfig> for ManifestTarget {
        type Error = anyhow::Error;

        fn try_from(target: DeployTargetConfig) -> anyhow::Result<Self> {
            validate_absolute("deploy_root", &target.deploy_root)?;

            Ok(ManifestTarget {
                service: target.service,
                host: target.host,
                deploy_root: target.deploy_root,
                files: target
                    .files
                    .iter()
                    .map(ManifestArtifact::try_from)
                    .collect::<anyhow::Result<Vec<ManifestArtifact>>>()?,
            })
        }
    }

    impl BuildManifest {
        /// Create new manifest for intiated build
        fn new(target: DeployTargetConfig) -> anyhow::Result<Self> {
            let now = time::Timestamp::now();
            let epoch = now.format(format_description!("[unix_timestamp]"))?;
            let time = now.format(&Rfc2822)?;
            let id = format!("{}-{}-{epoch}", target.host, target.service).to_ascii_lowercase();
            let manifest_target = ManifestTarget::try_from(target)?;

            Ok(BuildManifest {
                time,
                id,
                target: manifest_target,
            })
        }

        /// Output path for manifest
        fn output_path(&self) -> anyhow::Result<PathBuf> {
            Ok(PathBuf::from("./build").join(&self.id))
        }

        /// Create applyable build tree inside working directory
        pub fn assemble(
            target: &DeployTargetConfig,
            build_directory: TempDir,
        ) -> anyhow::Result<PathBuf> {
            let manifest = BuildManifest::new(target.clone())?;
            target.copy_config_files(build_directory.path())?;
            manifest.install_manifest(&build_directory.path().join("manifest.ron"))?;
            let output = manifest.finalize(build_directory)?;
            Ok(output)
        }

        /// Render manifest
        fn render(&self) -> anyhow::Result<String> {
            Ok(ron::ser::to_string_pretty(
                self,
                ron::ser::PrettyConfig::default(),
            )?)
        }

        /// Install manifest to file
        fn install_manifest(&self, path: &Path) -> anyhow::Result<()> {
            let manifest = self.render()?;
            std::fs::write(path, &manifest)?;
            Ok(())
        }

        /// Swap output directory and temporary build working directory
        fn finalize(&self, build_directory: TempDir) -> anyhow::Result<PathBuf> {
            std::fs::create_dir_all(PathBuf::from("./build"))?;

            let output_path = self.output_path()?;
            if output_path.exists() {
                anyhow::bail!("build output already exists: {}", output_path.display());
            }
            std::fs::rename(build_directory, &output_path)?;

            Ok(output_path)
        }
    }
}

#[derive(Parser, Debug)]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    Build { target: String },
    Apply { manifest: String },
    ListTargets {},
}

use crate::config::{DeployTargetConfig, HomelabConfig};
use crate::manifest::BuildManifest;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = HomelabConfig::load()?;

    match cli.command {
        CliCommand::Build { target } => {
            let target: &DeployTargetConfig = config.lookup(&target)?;

            let build_root = PathBuf::from("./build");
            std::fs::create_dir_all(&build_root)?;
            let build_directory = tempfile::Builder::new()
                .prefix(".tmp-")
                .tempdir_in(&build_root)?;

            let result = BuildManifest::assemble(target, build_directory)?;
            println!("{}", result.display());
        }
        CliCommand::Apply { manifest } => todo!(),
        CliCommand::ListTargets {} => todo!(),
    };

    Ok(())
}
