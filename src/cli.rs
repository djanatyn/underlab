use crate::config::{DeployTargetConfig, UnderlabConfig};
use crate::manifest::BuildManifest;
use crate::process;
use crate::tarball::Tarball;
use askama::Template;
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser, Debug)]
struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    Build {
        target: String,
    },
    Apply {
        #[arg(long)]
        dry_run: bool,
        manifest_tarball: String,
    },
    Deploy {
        archive: PathBuf,
    },
    Provision {
        target: String,
    },
    ListTargets {},
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        CliCommand::Build { target } => {
            let config = UnderlabConfig::load()?;
            let target: &DeployTargetConfig = config.lookup(&target)?;

            let build_root = PathBuf::from("./build");
            fs::create_dir_all(&build_root)?;
            let build_directory = tempfile::Builder::new()
                .prefix(".tmp-")
                .tempdir_in(&build_root)?;

            let result = BuildManifest::assemble(target, build_directory)?;
            println!("{}", result.path().display());
        }
        CliCommand::Apply {
            dry_run,
            manifest_tarball,
        } => {
            let temporary = Tarball::new(PathBuf::from(manifest_tarball)).unpack()?;
            let manifest = BuildManifest::load(&temporary.path().join("manifest.ron"))?;
            manifest.apply(temporary.path(), &crate::manifest::ApplyOptions { dry_run })?;
        }
        CliCommand::Deploy { archive } => deploy(&archive)?,
        CliCommand::Provision { target } => provision(&target)?,
        CliCommand::ListTargets {} => {
            let config = UnderlabConfig::load()?;
            for target in config.targets() {
                println!("{}", target.name());
            }
        }
    };

    Ok(())
}

fn ssh_destination(host: &crate::config::DeployHostConfig) -> String {
    format!("{}@{}", host.ssh_user, host.ssh_host)
}

mod filters {
    #[askama::filter_fn]
    pub fn shell_quote(
        value: impl std::fmt::Display,
        _env: &dyn askama::Values,
    ) -> askama::Result<String> {
        let value = value.to_string();
        Ok(format!("'{}'", value.replace('\'', "'\\''")))
    }
}

#[derive(Template)]
#[template(path = "provision.sh", escape = "none")]
struct ProvisionCommand<'a> {
    volumes: &'a [crate::config::DeployVolume],
}

fn provision(target_name: &str) -> anyhow::Result<()> {
    let config = UnderlabConfig::load()?;
    let target = config.lookup(target_name)?;
    let host = config.host(&target.host)?;
    let remote_command = ProvisionCommand {
        volumes: &target.volumes,
    }
    .render()?;
    process::run(
        Command::new("ssh")
            .arg(ssh_destination(host))
            .arg(remote_command),
        "remote volume provisioning",
    )
}

fn deploy(archive: &Path) -> anyhow::Result<()> {
    let temporary = Tarball::new(archive.to_path_buf()).unpack()?;
    let manifest = BuildManifest::load(&temporary.path().join("manifest.ron"))?;
    let config = UnderlabConfig::load()?;
    let host = config.host(manifest.host())?;
    let staging = &host.staging_directory;
    let archive_name = archive
        .file_name()
        .ok_or(anyhow::anyhow!("archive path has no file name"))?;
    let remote_binary = staging.join("underlab");
    let remote_archive = staging.join(archive_name);

    let host_model = manifest.host();
    host_model.ensure_target()?;
    let binary = host_model.prepare_binary()?;
    let destination = format!("{}:{}", ssh_destination(host), staging.display());
    process::run(
        Command::new("rsync")
            .arg(&binary)
            .arg(archive)
            .arg(destination),
        "remote artifact transfer",
    )?;
    process::run(
        Command::new("ssh")
            .arg(ssh_destination(host))
            .arg(remote_binary)
            .arg("apply")
            .arg(remote_archive),
        "remote apply",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_volume_provisioning_commands() {
        let volumes = vec![crate::config::DeployVolume {
            name: "underlab_paperless_data".to_string(),
        }];
        let command = ProvisionCommand { volumes: &volumes }.render().unwrap();

        assert!(command.contains("set -eu"));
        assert!(command.contains("docker volume inspect 'underlab_paperless_data'"));
        assert!(command.contains("docker volume create 'underlab_paperless_data'"));
        assert!(!command.contains("underlab_paperless_database"));
    }
}
