//! Bounded artwork decoding and explicit tagging of a private copy only.
use crate::{ArtworkAsset, MAX_ARTWORK_BYTES, MediaError, inspect, lyrics_text};
use lofty::{
    config::WriteOptions,
    file::TaggedFileExt,
    picture::{Picture, PictureType},
    prelude::*,
    probe::Probe,
    tag::{ItemKey, Tag},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Cursor, path::Path};
use vault_domain::{LyricsDocument, Metadata};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub size: u32,
}
pub fn artwork(bytes: &[u8], crop: Option<Crop>) -> Result<ArtworkAsset, MediaError> {
    if bytes.is_empty() || bytes.len() > 8 * 1024 * 1024 {
        return Err(MediaError::Artwork);
    }
    let format = image::guess_format(bytes).map_err(|_| MediaError::Artwork)?;
    if !matches!(
        format,
        image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP
    ) {
        return Err(MediaError::Artwork);
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut image = reader.decode().map_err(|_| MediaError::Artwork)?;
    if let Some(c) = crop {
        if c.size == 0
            || c.x.checked_add(c.size).is_none_or(|n| n > image.width())
            || c.y.checked_add(c.size).is_none_or(|n| n > image.height())
        {
            return Err(MediaError::Artwork);
        }
        image = image.crop_imm(c.x, c.y, c.size, c.size);
    }
    image = image.resize(1600, 1600, image::imageops::FilterType::Lanczos3);
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|_| MediaError::Artwork)?;
    let mut bytes = output.into_inner();
    if bytes.len() > MAX_ARTWORK_BYTES {
        let mut smaller = Cursor::new(Vec::new());
        image
            .resize(1000, 1000, image::imageops::FilterType::Lanczos3)
            .write_to(&mut smaller, image::ImageFormat::Png)
            .map_err(|_| MediaError::Artwork)?;
        bytes = smaller.into_inner();
    }
    if bytes.len() > MAX_ARTWORK_BYTES {
        return Err(MediaError::Artwork);
    }
    Ok(ArtworkAsset {
        sha256: hex::encode(Sha256::digest(&bytes)),
        mime_type: "image/png".into(),
        bytes,
    })
}
/// Caller supplies an owned copy, never a user-selected original.
pub fn write_tags(
    copy: &Path,
    metadata: &Metadata,
    cover: Option<&ArtworkAsset>,
    lyrics: Option<&LyricsDocument>,
) -> Result<(), MediaError> {
    let mut file = Probe::open(copy)
        .map_err(|_| MediaError::InvalidAudio)?
        .guess_file_type()
        .map_err(|_| MediaError::InvalidAudio)?
        .read()
        .map_err(|_| MediaError::InvalidAudio)?;
    let kind = file.primary_tag_type();
    if !file.contains_tag_type(kind) {
        file.insert_tag(Tag::new(kind));
    }
    let tag = file.primary_tag_mut().ok_or(MediaError::InvalidAudio)?;
    tag.set_title(metadata.title.clone());
    tag.set_artist(metadata.performer.clone());
    tag.set_album(metadata.album.clone());
    tag.set_genre(metadata.genre.clone());
    tag.insert_text(ItemKey::Year, metadata.year.to_string());
    tag.set_track(metadata.track_number);
    tag.set_disk(metadata.disc_number);
    for (key, value) in [
        (ItemKey::AlbumArtist, &metadata.album_artist),
        (ItemKey::Composer, &metadata.composer),
        (ItemKey::Comment, &metadata.comment),
        (ItemKey::CopyrightMessage, &metadata.copyright),
    ] {
        tag.insert_text(key, value.clone());
    }
    while !tag.pictures().is_empty() {
        tag.remove_picture(0);
    }
    if let Some(cover) = cover {
        let mut picture = Picture::from_reader(&mut Cursor::new(&cover.bytes))
            .map_err(|_| MediaError::Artwork)?;
        picture.set_pic_type(PictureType::CoverFront);
        tag.push_picture(picture);
    }
    tag.remove_key(ItemKey::UnsyncLyrics);
    tag.remove_key(ItemKey::Lyrics);
    if let Some(lyrics) = lyrics {
        tag.insert_text(ItemKey::UnsyncLyrics, lyrics_text(lyrics));
    }
    file.save_to_path(copy, WriteOptions::default())
        .map_err(|_| MediaError::InvalidAudio)?;
    std::fs::File::open(copy)?.sync_all()?;
    let verified = inspect(copy)?;
    if verified.metadata.title != metadata.title
        || verified.metadata.performer != metadata.performer
    {
        return Err(MediaError::InvalidAudio);
    }
    Ok(())
}
