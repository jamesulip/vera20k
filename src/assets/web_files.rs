//! In-memory install folder for the browser build (wasm32).
//!
//! Browsers give the engine no filesystem. The page copies the player's own
//! retail install into private browser storage; the web entry point reads it
//! back and installs it here once, before any `AssetManager` is created. The
//! asset manager and `util::vfs` then resolve the install directory's files
//! from this store instead of the disk. Archive bytes are shared, never copied.
//!
//! Files the game writes (options, saves) land here too and are marked dirty;
//! the web entry point copies dirty files back to browser storage, so they
//! come back with the install on the next visit.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

/// The install directory as the engine names it; its files resolve from here.
pub const WEB_RA2_DIR: &str = "/ra2";

#[derive(Default)]
struct Store {
    files: HashMap<String, Arc<[u8]>>,
    /// Written or removed since the last `take_dirty`.
    dirty: BTreeSet<String>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn store() -> MutexGuard<'static, Option<Store>> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The store key for `path`: its path below the install directory, lowercase
/// with `/` separators (retail names ignore case). Paths outside the install
/// keep their full lowercase spelling.
pub fn key(path: &Path) -> String {
    let text = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let root = format!("{WEB_RA2_DIR}/");
    text.strip_prefix(&root)
        .unwrap_or(&text)
        .trim_matches('/')
        .to_owned()
}

/// Install the player's files, keyed by their path below the install
/// directory. Only the first call takes effect.
pub fn install(files: impl IntoIterator<Item = (String, Vec<u8>)>) {
    let mut guard = store();
    if guard.is_some() {
        return;
    }
    let files = files
        .into_iter()
        .map(|(name, bytes)| (key(Path::new(&name)), Arc::<[u8]>::from(bytes)))
        .collect();
    *guard = Some(Store {
        files,
        dirty: BTreeSet::new(),
    });
}

/// The installed top-level file names, lowercase.
pub(crate) fn names() -> Vec<String> {
    store()
        .as_ref()
        .map(|store| {
            store
                .files
                .keys()
                .filter(|key| !key.contains('/'))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Bytes of the file at `path`.
pub fn get(path: &Path) -> Option<Arc<[u8]>> {
    store().as_ref()?.files.get(&key(path)).cloned()
}

/// Create or replace the file at `path`.
pub fn put(path: &Path, bytes: Vec<u8>) {
    let key = key(path);
    let mut guard = store();
    let store = guard.get_or_insert_with(Store::default);
    store.files.insert(key.clone(), bytes.into());
    store.dirty.insert(key);
}

/// Remove the file at `path`. Returns whether it existed.
pub fn remove(path: &Path) -> bool {
    let key = key(path);
    let mut guard = store();
    let Some(store) = guard.as_mut() else {
        return false;
    };
    let existed = store.files.remove(&key).is_some();
    if existed {
        store.dirty.insert(key);
    }
    existed
}

/// The files directly inside `directory`: (file name, size).
pub fn list(directory: &Path) -> Vec<(String, u64)> {
    let prefix = key(directory);
    let guard = store();
    let Some(store) = guard.as_ref() else {
        return Vec::new();
    };
    let mut entries: Vec<(String, u64)> = store
        .files
        .iter()
        .filter_map(|(key, bytes)| {
            let rest = if prefix.is_empty() {
                key.as_str()
            } else {
                key.strip_prefix(&prefix)?.strip_prefix('/')?
            };
            (!rest.contains('/')).then(|| (rest.to_owned(), bytes.len() as u64))
        })
        .collect();
    entries.sort();
    entries
}

/// Files changed since the last call: (key, new bytes, or `None` if removed).
pub fn take_dirty() -> Vec<(String, Option<Arc<[u8]>>)> {
    let mut guard = store();
    let Some(store) = guard.as_mut() else {
        return Vec::new();
    };
    std::mem::take(&mut store.dirty)
        .into_iter()
        .map(|key| {
            let bytes = store.files.get(&key).cloned();
            (key, bytes)
        })
        .collect()
}
