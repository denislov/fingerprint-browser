//! Reading and atomically replacing the settings document.
use super::*;

pub(super) fn read_config(path: &Path, t: &Text) -> Result<Stored, String> {
    let display = path.display().to_string();
    let failed = |error: &dyn std::fmt::Display| t.config_read_failed(&display, &error.to_string());
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|error| failed(&error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Stored::default()),
        Err(error) => Err(failed(&error)),
    }
}

pub(super) fn write_config(path: &Path, stored: &Stored, t: &Text) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            t.config_create_failed(&parent.display().to_string(), &error.to_string())
        })?;
    }
    let text = serde_json::to_string_pretty(stored)
        .map_err(|error| t.config_encode_failed(&error.to_string()))?;
    storage::atomic_file::write(path, format!("{text}\n").as_bytes()).map_err(|error| {
        t.config_file_write_failed(&path.display().to_string(), &error.to_string())
    })
}
