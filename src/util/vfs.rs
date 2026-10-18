//! Whole-file reads and writes that work on desktop and in the browser.
//!
//! Native builds use `std::fs`. The browser build (wasm32) has no filesystem,
//! so the same calls go to `assets::web_files`: the player's install copied
//! into memory, plus whatever the game has written since, which the web entry
//! point persists to browser storage.

use std::io;
use std::path::Path;

/// One file directly inside a listed directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub len: u64,
}

#[cfg(not(target_arch = "wasm32"))]
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    std::fs::read(path)
}

#[cfg(target_arch = "wasm32")]
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    crate::assets::web_files::get(path)
        .map(|bytes| bytes.to_vec())
        .ok_or_else(|| not_found(path))
}

pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    String::from_utf8(read(path)?).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let (path, bytes) = (path.as_ref(), bytes.as_ref());
    std::fs::write(path, bytes)
}

#[cfg(target_arch = "wasm32")]
pub fn write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let (path, bytes) = (path.as_ref(), bytes.as_ref());
    crate::assets::web_files::put(path, bytes.to_vec());
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn remove(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    std::fs::remove_file(path)
}

#[cfg(target_arch = "wasm32")]
pub fn remove(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if crate::assets::web_files::remove(path) {
        Ok(())
    } else {
        Err(not_found(path))
    }
}

/// `Path::exists` on desktop; in the browser, whether the file is stored.
pub fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    #[cfg(not(target_arch = "wasm32"))]
    {
        path.exists()
    }
    #[cfg(target_arch = "wasm32")]
    {
        crate::assets::web_files::get(path).is_some()
    }
}

pub fn is_file(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    #[cfg(not(target_arch = "wasm32"))]
    {
        path.is_file()
    }
    #[cfg(target_arch = "wasm32")]
    {
        crate::assets::web_files::get(path).is_some()
    }
}

/// The regular files directly inside `directory`, sorted by name.
#[cfg(not(target_arch = "wasm32"))]
pub fn list(directory: &Path) -> io::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            entries.push(FileEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                len: metadata.len(),
            });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

#[cfg(target_arch = "wasm32")]
pub fn list(directory: &Path) -> io::Result<Vec<FileEntry>> {
    Ok(crate::assets::web_files::list(directory)
        .into_iter()
        .map(|(name, len)| FileEntry { name, len })
        .collect())
}

#[cfg(target_arch = "wasm32")]
fn not_found(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{} not found", path.display()),
    )
}
