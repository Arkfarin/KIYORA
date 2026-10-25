use crate::{MAX_IMPORT_BYTES, MediaError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use uuid::Uuid;

pub(crate) const DOWNLOAD_CHUNK_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialDownload {
    schema: u32,
    key: String,
    remote_file_id: i64,
    size_bytes: u64,
    expected_sha256: Option<String>,
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CacheStats {
    pub entries: usize,
    pub pinned_entries: usize,
    pub partial_entries: usize,
    pub partial_bytes: u64,
    /// Bytes not yet written but reserved by active downloads.
    pub reserved_bytes: u64,
    /// Completed files plus bytes already written by partial downloads.
    pub used_bytes: u64,
    pub quota_bytes: u64,
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
    partial_leases: BTreeMap<String, u64>,
}
#[derive(Clone)]
pub struct CacheManager(Arc<Mutex<Inner>>);
pub struct CacheLease {
    pub path: PathBuf,
    pub entry: CacheEntry,
    inner: Weak<Mutex<Inner>>,
}
/// Exclusive writer and quota reservation for a resumable download. Dropping it
/// releases the reservation while retaining completed chunks for a later retry.
pub struct PartialDownloadLease {
    path: PathBuf,
    offset: u64,
    key: String,
    name: String,
    cache: CacheManager,
}
impl PartialDownloadLease {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn offset(&self) -> u64 {
        self.offset
    }
    pub fn publish(self, expected_sha256: &str, pinned: bool) -> Result<CacheLease, MediaError> {
        self.cache
            .insert_inner(self.key.clone(), &self.path, expected_sha256, pinned, true)
    }
    pub fn discard(self) -> Result<(), MediaError> {
        let state = self.cache.0.lock().map_err(|_| MediaError::CacheIndex)?;
        cleanup_partial_files(&state.root, &self.key)
    }
}
impl Drop for PartialDownloadLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.cache.0.lock() {
            state.partial_leases.remove(&self.name);
        }
    }
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
    /// Open one manager per locked profile and share it through `Clone`.
    /// Reconciles local files left by an interrupted publication before use.
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
            partial_leases: BTreeMap::new(),
        })));
        manager.reconcile()?;
        Ok(manager)
    }
    pub fn entries(&self) -> Result<Vec<CacheEntry>, MediaError> {
        let state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        Ok(state.index.entries.values().cloned().collect())
    }
    pub fn used_bytes(&self) -> Result<u64, MediaError> {
        Ok(self.stats()?.used_bytes)
    }
    /// Shrinking evicts only unused, unpinned complete files. A quota that
    /// cannot accommodate pinned files and download reservations is rejected.
    pub fn set_quota(&self, bytes: u64) -> Result<CacheStats, MediaError> {
        if bytes == 0 {
            return Err(MediaError::CacheFull);
        }
        let mut state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        let previous = std::mem::replace(&mut state.quota, bytes);
        if let Err(error) = reserve_capacity(&mut state, 0, "") {
            state.quota = previous;
            return Err(error);
        }
        drop(state);
        self.stats()
    }
    pub fn stats(&self) -> Result<CacheStats, MediaError> {
        let state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        let entries = state.index.entries.values();
        let mut stats = CacheStats {
            entries: state.index.entries.len(),
            pinned_entries: 0,
            partial_entries: 0,
            partial_bytes: 0,
            reserved_bytes: 0,
            used_bytes: 0,
            quota_bytes: state.quota,
        };
        for entry in entries {
            stats.used_bytes = stats
                .used_bytes
                .checked_add(entry.size_bytes)
                .ok_or(MediaError::CacheIndex)?;
            stats.pinned_entries += usize::from(entry.pinned);
        }
        for (name, size) in partial_files(&state.root)? {
            stats.partial_entries += 1;
            stats.partial_bytes = stats
                .partial_bytes
                .checked_add(size)
                .ok_or(MediaError::CacheIndex)?;
            let reserved = state
                .partial_leases
                .get(&name)
                .copied()
                .unwrap_or(0)
                .saturating_sub(size);
            stats.reserved_bytes = stats
                .reserved_bytes
                .checked_add(reserved)
                .ok_or(MediaError::CacheIndex)?;
        }
        stats.used_bytes = stats
            .used_bytes
            .checked_add(stats.partial_bytes)
            .ok_or(MediaError::CacheIndex)?;
        Ok(stats)
    }
    pub fn prepare_partial_download(
        &self,
        key: &str,
        remote_file_id: i64,
        size_bytes: u64,
        expected_sha256: Option<&str>,
    ) -> Result<PartialDownloadLease, MediaError> {
        if key.is_empty()
            || key.len() > 128
            || remote_file_id <= 0
            || size_bytes == 0
            || size_bytes > MAX_IMPORT_BYTES
            || expected_sha256.is_some_and(|value| !valid_digest(value))
        {
            return Err(MediaError::Range);
        }
        let mut state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        let name = hex::encode(Sha256::digest(key.as_bytes()));
        if state.partial_leases.contains_key(&name) {
            return Err(MediaError::CacheBusy);
        }
        reserve_capacity(&mut state, size_bytes, &name)?;
        let data_path = state.root.join(format!("partial-{name}.data"));
        let metadata_path = state.root.join(format!("partial-{name}.json"));
        let expected = PartialDownload {
            schema: 1,
            key: key.to_owned(),
            remote_file_id,
            size_bytes,
            expected_sha256: expected_sha256.map(str::to_owned),
        };
        let mut reusable = false;
        if metadata_path.exists() {
            let metadata = fs::symlink_metadata(&metadata_path)?;
            if !metadata.file_type().is_symlink() && metadata.is_file() && metadata.len() <= 4096 {
                let existing =
                    serde_json::from_slice::<PartialDownload>(&fs::read(&metadata_path)?).ok();
                reusable = existing.is_some_and(|value| {
                    value.schema == expected.schema
                        && value.key == expected.key
                        && value.remote_file_id == expected.remote_file_id
                        && value.size_bytes == expected.size_bytes
                        && value.expected_sha256 == expected.expected_sha256
                });
            }
        }
        if !reusable {
            remove_if_regular_or_missing(&metadata_path)?;
            remove_if_regular_or_missing(&data_path)?;
            let encoded = serde_json::to_vec(&expected).map_err(|_| MediaError::CacheIndex)?;
            let mut temp = staging_file(&state.root)?;
            temp.write_all(&encoded)?;
            temp.as_file().sync_all()?;
            temp.persist(&metadata_path)
                .map_err(|error| MediaError::Io(error.error))?;
            #[cfg(unix)]
            File::open(&state.root)?.sync_all()?;
        }
        if !data_path.exists() {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            options.open(&data_path)?.sync_all()?;
        }
        let data_metadata = fs::symlink_metadata(&data_path)?;
        if !data_metadata.is_file() || data_metadata.file_type().is_symlink() {
            return Err(MediaError::CacheIndex);
        }
        let available = data_metadata.len().min(size_bytes);
        let offset = if available == size_bytes {
            available
        } else {
            available / DOWNLOAD_CHUNK_BYTES * DOWNLOAD_CHUNK_BYTES
        };
        let file = OpenOptions::new().write(true).open(&data_path)?;
        file.set_len(offset)?;
        file.sync_data()?;
        state.partial_leases.insert(name.clone(), size_bytes);
        Ok(PartialDownloadLease {
            path: data_path,
            offset,
            key: key.to_owned(),
            name,
            cache: self.clone(),
        })
    }
    pub fn partial_download_progress(&self, key: &str) -> Result<Option<(u64, u64)>, MediaError> {
        if key.is_empty() || key.len() > 128 {
            return Err(MediaError::Range);
        }
        let state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        let name = hex::encode(Sha256::digest(key.as_bytes()));
        let data_path = state.root.join(format!("partial-{name}.data"));
        let metadata_path = state.root.join(format!("partial-{name}.json"));
        let metadata = match fs::symlink_metadata(&metadata_path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(MediaError::Io(error)),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
            return Err(MediaError::CacheIndex);
        }
        let partial: PartialDownload = serde_json::from_slice(&fs::read(metadata_path)?)
            .map_err(|_| MediaError::CacheIndex)?;
        if partial.schema != 1
            || partial.key != key
            || partial.remote_file_id <= 0
            || partial.size_bytes == 0
            || partial
                .expected_sha256
                .as_deref()
                .is_some_and(|value| !valid_digest(value))
        {
            return Err(MediaError::CacheIndex);
        }
        let data_metadata = match fs::symlink_metadata(data_path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(MediaError::Io(error)),
        };
        if !data_metadata.is_file() || data_metadata.file_type().is_symlink() {
            return Err(MediaError::CacheIndex);
        }
        let available = data_metadata.len().min(partial.size_bytes);
        let complete = if available == partial.size_bytes {
            available
        } else {
            available / DOWNLOAD_CHUNK_BYTES * DOWNLOAD_CHUNK_BYTES
        };
        Ok((complete > 0).then_some((complete, partial.size_bytes)))
    }
    pub fn finish_partial_download(&self, key: &str) -> Result<(), MediaError> {
        if key.is_empty() || key.len() > 128 {
            return Err(MediaError::Range);
        }
        let state = self.0.lock().map_err(|_| MediaError::CacheIndex)?;
        let name = hex::encode(Sha256::digest(key.as_bytes()));
        if state.partial_leases.contains_key(&name) {
            return Err(MediaError::CacheBusy);
        }
        for path in [
            state.root.join(format!("partial-{name}.data")),
            state.root.join(format!("partial-{name}.json")),
        ] {
            remove_if_regular_or_missing(&path)?;
        }
        #[cfg(unix)]
        File::open(&state.root)?.sync_all()?;
        Ok(())
    }
    pub fn insert(
        &self,
        key: String,
        source: &Path,
        expected_sha256: &str,
        pinned: bool,
    ) -> Result<CacheLease, MediaError> {
        self.insert_inner(key, source, expected_sha256, pinned, false)
    }
    fn insert_inner(
        &self,
        key: String,
        source: &Path,
        expected_sha256: &str,
        pinned: bool,
        owns_partial: bool,
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
        let partial_name = hex::encode(Sha256::digest(key.as_bytes()));
        if lock.partial_leases.contains_key(&partial_name) && !owns_partial {
            return Err(MediaError::CacheBusy);
        }
        if owns_partial && lock.partial_leases.get(&partial_name) != Some(&size) {
            return Err(MediaError::Range);
        }
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
                persist(&lock)?;
                cleanup_partial_files(&lock.root, &result.key)?;
                lock.leases
                    .entry(result.file_id)
                    .and_modify(|n| *n += 1)
                    .or_insert(1);
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
        used = used
            .checked_add(partial_charge(&lock, Some(&partial_name))?)
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
        while used
            .checked_add(needed)
            .is_none_or(|total| total > lock.quota)
        {
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
        if owns_partial {
            verify_file(source, size, expected_sha256)?;
            fs::rename(source, &destination)?;
        } else {
            let mut temp = staging_file(&lock.root)?;
            let mut input = File::open(source)?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0u8; 256 * 1024];
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
        }
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
            if owns_partial {
                fs::rename(&destination, source)?;
            } else {
                let _ = fs::remove_file(destination);
            }
            return Err(error);
        }
        for old in evicted {
            let _ = fs::remove_file(cache_path(&lock.root, old));
        }
        if let Some(old) = previous.entries.get(&entry.key) {
            let _ = fs::remove_file(cache_path(&lock.root, old.file_id));
        }
        cleanup_partial_files(&lock.root, &entry.key)?;
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
        let partial_name = hex::encode(Sha256::digest(key.as_bytes()));
        if !state.partial_leases.contains_key(&partial_name) {
            cleanup_partial_files(&state.root, key)?;
        }
        let path = cache_path(&state.root, entry.file_id);
        verify_file(&path, entry.size_bytes, &entry.sha256)?;
        let entry = state
            .index
            .entries
            .get_mut(key)
            .ok_or(MediaError::CacheIndex)?;
        entry.last_access_unix = now();
        let entry = entry.clone();
        persist(&state)?;
        state
            .leases
            .entry(entry.file_id)
            .and_modify(|n| *n += 1)
            .or_insert(1);
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
        let previous = std::mem::replace(&mut entry.pinned, pinned);
        if let Err(error) = persist(&state) {
            if let Some(entry) = state.index.entries.get_mut(key) {
                entry.pinned = previous;
            }
            return Err(error);
        }
        Ok(())
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
        let previous = state.index.clone();
        state.index.entries.retain(|_, e| {
            if ids.contains(&e.file_id) {
                removed = removed.saturating_add(e.size_bytes);
                false
            } else {
                true
            }
        });
        if let Err(error) = persist(&state) {
            state.index = previous;
            return Err(error);
        }
        for id in ids {
            let _ = fs::remove_file(cache_path(&state.root, id));
        }
        for (name, size) in partial_files(&state.root)? {
            if !state.partial_leases.contains_key(&name) {
                remove_if_regular_or_missing(&state.root.join(format!("partial-{name}.data")))?;
                remove_if_regular_or_missing(&state.root.join(format!("partial-{name}.json")))?;
                removed = removed.checked_add(size).ok_or(MediaError::CacheIndex)?;
            }
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
        let known: std::collections::BTreeSet<_> = state
            .index
            .entries
            .values()
            .map(|entry| entry.file_id)
            .collect();
        for entry in fs::read_dir(&state.root)? {
            let entry = entry?;
            let filename = entry.file_name();
            let Some(filename) = filename.to_str() else {
                continue;
            };
            let orphan = filename
                .strip_suffix(".media")
                .and_then(|id| id.parse::<Uuid>().ok())
                .is_some_and(|id| filename == format!("{id}.media") && !known.contains(&id));
            if orphan || filename.starts_with(".kiyora-cache-") {
                remove_if_regular_or_missing(&entry.path())?;
            }
        }
        for name in partial_files(&state.root)?.keys() {
            if !state.root.join(format!("partial-{name}.data")).exists() {
                remove_if_regular_or_missing(&state.root.join(format!("partial-{name}.json")))?;
            }
        }
        persist(&state)
    }
}
/// Only filenames owned by the partial-download format participate. Unknown
/// files are never interpreted as download data or removed by cache clearing.
fn partial_files(root: &Path) -> Result<BTreeMap<String, u64>, MediaError> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str().and_then(|name| name.strip_prefix("partial-")) else {
            continue;
        };
        let Some(digest) = name
            .strip_suffix(".data")
            .or_else(|| name.strip_suffix(".json"))
        else {
            continue;
        };
        if !valid_digest(digest) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(MediaError::CacheIndex);
        }
        let size = files.entry(digest.to_owned()).or_insert(0);
        if name.ends_with(".data") {
            *size = metadata.len();
        }
    }
    Ok(files)
}
fn partial_charge(state: &Inner, excluding: Option<&str>) -> Result<u64, MediaError> {
    partial_files(&state.root)?
        .into_iter()
        .filter(|(name, _)| Some(name.as_str()) != excluding)
        .try_fold(0u64, |sum, (name, bytes)| {
            let reserved = state.partial_leases.get(&name).copied().unwrap_or(0);
            sum.checked_add(bytes.max(reserved))
                .ok_or(MediaError::CacheIndex)
        })
}
/// Reserve the full future download size. Only unpinned, unused complete files
/// are LRU eviction candidates; paused partial downloads remain until cleared.
fn reserve_capacity(state: &mut Inner, needed: u64, partial_name: &str) -> Result<(), MediaError> {
    let mut used = state.index.entries.values().try_fold(
        partial_charge(state, Some(partial_name))?,
        |sum, entry| {
            sum.checked_add(entry.size_bytes)
                .ok_or(MediaError::CacheIndex)
        },
    )?;
    let mut candidate = state.index.clone();
    let mut evictions = candidate
        .entries
        .values()
        .filter(|entry| !entry.pinned && !state.leases.contains_key(&entry.file_id))
        .map(|entry| (entry.last_access_unix, entry.key.clone()))
        .collect::<Vec<_>>();
    evictions.sort();
    let mut victims = evictions.into_iter();
    let mut removed = Vec::new();
    while used
        .checked_add(needed)
        .is_none_or(|total| total > state.quota)
    {
        let Some((_, key)) = victims.next() else {
            return Err(MediaError::CacheFull);
        };
        if let Some(entry) = candidate.entries.remove(&key) {
            used = used
                .checked_sub(entry.size_bytes)
                .ok_or(MediaError::CacheIndex)?;
            removed.push(entry.file_id);
        }
    }
    if !removed.is_empty() {
        let previous = std::mem::replace(&mut state.index, candidate);
        if let Err(error) = persist(state) {
            state.index = previous;
            return Err(error);
        }
        for id in removed {
            remove_if_regular_or_missing(&cache_path(&state.root, id))?;
        }
    }
    Ok(())
}
fn cache_path(root: &Path, id: Uuid) -> PathBuf {
    root.join(format!("{id}.media"))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn remove_if_regular_or_missing(path: &Path) -> Result<(), MediaError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path)?;
            Ok(())
        }
        Ok(_) => Err(MediaError::CacheIndex),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(MediaError::Io(error)),
    }
}
fn cleanup_partial_files(root: &Path, key: &str) -> Result<(), MediaError> {
    if key.is_empty() || key.len() > 128 {
        return Err(MediaError::Range);
    }
    let name = hex::encode(Sha256::digest(key.as_bytes()));
    remove_if_regular_or_missing(&root.join(format!("partial-{name}.data")))?;
    remove_if_regular_or_missing(&root.join(format!("partial-{name}.json")))?;
    Ok(())
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
    let mut buffer = vec![0u8; 256 * 1024];
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
    let mut temp = staging_file(&state.root)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| MediaError::Io(e.error))?;
    #[cfg(unix)]
    File::open(&state.root)?.sync_all()?;
    Ok(())
}
fn staging_file(root: &Path) -> Result<tempfile::NamedTempFile, std::io::Error> {
    tempfile::Builder::new()
        .prefix(".kiyora-cache-")
        .tempfile_in(root)
}
