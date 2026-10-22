use lofty::{config::ParseOptions, file::TaggedFileExt, prelude::*, probe::Probe, tag::ItemKey};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};
use vault_domain::{Artwork, Metadata};

pub const MAX_IMPORT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_ARTWORK_BYTES: usize = 4 * 1024 * 1024;
const MAX_TAGGED_INPUT: u64 = MAX_IMPORT_BYTES;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("unsupported or invalid audio file")]
    InvalidAudio,
    #[error("audio file exceeds the configured import limit")]
    TooLarge,
    #[error("audio file operation failed")]
    Io(#[from] std::io::Error),
    #[error("artwork format or size is unsupported")]
    Artwork,
    #[error("cache quota cannot be satisfied while files are pinned or in use")]
    CacheFull,
    #[error("cache index is invalid")]
    CacheIndex,
    #[error("download range was incomplete or inconsistent")]
    Range,
    #[error(transparent)]
    Telegram(#[from] vault_telegram::TelegramError),
}

#[derive(Debug, Clone)]
pub struct ImportAsset {
    pub path: PathBuf,
    pub filename: String,
    pub metadata: Metadata,
    pub sha256: String,
    pub size_bytes: u64,
    pub artwork: Option<ArtworkAsset>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkAsset {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub sha256: String,
}

/// Reads a supported local format, normalizes common tags, and hashes the complete
/// source in bounded memory. Symlinks and special files are refused.
pub fn inspect(path: &Path) -> Result<ImportAsset, MediaError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(MediaError::InvalidAudio);
    }
    let size = metadata.len();
    if size == 0 {
        return Err(MediaError::InvalidAudio);
    }
    if size > MAX_TAGGED_INPUT {
        return Err(MediaError::TooLarge);
    }
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "mp3" | "flac" | "m4a" | "mp4" | "aac" | "ogg" | "opus" | "wav" | "aiff" | "aif"
    ) {
        return Err(MediaError::InvalidAudio);
    }
    let tagged = Probe::open(path)
        .map_err(|_| MediaError::InvalidAudio)?
        .options(ParseOptions::new())
        .read()
        .map_err(|_| MediaError::InvalidAudio)?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("Untitled");
    let mut normalized = Metadata {
        title: stem.to_owned(),
        ..Metadata::default()
    };
    let mut artwork = None;
    if let Some(tag) = tag {
        normalized.title = tag
            .title()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| stem.to_owned().into())
            .into_owned();
        normalized.performer = tag.artist().unwrap_or_default().into_owned();
        normalized.album = tag.album().unwrap_or_default().into_owned();
        normalized.genre = tag.genre().unwrap_or_default().into_owned();
        normalized.album_artist = tag
            .get_string(ItemKey::AlbumArtist)
            .unwrap_or_default()
            .to_owned();
        normalized.track_number = tag
            .get_string(ItemKey::TrackNumber)
            .and_then(parse_number)
            .unwrap_or_default();
        normalized.disc_number = tag
            .get_string(ItemKey::DiscNumber)
            .and_then(parse_number)
            .unwrap_or_default();
        normalized.year = tag
            .get_string(ItemKey::Year)
            .or_else(|| tag.get_string(ItemKey::RecordingDate))
            .and_then(|s| s.get(..4))
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();
        if let Some(picture) = tag
            .pictures()
            .iter()
            .find(|p| p.data().len() <= MAX_ARTWORK_BYTES)
            && let Some(mime) = picture
                .mime_type()
                .map(ToString::to_string)
                .filter(|s| matches!(s.as_str(), "image/jpeg" | "image/png" | "image/webp"))
        {
            let bytes = picture.data().to_vec();
            let digest = hex::encode(Sha256::digest(&bytes));
            normalized.artwork = Some(Artwork {
                sha256: digest.clone(),
                mime_type: mime.clone(),
                source: None,
            });
            artwork = Some(ArtworkAsset {
                bytes,
                mime_type: mime,
                sha256: digest,
            });
        }
    }
    if normalized.title.len() > 1024
        || normalized.performer.len() > 1024
        || normalized.album.len() > 1024
        || normalized.album_artist.len() > 1024
        || normalized.genre.len() > 256
    {
        return Err(MediaError::InvalidAudio);
    }
    let props = tagged.properties();
    normalized.duration_ms = u64::try_from(props.duration().as_millis()).unwrap_or(u64::MAX);
    normalized.bitrate = props.audio_bitrate().unwrap_or(0);
    normalized.codec = extension;
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    let mut read_total = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        read_total = read_total
            .checked_add(count as u64)
            .ok_or(MediaError::TooLarge)?;
        if read_total > MAX_IMPORT_BYTES {
            return Err(MediaError::TooLarge);
        }
        hasher.update(&buffer[..count]);
    }
    if read_total != size {
        return Err(MediaError::InvalidAudio);
    }
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(MediaError::InvalidAudio)?
        .to_owned();
    if filename.len() > 240 || filename.contains(['\0', '/', '\\']) {
        return Err(MediaError::InvalidAudio);
    }
    Ok(ImportAsset {
        path: path.to_owned(),
        filename,
        metadata: normalized,
        sha256: hex::encode(hasher.finalize()),
        size_bytes: size,
        artwork,
    })
}
fn parse_number(value: &str) -> Option<u32> {
    value.split('/').next()?.trim().parse().ok()
}

/// Copies a selected import to a private staging file while recalculating its digest.
/// The caller can then upload from a stable file descriptor path.
pub fn stage(asset: &ImportAsset, root: &Path) -> Result<tempfile::NamedTempFile, MediaError> {
    let mut input = BufReader::new(File::open(&asset.path)?);
    let extension = asset
        .path
        .extension()
        .and_then(|v| v.to_str())
        .filter(|v| v.len() <= 8)
        .ok_or(MediaError::InvalidAudio)?;
    let mut out = tempfile::Builder::new()
        .suffix(&format!(".{extension}"))
        .tempfile_in(root)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or(MediaError::TooLarge)?;
        if total > MAX_IMPORT_BYTES {
            return Err(MediaError::TooLarge);
        }
        hasher.update(&buffer[..count]);
        use std::io::Write;
        out.write_all(&buffer[..count])?;
    }
    if total != asset.size_bytes || hex::encode(hasher.finalize()) != asset.sha256 {
        return Err(MediaError::InvalidAudio);
    }
    out.as_file().sync_all()?;
    Ok(out)
}
