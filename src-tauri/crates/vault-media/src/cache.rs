use crate::MediaError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheEntry {
    pub key: String,
    pub file_id: Uuid,
    pub size_bytes: u64,
    pub sha256: String,
    pub pinned: bool,
    pub last_access_unix: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    schema: u32,
    entries: BTreeMap<String, CacheEntry>,
}
struct Inner {
    root: PathBuf,
    quota: u64,
    index: Index,
    leases: BTreeMap<Uuid, usize>,
}
#[derive(Clone)]
pub struct CacheManager(Arc<Mutex<Inner>>);
pub struct CacheLease {
    pub path: PathBuf,
    pub entry: CacheEntry,
    inner: Weak<Mutex<Inner>>,
}
impl Drop for CacheLease {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade()
            && let Ok(mut state) = inner.lock()
            && let Some(count) = state.leases.get_mut(&self.entry.file_id)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.leases.remove(&self.entry.file_id);
            }
        }
    }
}

impl CacheManager {
    pub fn open(root: impl AsRef<Path>, quota: u64) -> Result<Self, MediaError> {
        if quota == 0 {
            return Err(MediaError::CacheFull);
        }
        fs::create_dir_all(root.as_ref())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.as_ref(), fs::Permissions::from_mode(0o700))?;
        }
        if fs::symlink_metadata(root.as_ref())?
            .file_type()
            .is_symlink()
        {
            return Err(MediaError::CacheIndex);
        }
        let root = root.as_ref().canonicalize()?;
        let index_path = root.join("index.json");
        let index = if index_path.exists() {
            if fs::symlink_metadata(&index_path)?.file_type().is_symlink()
                || fs::metadata(&index_path)?.len() > 4 * 1024 * 1024
            {
                return Err(MediaError::CacheIndex);
            }
            let value: Index = serde_json::from_slice(&fs::read(&index_path)?)
                .map_err(|_| MediaError::CacheIndex)?;
            if value.schema != 1 {
                return Err(MediaError::CacheIndex);
            }
            value
        } else {
            Index {
                schema: 1,
                entries: BTreeMap::new(),
            }
        };
        let manager = Self(Arc::new(Mutex::new(Inner {
            root,
            quota,
            index,
            leases: BTreeMap::new(),
        })));
        manager.reconcile()?;
        Ok(manager)
    }
    pub fn entries(&self) -> Result<Vec<CacheEntry>, MediaError> {
        let state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        Ok(state.index.entries.values().cloned().collect())
    }
    pub fn used_bytes(&self) -> Result<u64, MediaError> {
        let state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        Ok(state.index.entries.values().map(|e| e.size_bytes).sum())
    }
    pub fn insert(
        &self,
        key: String,
        source: &Path,
        expected_sha256: &str,
        pinned: bool,
    ) -> Result<CacheLease, MediaError> {
        if key.is_empty() || key.len() > 128 || !valid_digest(expected_sha256) {
            return Err(MediaError::CacheIndex);
        }
        let source_meta = fs::symlink_metadata(source)?;
        if !source_meta.is_file() || source_meta.file_type().is_symlink() {
            return Err(MediaError::CacheIndex);
        }
        let size = source_meta.len();
        let mut lock = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        if let Some(old) = lock.index.entries.get(&key).cloned() {
            let old_path = cache_path(&lock.root, old.file_id);
            if old.sha256 == expected_sha256
                && old.size_bytes == size
                && verify_file(&old_path, size, expected_sha256).is_ok()
            {
                let entry = lock
                    .index
                    .entries
                    .get_mut(&key)
                    .ok_or(MediaError::CacheIndex)?;
                entry.last_access_unix = now();
                entry.pinned |= pinned;
                let result = entry.clone();
                lock.leases
                    .entry(result.file_id)
                    .and_modify(|n| *n += 1)
                    .or_insert(1);
                persist(&lock)?;
                return Ok(CacheLease {
                    path: old_path,
                    entry: result,
                    inner: Arc::downgrade(&self.0),
                });
            }
            if lock.leases.get(&old.file_id).copied().unwrap_or(0) > 0 {
                return Err(MediaError::CacheFull);
            }
        }
        let mut candidate = lock.index.clone();
        candidate.entries.remove(&key);
        let needed = size;
        let mut used = candidate
            .entries
            .values()
            .try_fold(0u64, |sum, entry| sum.checked_add(entry.size_bytes))
            .ok_or(MediaError::CacheIndex)?;
        let mut evictions = candidate
            .entries
            .values()
            .filter(|e| !e.pinned && lock.leases.get(&e.file_id).copied().unwrap_or(0) == 0)
            .map(|e| (e.last_access_unix, e.key.clone()))
            .collect::<Vec<_>>();
        evictions.sort();
        let mut evicted = Vec::new();
        let mut candidates = evictions.into_iter();
        while used.saturating_add(needed) > lock.quota {
            let Some((_, evict_key)) = candidates.next() else {
                return Err(MediaError::CacheFull);
            };
            if let Some(old) = candidate.entries.remove(&evict_key) {
                used = used.saturating_sub(old.size_bytes);
                evicted.push(old.file_id);
            }
        }
        let id = Uuid::new_v4();
        let destination = cache_path(&lock.root, id);
        let mut temp = tempfile::NamedTempFile::new_in(&lock.root)?;
        let mut input = File::open(source)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 256 * 1024];
        let mut written = 0u64;
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            written = written
                .checked_add(count as u64)
                .ok_or(MediaError::CacheFull)?;
            if written > size {
                return Err(MediaError::CacheIndex);
            }
            hasher.update(&buffer[..count]);
            temp.write_all(&buffer[..count])?;
        }
        if written != size || hex::encode(hasher.finalize()) != expected_sha256 {
            return Err(MediaError::CacheIndex);
        }
        temp.as_file().sync_all()?;
        temp.persist(&destination)
            .map_err(|e| MediaError::Io(e.error))?;
        let entry = CacheEntry {
            key: key.clone(),
            file_id: id,
            size_bytes: size,
            sha256: expected_sha256.to_owned(),
            pinned,
            last_access_unix: now(),
        };
        candidate.entries.insert(key, entry.clone());
        let previous = std::mem::replace(&mut lock.index, candidate);
        if let Err(error) = persist(&lock) {
            lock.index = previous;
            let _ = fs::remove_file(destination);
            return Err(error);
        }
        for old in evicted {
            let _ = fs::remove_file(cache_path(&lock.root, old));
        }
        lock.leases.insert(id, 1);
        Ok(CacheLease {
            path: destination,
            entry,
            inner: Arc::downgrade(&self.0),
        })
    }
    pub fn acquire(&self, key: &str) -> Result<Option<CacheLease>, MediaError> {
        let mut state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        let Some(entry) = state.index.entries.get(key).cloned() else {
            return Ok(None);
        };
        let path = cache_path(&state.root, entry.file_id);
        verify_file(&path, entry.size_bytes, &entry.sha256)?;
        let entry = state
            .index
            .entries
            .get_mut(key)
            .ok_or(MediaError::CacheIndex)?;
        entry.last_access_unix = now();
        let entry = entry.clone();
        state
            .leases
            .entry(entry.file_id)
            .and_modify(|n| *n += 1)
            .or_insert(1);
        persist(&state)?;
        Ok(Some(CacheLease {
            path,
            entry,
            inner: Arc::downgrade(&self.0),
        }))
    }
    pub fn pin(&self, key: &str, pinned: bool) -> Result<(), MediaError> {
        let mut state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        let entry = state
            .index
            .entries
            .get_mut(key)
            .ok_or(MediaError::CacheIndex)?;
        entry.pinned = pinned;
        persist(&state)
    }
    pub fn clear_unpinned(&self) -> Result<u64, MediaError> {
        let mut state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        let ids = state
            .index
            .entries
            .values()
            .filter(|e| !e.pinned && state.leases.get(&e.file_id).copied().unwrap_or(0) == 0)
            .map(|e| e.file_id)
            .collect::<Vec<_>>();
        let mut removed = 0u64;
        state.index.entries.retain(|_, e| {
            if ids.contains(&e.file_id) {
                removed = removed.saturating_add(e.size_bytes);
                false
            } else {
                true
            }
        });
        persist(&state)?;
        for id in ids {
            let _ = fs::remove_file(cache_path(&state.root, id));
        }
        Ok(removed)
    }
    fn reconcile(&self) -> Result<(), MediaError> {
        let mut state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        let entries = state.index.entries.values().cloned().collect::<Vec<_>>();
        for entry in entries {
            if entry.key.is_empty() || entry.file_id.is_nil() || !valid_digest(&entry.sha256) {
                return Err(MediaError::CacheIndex);
            }
            let path = cache_path(&state.root, entry.file_id);
            if fs::symlink_metadata(&path).is_err() {
                state.index.entries.remove(&entry.key);
                continue;
            }
            verify_file(&path, entry.size_bytes, &entry.sha256)?;
        }
        persist(&state)
    }
}
fn cache_path(root: &Path, id: Uuid) -> PathBuf {
    root.join(format!("{id}.media"))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn verify_file(path: &Path, size: u64, digest: &str) -> Result<(), MediaError> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() != size {
        return Err(MediaError::CacheIndex);
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    if hex::encode(hasher.finalize()) != digest {
        return Err(MediaError::CacheIndex);
    }
    Ok(())
}
fn persist(state: &Inner) -> Result<(), MediaError> {
    let path = state.root.join("index.json");
    let bytes = serde_json::to_vec(&state.index).map_err(|_| MediaError::CacheIndex)?;
    let mut temp = tempfile::NamedTempFile::new_in(&state.root)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| MediaError::Io(e.error))?;
    #[cfg(unix)]
    File::open(&state.root)?.sync_all()?;
    Ok(())
}
