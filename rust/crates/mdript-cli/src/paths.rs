use std::path::{Component, Path, PathBuf};

// Resolve existing ancestors, including junctions, before comparing paths.
pub(crate) fn resolved(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => {
                out.push(other.as_os_str());
                if out.is_absolute() && out.exists() {
                    out = std::fs::canonicalize(&out).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(out)
}

pub(crate) fn is_within(path: &Path, base: &Path) -> bool {
    let mut left = path.components();
    for right in base.components() {
        let Some(candidate) = left.next() else {
            return false;
        };
        #[cfg(windows)]
        if candidate.as_os_str().to_string_lossy().to_lowercase()
            != right.as_os_str().to_string_lossy().to_lowercase()
        {
            return false;
        }
        #[cfg(not(windows))]
        if candidate != right {
            return false;
        }
    }
    true
}

pub(crate) fn validate_user_data(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("--user-data must be absolute".into());
    }
    let normalized = resolved(path)?;
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let live = resolved(&PathBuf::from(appdata).join("mdript"))?;
        if is_within(&normalized, &live) {
            return Err("live APPDATA/mdript is not allowed".into());
        }
    }
    Ok(normalized)
}
