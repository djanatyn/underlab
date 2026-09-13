use crate::config::{ConfigFile, DeployTargetConfig};
use crate::model::{Host, Service};
use crate::paths::{normalize_relative, validate_absolute};
use crate::tarball::Tarball;
use nix::libc;
use nix::sys::stat::{Mode, fchmod};
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use time::format_description::well_known::Rfc2822;
use time::macros::format_description;
use walkdir::WalkDir;

#[derive(Debug, Deserialize, Serialize)]
pub struct BuildManifest {
    time: String,
    id: String,
    target: ManifestTarget,
}

struct ParsedManifest(BuildManifest);

impl ParsedManifest {
    fn normalize(mut self) -> anyhow::Result<BuildManifest> {
        validate_absolute("deploy_root", &self.0.target.deploy_root)?;
        for file in &mut self.0.target.files {
            file.artifact_path = normalize_relative("artifact_path", &file.artifact_path)?;
            file.deploy_path = normalize_relative("deploy_path", &file.deploy_path)?;
            Mode::from_bits(file.mode as libc::mode_t)
                .ok_or_else(|| anyhow::anyhow!("invalid file mode"))?;
        }
        Ok(self.0)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ManifestTarget {
    service: Service,
    host: Host,
    deploy_root: PathBuf,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ManifestFile {
    artifact_path: PathBuf,
    deploy_path: PathBuf,
    mode: u16,
}

pub struct ApplyOptions {
    pub dry_run: bool,
}

impl From<&ConfigFile> for ManifestFile {
    fn from(file: &ConfigFile) -> Self {
        ManifestFile {
            artifact_path: file.artifact_path.clone(),
            deploy_path: file.deploy_path.clone(),
            mode: file.mode,
        }
    }
}

impl From<DeployTargetConfig> for ManifestTarget {
    fn from(target: DeployTargetConfig) -> Self {
        ManifestTarget {
            service: target.service,
            host: target.host,
            deploy_root: target.deploy_root,
            files: target.files.iter().map(ManifestFile::from).collect(),
        }
    }
}

impl BuildManifest {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let manifest = ron::de::from_bytes(&fs::read(path)?)?;
        ParsedManifest(manifest).normalize()
    }

    pub fn host(&self) -> &Host {
        &self.target.host
    }

    fn validate(&self, archive_root: &Path) -> anyhow::Result<()> {
        if !self.target.deploy_root.is_dir() {
            anyhow::bail!(
                "deploy_root does not exist or is not a directory: {}",
                self.target.deploy_root.display()
            );
        }
        if fs::symlink_metadata(&self.target.deploy_root)?
            .file_type()
            .is_symlink()
        {
            anyhow::bail!("deploy_root must not be a symlink");
        }

        let mut expected = HashSet::from([PathBuf::from("manifest.ron")]);
        let mut destinations = HashSet::new();
        for file in &self.target.files {
            if !destinations.insert(file.deploy_path.clone()) {
                anyhow::bail!("duplicate deployment path: {}", file.deploy_path.display());
            }
            if !expected.insert(file.artifact_path.clone()) {
                anyhow::bail!(
                    "duplicate manifest artifact: {}",
                    file.artifact_path.display()
                );
            }
            let mut parent = file.artifact_path.parent();
            while let Some(path) = parent {
                if !path.as_os_str().is_empty() {
                    expected.insert(path.to_path_buf());
                }
                parent = path.parent();
            }
        }

        for entry in WalkDir::new(archive_root) {
            let entry = entry?;
            let relative = entry.path().strip_prefix(archive_root)?;
            if !relative.as_os_str().is_empty() && !expected.contains(relative) {
                anyhow::bail!("archive path is not in manifest: {}", relative.display());
            }
        }

        for file in &self.target.files {
            let archive_path = archive_root.join(&file.artifact_path);
            if !archive_path.is_file() {
                anyhow::bail!(
                    "manifest artifact is not a regular file: {}",
                    archive_path.display()
                );
            }
        }

        Ok(())
    }

    pub fn apply(&self, archive_root: &Path, options: &ApplyOptions) -> anyhow::Result<()> {
        self.validate(archive_root)?;

        for file in &self.target.files {
            let destination = self.target.deploy_root.join(&file.deploy_path);
            self.validate_destination(&destination)?;
            let content = fs::read(archive_root.join(&file.artifact_path))?;
            show_diff(&destination, &content)?;
            if !options.dry_run {
                let parent = destination
                    .parent()
                    .ok_or(anyhow::anyhow!("deployment path has no parent"))?;
                fs::create_dir_all(parent)?;
                let mut temporary = tempfile::Builder::new()
                    .prefix(".underlab-")
                    .tempfile_in(parent)?;
                temporary.write_all(&content)?;
                temporary.as_file().sync_all()?;
                let mode = Mode::from_bits(file.mode as libc::mode_t)
                    .ok_or(anyhow::anyhow!("invalid mode"))?;
                fchmod(temporary.as_file(), mode)?;
                fs::rename(temporary.path(), &destination)?;
            }
        }

        Ok(())
    }

    fn validate_destination(&self, destination: &Path) -> anyhow::Result<()> {
        for ancestor in destination.ancestors().skip(1) {
            if ancestor == self.target.deploy_root {
                break;
            }
            if fs::symlink_metadata(ancestor)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                anyhow::bail!("deployment path contains a symlink: {}", ancestor.display());
            }
        }

        Ok(())
    }

    /// Create a new manifest for an initiated build.
    fn new(target: DeployTargetConfig) -> anyhow::Result<Self> {
        let now = time::Timestamp::now();
        let epoch = now.format(format_description!("[unix_timestamp]"))?;
        let time = now.format(&Rfc2822)?;
        let id = format!("{}-{}-{epoch}", target.host, target.service).to_ascii_lowercase();
        let manifest_target = ManifestTarget::from(target);

        Ok(BuildManifest {
            time,
            id,
            target: manifest_target,
        })
    }

    fn output_path(&self) -> anyhow::Result<PathBuf> {
        Ok(PathBuf::from("./build").join(format!("{}.tar.gz", self.id)))
    }

    pub fn assemble(
        target: &DeployTargetConfig,
        build_directory: TempDir,
    ) -> anyhow::Result<Tarball> {
        let manifest = BuildManifest::new(target.clone())?;
        target.copy_config_files(build_directory.path())?;
        manifest.install_manifest(&build_directory.path().join("manifest.ron"))?;
        let output = Tarball::pack(build_directory, manifest.output_path()?)?;
        Ok(output)
    }

    fn render(&self) -> anyhow::Result<String> {
        Ok(ron::ser::to_string_pretty(
            self,
            ron::ser::PrettyConfig::default(),
        )?)
    }

    fn install_manifest(&self, path: &Path) -> anyhow::Result<()> {
        let manifest = self.render()?;
        fs::write(path, &manifest)?;
        Ok(())
    }
}

fn show_diff(path: &Path, incoming: &[u8]) -> anyhow::Result<()> {
    let existing = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    if existing == incoming {
        return Ok(());
    }

    match (
        std::str::from_utf8(&existing),
        std::str::from_utf8(incoming),
    ) {
        (Ok(old), Ok(new)) => {
            let diff = TextDiff::from_lines(old, new)
                .unified_diff()
                .header(&path.display().to_string(), &path.display().to_string())
                .to_string();
            eprint!("{diff}");
        }
        _ => eprintln!("binary files differ: {}", path.display()),
    }
    Ok(())
}
