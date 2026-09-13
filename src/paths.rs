use std::path::{Component, Path, PathBuf};

pub fn normalize_relative(field: &str, path: &Path) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        anyhow::bail!("{field} must be relative: {}", path.display());
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => anyhow::bail!("{field} must not escape its root: {}", path.display()),
        }
    }
    let normalized: PathBuf = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value),
            Component::CurDir => None,
            _ => unreachable!(),
        })
        .collect();
    if normalized.as_os_str().is_empty() {
        anyhow::bail!("{field} must not be empty");
    }
    Ok(normalized)
}

pub fn validate_absolute(field: &str, path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() {
        anyhow::bail!("{field} must be absolute: {}", path.display());
    }
    Ok(())
}
