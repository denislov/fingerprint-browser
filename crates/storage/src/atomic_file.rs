//! Replace a file only after its entire replacement has been written and synced.
use std::{
    fs::File,
    io::{self, Write},
    path::Path,
};

pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, |file| file.write_all(bytes))
}

fn replace_with(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Same filesystem, exclusive creation, private permissions, automatic
    // cleanup on error/unwind, and platform-specific atomic replacement.
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    write(temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_write_failure_preserves_the_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup.json");
        std::fs::write(&path, b"previous complete backup").unwrap();
        let result = replace_with(&path, |file| {
            file.write_all(b"incomplete")?;
            Err(io::Error::other("disk full"))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous complete backup");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        write(&path, b"replacement").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    }

    #[test]
    fn a_failed_rename_does_not_remove_the_target_or_leak_a_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("directory");
        std::fs::create_dir(&target).unwrap();
        assert!(write(&target, b"data").is_err());
        assert!(target.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
