//! Portable uncompressed tar container with a bounded zstd state document.
//! No generic archive extraction: all output files have application-chosen paths.
use crate::{MAX_COMPRESSED, MAX_UNCOMPRESSED, SnapshotError, digest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;
use vault_domain::{Action, ChatId, MessageId, Source, State, UserId};
use vault_protocol::{canonical_json, strict_json};

const README: &str = "kiyora portable backup v1\nThis archive contains shared music library metadata and optionally media. No TDLib sessions, local paths, passwords or encryption keys are included. SHA-256 detects corruption; it is not an owner signature. Metadata-only backups do not preserve lost audio. Restore is explicit and never silently overwrites an existing vault.\n";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    Metadata,
    Full,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checksum {
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub protocol: u32,
    pub state_schema: u32,
    pub kind: BackupKind,
    pub vault_id: Uuid,
    pub owner_id: UserId,
    pub generation: u64,
    pub cursor: Option<MessageId>,
    pub barrier_message_id: Option<MessageId>,
    pub track_count: usize,
    pub playlist_count: usize,
    pub entries: BTreeMap<String, Checksum>,
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub total_bytes: u64,
    pub media_file_bytes: u64,
    pub entries: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            total_bytes: 64 * 1024 * 1024 * 1024,
            media_file_bytes: 8 * 1024 * 1024 * 1024,
            entries: 100_004,
        }
    }
}
pub struct VerifiedBackup {
    pub manifest: Manifest,
    pub state: State,
    media: BTreeMap<Uuid, PathBuf>,
    _staging: tempfile::TempDir,
}
impl VerifiedBackup {
    pub fn media_path(&self, track: Uuid) -> Option<&Path> {
        self.media.get(&track).map(PathBuf::as_path)
    }
}

fn append<W: Write, R: Read>(
    builder: &mut tar::Builder<W>,
    name: &str,
    size: u64,
    reader: R,
) -> Result<Checksum, SnapshotError> {
    let mut header = tar::Header::new_ustar();
    header.set_size(size);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    let mut hashing = HashingReader {
        reader: reader.take(size),
        hasher: Sha256::new(),
        bytes: 0,
    };
    builder.append_data(&mut header, name, &mut hashing)?;
    if hashing.bytes != size {
        return Err(SnapshotError::Corrupt);
    }
    Ok(Checksum {
        bytes: size,
        sha256: hex::encode(hashing.hasher.finalize()),
    })
}
struct HashingReader<R> {
    reader: R,
    hasher: Sha256,
    bytes: u64,
}
impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.reader.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        self.bytes += read as u64;
        Ok(read)
    }
}

