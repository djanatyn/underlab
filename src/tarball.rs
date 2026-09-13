use crate::paths::normalize_relative;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder, EntryType};
use tempfile::TempDir;
use walkdir::WalkDir;

pub struct Tarball {
    path: PathBuf,
}

impl Tarball {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn pack(source: TempDir, path: PathBuf) -> anyhow::Result<Self> {
        if path.exists() {
            anyhow::bail!("tarball output already exists: {}", path.display());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let output = File::create(&path)?;
        let encoder = GzEncoder::new(output, Compression::default());
        let mut archive = Builder::new(encoder);
        for entry in WalkDir::new(source.path()) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry.path().strip_prefix(source.path())?;
            archive.append_path_with_name(entry.path(), relative)?;
        }
        archive.into_inner()?.finish()?;

        Ok(Self { path })
    }

    pub fn unpack(&self) -> anyhow::Result<TempDir> {
        let temporary = tempfile::tempdir()?;
        let file = File::open(&self.path)?;
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        let mut paths = HashSet::new();
        for item in archive.entries()? {
            let mut entry = item?;
            let entry_path = entry.path()?.into_owned();
            let normalized_path = normalize_relative("archive path", &entry_path)?;
            if normalized_path.as_os_str().is_empty() || !paths.insert(normalized_path.clone()) {
                anyhow::bail!(
                    "invalid or duplicate archive path: {}",
                    entry_path.display()
                );
            }
            let entry_type = entry.header().entry_type();
            if entry_type != EntryType::Regular && entry_type != EntryType::Directory {
                anyhow::bail!(
                    "archive contains unsupported entry: {}",
                    entry_path.display()
                );
            }
            entry.unpack_in(temporary.path())?;
        }
        if !paths.contains(Path::new("manifest.ron")) {
            anyhow::bail!("archive does not contain manifest.ron");
        }
        Ok(temporary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_and_unpacks_gzip_tarball() {
        let source = tempfile::tempdir().unwrap();
        fs::write(source.path().join("manifest.ron"), b"manifest").unwrap();
        let output = tempfile::tempdir().unwrap();
        let path = output.path().join("build.tar.gz");

        let tarball = Tarball::pack(source, path).unwrap();
        let extracted = tarball.unpack().unwrap();

        assert_eq!(
            fs::read(extracted.path().join("manifest.ron")).unwrap(),
            b"manifest"
        );
    }
}
