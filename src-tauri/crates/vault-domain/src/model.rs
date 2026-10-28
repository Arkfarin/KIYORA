use crate::{ChatId, MessageId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub title: String,
    #[serde(default)]
    pub performer: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub album_artist: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub year: u32,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub track_number: u32,
    #[serde(default)]
    pub disc_number: u32,
    #[serde(default)]
    pub bitrate: u32,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub artwork: Option<Artwork>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub composer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub copyright: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lyrics: Option<LyricsReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Artwork {
    pub sha256: String,
    pub mime_type: String,
    pub source: Option<Source>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MetadataPatch {
    pub title: Option<String>,
    pub performer: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub duration_ms: Option<u64>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub artwork: Option<Artwork>,
    pub clear_artwork: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copyright: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lyrics: Option<LyricsReference>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clear_lyrics: bool,
    /// Optimistic concurrency is enforced during server-ordered replay on every device.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub expected_order: Option<MessageId>,
}

impl MetadataPatch {
    pub fn apply_to(&self, metadata: &mut Metadata) {
        macro_rules! patch {
            ($($field:ident),*) => { $(if let Some(value) = &self.$field { metadata.$field = value.clone(); })* };
        }
        patch!(
            title,
            performer,
            album,
            album_artist,
            duration_ms,
            year,
            genre,
            track_number,
            disc_number,
            composer,
            comment,
            copyright
        );
        if self.clear_lyrics {
            metadata.lyrics = None;
        }
        if let Some(lyrics) = &self.lyrics {
            metadata.lyrics = Some(lyrics.clone());
        }
        if self.clear_artwork {
            metadata.artwork = None;
        }
        if let Some(artwork) = &self.artwork {
            metadata.artwork = Some(artwork.clone());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub source_id: Uuid,
    #[schemars(with = "String")]
    pub chat_id: ChatId,
    #[schemars(with = "String")]
    pub message_id: MessageId,
    pub sha256: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", content = "payload", deny_unknown_fields)]
pub enum Action {
    #[serde(rename = "track.register")]
    TrackRegister { metadata: Metadata, source: Source },
    #[serde(rename = "track.metadata.update")]
    TrackMetadataUpdate { patch: MetadataPatch },
    #[serde(rename = "track.media.replace")]
    TrackMediaReplace {
        metadata: Metadata,
        source: Source,
        #[schemars(with = "String")]
        expected_order: MessageId,
    },
    #[serde(rename = "track.delete")]
    TrackDelete {},
    #[serde(rename = "playlist.create")]
    PlaylistCreate { title: String },
    #[serde(rename = "playlist.rename")]
    PlaylistRename { title: String },
    #[serde(rename = "playlist.metadata.update")]
    PlaylistMetadataUpdate { patch: PlaylistPatch },
    #[serde(rename = "playlist.delete")]
    PlaylistDelete {},
    #[serde(rename = "playlist.track.add")]
    PlaylistTrackAdd {
        track_id: Uuid,
        entry_id: Uuid,
        after_entry_id: Option<Uuid>,
    },
    #[serde(rename = "playlist.track.remove")]
    PlaylistTrackRemove { entry_id: Uuid },
    #[serde(rename = "playlist.entry.move")]
    PlaylistEntryMove {
        entry_id: Uuid,
        after_entry_id: Option<Uuid>,
    },
    #[serde(rename = "favorite.set")]
    FavoriteSet { set_name: String, value: bool },
    #[serde(rename = "source.relink")]
    SourceRelink { source: Source },
    #[serde(rename = "vault.settings.update")]
    VaultSettingsUpdate { settings: SharedSettings },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Self::TrackRegister { .. } => "track.register",
            Self::TrackMetadataUpdate { .. } => "track.metadata.update",
            Self::TrackMediaReplace { .. } => "track.media.replace",
            Self::TrackDelete {} => "track.delete",
            Self::PlaylistCreate { .. } => "playlist.create",
            Self::PlaylistRename { .. } => "playlist.rename",
            Self::PlaylistMetadataUpdate { .. } => "playlist.metadata.update",
            Self::PlaylistDelete {} => "playlist.delete",
            Self::PlaylistTrackAdd { .. } => "playlist.track.add",
            Self::PlaylistTrackRemove { .. } => "playlist.track.remove",
            Self::PlaylistEntryMove { .. } => "playlist.entry.move",
            Self::FavoriteSet { .. } => "favorite.set",
            Self::SourceRelink { .. } => "source.relink",
            Self::VaultSettingsUpdate { .. } => "vault.settings.update",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub vault_id: Uuid,
    pub op_id: Uuid,
    pub device_id: String,
    pub base_generation: u64,
    pub entity_id: Uuid,
    pub action: Action,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SharedSettings {
    pub gc_device_id: Option<String>,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub track_id: Uuid,
    pub metadata: Metadata,
    pub sources: BTreeMap<Uuid, Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_source_id: Option<Uuid>,
    #[schemars(with = "String")]
    pub updated_order: MessageId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Playlist {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork: Option<Artwork>,
    pub playlist_id: Uuid,
    pub title: String,
    #[schemars(with = "String")]
    pub created_order: MessageId,
    #[schemars(with = "String")]
    pub updated_order: MessageId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PlaylistPatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork: Option<Artwork>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clear_artwork: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub expected_order: Option<MessageId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaylistEntry {
    pub entry_id: Uuid,
    pub playlist_id: Uuid,
    pub track_id: Uuid,
    pub after_entry_id: Option<Uuid>,
    #[schemars(with = "String")]
    pub order_key: MessageId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tombstones {
    pub tracks: BTreeSet<Uuid>,
    pub playlists: BTreeSet<Uuid>,
    pub entries: BTreeSet<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub enum Outcome {
    Applied,
    Rejected(RejectReason),
    NoOp(NoOpReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    Deleted,
    MissingEntity,
    InvalidAnchor,
    InvalidPayload,
    VersionConflict,
    WrongEntity,
    FutureGeneration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoOpReason {
    AlreadyExists,
    AlreadyDeleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    #[schemars(with = "String")]
    pub message_id: MessageId,
    pub action: String,
    pub event_sha256: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub vault_id: Uuid,
    pub generation: u64,
    #[schemars(with = "Option<String>")]
    pub last_event_message_id: Option<MessageId>,
    pub tracks: BTreeMap<Uuid, Track>,
    pub playlists: BTreeMap<Uuid, Playlist>,
    pub entries: BTreeMap<Uuid, PlaylistEntry>,
    pub favorites: BTreeMap<String, BTreeSet<Uuid>>,
    pub tombstones: Tombstones,
    pub settings: SharedSettings,
    pub applied: BTreeMap<Uuid, Receipt>,
}

impl State {
    pub fn new(vault_id: Uuid) -> Self {
        Self {
            vault_id,
            generation: 0,
            last_event_message_id: None,
            tracks: BTreeMap::new(),
            playlists: BTreeMap::new(),
            entries: BTreeMap::new(),
            favorites: BTreeMap::new(),
            tombstones: Tombstones::default(),
            settings: SharedSettings::default(),
            applied: BTreeMap::new(),
        }
    }
}

/// A small document in the vault, fetched independently of the audio bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LyricsReference {
    pub sha256: String,
    pub source: Source,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LyricLine {
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LyricsDocument {
    pub version: u32,
    pub language: Option<String>,
    pub lines: Vec<LyricLine>,
}
impl LyricsDocument {
    pub fn valid(&self) -> bool {
        self.version == 1
            && self.lines.len() <= 2000
            && self
                .language
                .as_ref()
                .is_none_or(|s| s.len() <= 32 && !s.contains('\0'))
            && self.lines.iter().map(|l| l.text.len()).sum::<usize>() <= 128 * 1024
            && self.lines.iter().all(|l| {
                l.text.len() <= 4096
                    && !l.text.contains('\0')
                    && l.start_ms.is_none_or(|n| n <= 7 * 24 * 3600 * 1000)
                    && l.end_ms.is_none_or(|end| {
                        l.start_ms.is_some_and(|start| end >= start) && end <= 7 * 24 * 3600 * 1000
                    })
            })
            && (self.lines.iter().all(|l| l.start_ms.is_none())
                || self.lines.iter().all(|l| l.start_ms.is_some()))
            && self
                .lines
                .windows(2)
                .all(|w| w[0].start_ms <= w[1].start_ms)
    }
}

impl Track {
    pub fn media_sources(&self) -> impl Iterator<Item = &Source> {
        self.sources
            .values()
            .chain(
                self.metadata
                    .artwork
                    .as_ref()
                    .and_then(|a| a.source.as_ref()),
            )
            .chain(self.metadata.lyrics.as_ref().map(|l| &l.source))
    }
}

impl State {
    /// All live media dependencies, including non-track attachments.
    pub fn media_sources(&self) -> impl Iterator<Item = &Source> {
        self.tracks.values().flat_map(Track::media_sources).chain(
            self.playlists
                .values()
                .filter_map(|p| p.artwork.as_ref()?.source.as_ref()),
        )
    }
}
