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
            disc_number
        );
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
    #[serde(rename = "track.delete")]
    TrackDelete {},
    #[serde(rename = "playlist.create")]
    PlaylistCreate { title: String },
    #[serde(rename = "playlist.rename")]
    PlaylistRename { title: String },
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
            Self::TrackDelete {} => "track.delete",
            Self::PlaylistCreate { .. } => "playlist.create",
            Self::PlaylistRename { .. } => "playlist.rename",
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
    #[schemars(with = "String")]
    pub updated_order: MessageId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Playlist {
    pub playlist_id: Uuid,
    pub title: String,
    #[schemars(with = "String")]
    pub created_order: MessageId,
    #[schemars(with = "String")]
    pub updated_order: MessageId,
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
    WrongEntity,
    FutureGeneration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoOpReason {
    AlreadyExists,
    AlreadyDeleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub message_id: MessageId,
    pub action: String,
    pub event_sha256: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub vault_id: Uuid,
    pub generation: u64,
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