/// Atomic no-clobber export. Full mode requires a verified local media source for
/// every active track. Paths are never embedded in the state or manifest.
pub fn export(
    destination: &Path,
    state: &State,
    owner: UserId,
    barrier: Option<MessageId>,
    kind: BackupKind,
    media: &BTreeMap<Uuid, PathBuf>,
    limits: Limits,
) -> Result<Manifest, SnapshotError> {
    state.validate().map_err(|_| SnapshotError::Corrupt)?;
    if destination.exists()
        || (kind == BackupKind::Metadata && !media.is_empty())
        || (kind == BackupKind::Full
            && (media.len() != state.tracks.len()
                || media.keys().any(|id| !state.tracks.contains_key(id))))
    {
        return Err(SnapshotError::Conflict);
    }
    if media
        .len()
        .checked_add(4)
        .is_none_or(|n| n > limits.entries)
    {
        return Err(SnapshotError::Limit);
    }
    let state_bytes = canonical_json(state).map_err(|_| SnapshotError::Corrupt)?;
    if state_bytes.len() as u64 > MAX_UNCOMPRESSED {
        return Err(SnapshotError::Limit);
    }
    let compressed = zstd::stream::encode_all(state_bytes.as_slice(), 3)?;
    if compressed.len() as u64 > MAX_COMPRESSED {
        return Err(SnapshotError::Limit);
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut builder = tar::Builder::new(&mut temporary);
    let mut entries = BTreeMap::new();
    entries.insert(
        "state.json.zst".into(),
        append(
            &mut builder,
            "state.json.zst",
            compressed.len() as u64,
            compressed.as_slice(),
        )?,
    );
    entries.insert(
        "README.txt".into(),
        append(
            &mut builder,
            "README.txt",
            README.len() as u64,
            README.as_bytes(),
        )?,
    );
    let mut total = compressed.len() as u64 + README.len() as u64;
    for (track, path) in media {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > limits.media_file_bytes {
            return Err(SnapshotError::Limit);
        }
        total = total
            .checked_add(metadata.len())
            .ok_or(SnapshotError::Limit)?;
        if total > limits.total_bytes {
            return Err(SnapshotError::Limit);
        }
        let name = format!("media/{track}");
        let checksum = append(&mut builder, &name, metadata.len(), file)?;
        if !state.tracks[track].sources.values().any(|s| {
            s.sha256.as_ref() == Some(&checksum.sha256)
                && s.size_bytes.is_none_or(|size| size == checksum.bytes)
        }) {
            return Err(SnapshotError::Corrupt);
        }
        entries.insert(name, checksum);
    }
    let manifest = Manifest {
        format: "kiyora.tmv-backup".into(),
        protocol: 1,
        state_schema: 1,
        kind,
        vault_id: state.vault_id,
        owner_id: owner,
        generation: state.generation,
        cursor: state.last_event_message_id,
        barrier_message_id: barrier,
        track_count: state.tracks.len(),
        playlist_count: state.playlists.len(),
        entries,
    };
    let manifest_bytes = canonical_json(&manifest).map_err(|_| SnapshotError::Corrupt)?;
    if manifest_bytes.len() > 4 * 1024 * 1024 {
        return Err(SnapshotError::Limit);
    }
    let manifest_sum = append(
        &mut builder,
        "manifest.json",
        manifest_bytes.len() as u64,
        manifest_bytes.as_slice(),
    )?;
    let checksums = checksums_text(&manifest.entries, &manifest_sum);
    if checksums.len() > 4 * 1024 * 1024
        || total + manifest_bytes.len() as u64 + checksums.len() as u64 > limits.total_bytes
    {
        return Err(SnapshotError::Limit);
    }
    append(
        &mut builder,
        "checksums.sha256",
        checksums.len() as u64,
        checksums.as_bytes(),
    )?;
    builder.finish()?;
    drop(builder);
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(destination)
        .map_err(|e| SnapshotError::Io(e.error))?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(manifest)
}
fn checksums_text(entries: &BTreeMap<String, Checksum>, manifest: &Checksum) -> String {
    let mut sums = entries.clone();
    sums.insert("manifest.json".into(), manifest.clone());
    sums.into_iter()
        .map(|(name, sum)| format!("{}  {name}\n", sum.sha256))
        .collect()
}
fn validate_name(name: &str) -> Result<Option<Uuid>, SnapshotError> {
    match name {
        "manifest.json" | "state.json.zst" | "checksums.sha256" | "README.txt" => Ok(None),
        _ => {
            let suffix = name.strip_prefix("media/").ok_or(SnapshotError::Corrupt)?;
            let uuid: Uuid = suffix.parse().map_err(|_| SnapshotError::Corrupt)?;
            if uuid.is_nil() || suffix != uuid.to_string() {
                return Err(SnapshotError::Corrupt);
            }
            Ok(Some(uuid))
        }
    }
}

/// Validate and stage files under generated temporary names. Archive paths are
/// never joined to a destination, and symlinks/hardlinks/devices/sparse/PAX fail.
pub fn inspect(path: &Path, limits: Limits) -> Result<VerifiedBackup, SnapshotError> {
    let file = File::open(path)?;
    let mut archive = tar::Archive::new(file);
    let staging = tempfile::tempdir()?;
    let mut entries = BTreeMap::new();
    let mut metadata = BTreeMap::new();
    let mut media = BTreeMap::new();
    let mut total = 0u64;
    for (index, entry) in archive.entries()?.raw(true).enumerate() {
        if index >= limits.entries {
            return Err(SnapshotError::Limit);
        }
        let mut entry = entry?;
        if entry.header().entry_type() != tar::EntryType::Regular || entry.link_name()?.is_some() {
            return Err(SnapshotError::Corrupt);
        }
        let name =
            String::from_utf8(entry.path_bytes().to_vec()).map_err(|_| SnapshotError::Corrupt)?;
        let track = validate_name(&name)?;
        if entries.contains_key(&name) {
            return Err(SnapshotError::Corrupt);
        }
        let size = entry.size();
        let cap = match name.as_str() {
            "state.json.zst" => MAX_COMPRESSED,
            "README.txt" => 16 * 1024,
            "manifest.json" | "checksums.sha256" => 4 * 1024 * 1024,
            _ => limits.media_file_bytes,
        };
        total = total.checked_add(size).ok_or(SnapshotError::Limit)?;
        if size > cap || total > limits.total_bytes {
            return Err(SnapshotError::Limit);
        }
        let mut hashing = HashingReader {
            reader: &mut entry,
            hasher: Sha256::new(),
            bytes: 0,
        };
        if let Some(track) = track {
            let destination = staging.path().join(index.to_string());
            let mut output = File::create(&destination)?;
            std::io::copy(&mut hashing, &mut output)?;
            output.sync_all()?;
            media.insert(track, destination);
        } else {
            let mut bytes = Vec::new();
            hashing.read_to_end(&mut bytes)?;
            metadata.insert(name.clone(), bytes);
        }
        if hashing.bytes != size {
            return Err(SnapshotError::Corrupt);
        }
        entries.insert(
            name,
            Checksum {
                bytes: size,
                sha256: hex::encode(hashing.hasher.finalize()),
            },
        );
    }
    let manifest_bytes = metadata
        .remove("manifest.json")
        .ok_or(SnapshotError::Corrupt)?;
    let manifest: Manifest =
        serde_json::from_value(strict_json(&manifest_bytes).map_err(|_| SnapshotError::Corrupt)?)
            .map_err(|_| SnapshotError::Corrupt)?;
    if manifest.format != "kiyora.tmv-backup"
        || manifest.protocol != 1
        || manifest.state_schema != 1
        || manifest.vault_id.is_nil()
    {
        return Err(SnapshotError::Corrupt);
    }
    let checksum_bytes = metadata
        .remove("checksums.sha256")
        .ok_or(SnapshotError::Corrupt)?;
    entries.remove("checksums.sha256");
    let manifest_sum = entries
        .remove("manifest.json")
        .ok_or(SnapshotError::Corrupt)?;
    if entries != manifest.entries
        || checksums_text(&entries, &manifest_sum).as_bytes() != checksum_bytes
    {
        return Err(SnapshotError::Corrupt);
    }
    if !metadata.contains_key("README.txt") {
        return Err(SnapshotError::Corrupt);
    }
    let compressed = metadata
        .remove("state.json.zst")
        .ok_or(SnapshotError::Corrupt)?;
    let mut decoder = zstd::stream::read::Decoder::new(compressed.as_slice())
        .map_err(|_| SnapshotError::Corrupt)?;
    decoder.window_log_max(27)?;
    let mut json = Vec::new();
    decoder.take(MAX_UNCOMPRESSED + 1).read_to_end(&mut json)?;
    if json.len() as u64 > MAX_UNCOMPRESSED {
        return Err(SnapshotError::Limit);
    }
    let state: State =
        serde_json::from_value(strict_json(&json).map_err(|_| SnapshotError::Corrupt)?)
            .map_err(|_| SnapshotError::Corrupt)?;
    state.validate().map_err(|_| SnapshotError::Corrupt)?;
    if state.vault_id != manifest.vault_id
        || state.generation != manifest.generation
        || state.last_event_message_id != manifest.cursor
        || state.tracks.len() != manifest.track_count
        || state.playlists.len() != manifest.playlist_count
    {
        return Err(SnapshotError::Corrupt);
    }
    match manifest.kind {
        BackupKind::Metadata if !media.is_empty() => return Err(SnapshotError::Corrupt),
        BackupKind::Full => {
            if media.len() != state.tracks.len() {
                return Err(SnapshotError::Corrupt);
            }
            for track in media.keys() {
                let track_state = state.tracks.get(track).ok_or(SnapshotError::Corrupt)?;
                let checksum = entries
                    .get(&format!("media/{track}"))
                    .ok_or(SnapshotError::Corrupt)?;
                if !track_state.sources.values().any(|s| {
                    s.sha256.as_ref() == Some(&checksum.sha256)
                        && s.size_bytes.is_none_or(|bytes| bytes == checksum.bytes)
                }) {
                    return Err(SnapshotError::Corrupt);
                }
            }
        }
        _ => {}
    }
    Ok(VerifiedBackup {
        manifest,
        state,
        media,
        _staging: staging,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreOperation {
    pub op_id: Uuid,
    pub entity_id: Uuid,
    pub action: Action,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestorePlan {
    pub source_vault_id: Uuid,
    pub target_vault_id: Uuid,
    pub operations: Vec<RestoreOperation>,
}
impl RestorePlan {
    pub fn hash(&self) -> Result<String, SnapshotError> {
        Ok(digest(
            &canonical_json(self).map_err(|_| SnapshotError::Corrupt)?,
        ))
    }
}

/// Recreate active objects with new UUIDs in an explicitly selected fresh vault.
/// Every track requires a confirmed new media reference supplied by the upload
/// service; stale Telegram ids in the archive are never reused automatically.
pub fn plan_new_vault(
    backup: &VerifiedBackup,
    new_vault: Uuid,
    new_chat: ChatId,
    sources: &BTreeMap<Uuid, Source>,
) -> Result<RestorePlan, SnapshotError> {
    if new_vault.is_nil()
        || new_vault == backup.state.vault_id
        || sources.len() != backup.state.tracks.len()
    {
        return Err(SnapshotError::Conflict);
    }
    let mut tracks = BTreeMap::new();
    let mut operations = Vec::new();
    let mut push = |entity_id, action| {
        operations.push(RestoreOperation {
            op_id: Uuid::new_v4(),
            entity_id,
            action,
        })
    };
    for (id, track) in &backup.state.tracks {
        let source = sources.get(id).ok_or(SnapshotError::Corrupt)?;
        if source.chat_id != new_chat
            || track
                .sources
                .values()
                .any(|old| old.chat_id == source.chat_id && old.message_id == source.message_id)
        {
            return Err(SnapshotError::Conflict);
        }
        let new_id = Uuid::new_v4();
        tracks.insert(*id, new_id);
        let mut metadata = track.metadata.clone();
        // Artwork remote references also belong to the old vault; keep its digest
        // but require the media service to attach a new reference separately.
        if let Some(artwork) = metadata.artwork.as_mut() {
            artwork.source = None;
        }
        push(
            new_id,
            Action::TrackRegister {
                metadata,
                source: source.clone(),
            },
        );
    }
    for (id, playlist) in &backup.state.playlists {
        let new_id = Uuid::new_v4();
        push(
            new_id,
            Action::PlaylistCreate {
                title: playlist.title.clone(),
            },
        );
        let mut after = None;
        for entry in backup
            .state
            .playlist_order(*id)
            .map_err(|_| SnapshotError::Corrupt)?
        {
            let entry = backup
                .state
                .entries
                .get(&entry)
                .ok_or(SnapshotError::Corrupt)?;
            let entry_id = Uuid::new_v4();
            push(
                new_id,
                Action::PlaylistTrackAdd {
                    track_id: *tracks.get(&entry.track_id).ok_or(SnapshotError::Corrupt)?,
                    entry_id,
                    after_entry_id: after,
                },
            );
            after = Some(entry_id);
        }
    }
    for (name, set) in &backup.state.favorites {
        for id in set {
            push(
                *tracks.get(id).ok_or(SnapshotError::Corrupt)?,
                Action::FavoriteSet {
                    set_name: name.clone(),
                    value: true,
                },
            );
        }
    }
    let mut settings = backup.state.settings.clone();
    settings.gc_device_id = None;
    push(new_vault, Action::VaultSettingsUpdate { settings });
    Ok(RestorePlan {
        source_vault_id: backup.state.vault_id,
        target_vault_id: new_vault,
        operations,
    })
}
