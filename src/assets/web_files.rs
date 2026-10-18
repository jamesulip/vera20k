//! In-memory install folder for the browser build (wasm32).
//!
//! Browsers give the engine no filesystem. The page copies the player's own
//! retail install into private browser storage; the web entry point reads it
//! back and installs it here once, before any `AssetManager` is created. The
//! asset manager then resolves the install directory's files by name from this
//! store instead of the disk. Archive bytes are shared, never copied.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

static FILES: OnceLock<HashMap<String, Arc<[u8]>>> = OnceLock::new();

/// Install the player's files, keyed by file name. Later calls are ignored.
pub fn install(files: impl IntoIterator<Item = (String, Vec<u8>)>) {
    let files = files
        .into_iter()
        .map(|(name, bytes)| (name.to_ascii_lowercase(), Arc::<[u8]>::from(bytes)))
        .collect();
    let _ = FILES.set(files);
}

/// The installed file names, lowercase.
pub(crate) fn names() -> impl Iterator<Item = &'static str> {
    FILES.get().into_iter().flat_map(|files| files.keys().map(String::as_str))
}

/// Bytes of the installed file whose name matches `path`'s file name.
pub(crate) fn get(path: &Path) -> Option<Arc<[u8]>> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    FILES.get()?.get(&name).cloned()
}
