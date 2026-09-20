use std::{fs, io, path::Path};

use serde::Serialize;

pub fn save_json_atomic(path: impl AsRef<Path>, value: &impl Serialize) -> io::Result<()> {
    let path = path.as_ref();
    let directory = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(directory)?;
    // Keep serialization and disk-write failures from truncating the previous document.
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(&mut file, value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}
