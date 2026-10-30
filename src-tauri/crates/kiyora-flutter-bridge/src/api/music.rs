use super::models::Failure;
use crate::runtime::{id, lease};
use vault_core::ApiError;

#[derive(Clone, Debug)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub duration_ms: u64,
    pub year: u32,
    pub artwork_hash: Option<String>,
    pub has_lyrics: bool,
    pub updated_order: String,
}

#[derive(Clone, Debug)]
pub struct TrackPage {
    pub offset: u32,
    /// Absent only when the actual final page was returned.
    pub next_offset: Option<u32>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub artwork_hash: Option<String>,
    pub updated_order: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackPhase {
    Idle,
    Buffering,
    Playing,
    Paused,
    Ended,
    Failed,
}

#[derive(Clone, Debug)]
pub struct Playback {
    pub phase: PlaybackPhase,
    pub track_id: Option<String>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub failure_code: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    Off,
    All,
    One,
}

#[derive(Clone, Debug)]
pub struct Queue {
    pub id: String,
    pub revision: u64,
    pub total: u32,
    pub position: Option<u32>,
    pub current_track_id: Option<String>,
    pub shuffle: bool,
    pub repeat: Repeat,
}

#[derive(Clone, Debug)]
pub struct QueuePage {
    pub queue: Queue,
    pub offset: u32,
    pub track_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum Transport {
    Play,
    Pause,
    Next,
    Previous,
    Stop,
}

#[derive(Clone, Debug)]
pub struct LyricLine {
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
}

#[derive(Clone, Debug)]
pub struct Artwork {
    pub mime_type: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

pub async fn artwork(
    client_id: String,
    session_id: String,
    request_id: String,
    track_id: String,
) -> Result<Option<Artwork>, Failure> {
    let runtime = lease(&client_id).await?;
    let operation = runtime.operation(&request_id)?;
    Ok(runtime
        .backend
        .artwork(id(&session_id)?, id(&track_id)?, &operation.cancel)
        .await
        .map_err(Failure::from)?
        .map(|a| Artwork {
            mime_type: a.mime_type,
            sha256: a.sha256,
            bytes: a.bytes,
        }))
}

pub async fn lyrics(
    client_id: String,
    session_id: String,
    request_id: String,
    track_id: String,
) -> Result<Option<Lyrics>, Failure> {
    let runtime = lease(&client_id).await?;
    let operation = runtime.operation(&request_id)?;
    Ok(runtime
        .backend
        .lyrics(id(&session_id)?, id(&track_id)?, &operation.cancel)
        .await
        .map_err(Failure::from)?
        .map(|l| Lyrics {
            lines: l
                .lines
                .into_iter()
                .map(|l| LyricLine {
                    start_ms: l.start_ms,
                    end_ms: l.end_ms,
                    text: l.text,
                })
                .collect(),
        }))
}

/// Pages come from SQLite, never a truncated in-memory Dart library.
pub async fn library(
    client_id: String,
    session_id: String,
    query: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<TrackPage, Failure> {
    let runtime = lease(&client_id).await?;
    let limit = limit.clamp(1, 500);
    let tracks = runtime
        .backend
        .local_reader(id(&session_id)?)
        .await
        .map_err(Failure::from)?
        .library(query, offset, limit)
        .await
        .map_err(ApiError::from)
        .map_err(Failure::from)?;
    let next_offset = if tracks.len() == limit as usize {
        offset.checked_add(limit)
    } else {
        None
    };
    Ok(TrackPage {
        offset,
        next_offset,
        tracks: tracks.into_iter().map(Into::into).collect(),
    })
}

pub async fn playlists(
    client_id: String,
    session_id: String,
    query: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<Vec<Playlist>, Failure> {
    let runtime = lease(&client_id).await?;
    Ok(runtime
        .backend
        .local_reader(id(&session_id)?)
        .await
        .map_err(Failure::from)?
        .playlists(query, offset, limit)
        .await
        .map_err(ApiError::from)
        .map_err(Failure::from)?
        .into_iter()
        .map(|p| Playlist {
            id: p.playlist_id.to_string(),
            title: p.title,
            artwork_hash: p.artwork.map(|a| a.sha256),
            updated_order: p.updated_order.to_string(),
        })
        .collect())
}

pub async fn playback(client_id: String, session_id: String) -> Result<Playback, Failure> {
    let runtime = lease(&client_id).await?;
    Ok(runtime
        .backend
        .playback_state(id(&session_id)?)
        .await
        .map_err(Failure::from)?
        .into())
}

pub async fn transport(
    client_id: String,
    session_id: String,
    action: Transport,
) -> Result<Playback, Failure> {
    let runtime = lease(&client_id).await?;
    use vault_core::PlaybackAction as A;
    let action = match action {
        Transport::Play => A::Resume,
        Transport::Pause => A::Pause,
        Transport::Next => A::Next,
        Transport::Previous => A::Previous,
        Transport::Stop => A::Stop,
    };
    Ok(runtime
        .backend
        .control_playback(id(&session_id)?, action)
        .await
        .map_err(Failure::from)?
        .into())
}

pub async fn seek(
    client_id: String,
    session_id: String,
    position_ms: u64,
) -> Result<Playback, Failure> {
    let runtime = lease(&client_id).await?;
    Ok(runtime
        .backend
        .control_playback(
            id(&session_id)?,
            vault_core::PlaybackAction::Seek(position_ms),
        )
        .await
        .map_err(Failure::from)?
        .into())
}

pub async fn queue(
    client_id: String,
    session_id: String,
    offset: u32,
    limit: u32,
) -> Result<QueuePage, Failure> {
    let runtime = lease(&client_id).await?;
    let page = runtime
        .backend
        .playback_queue(id(&session_id)?, offset, limit)
        .await
        .map_err(Failure::from)?;
    Ok(QueuePage {
        queue: page.info.into(),
        offset: page.offset,
        track_ids: page
            .track_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
    })
}

pub async fn play_tracks(
    client_id: String,
    session_id: String,
    track_ids: Vec<String>,
    position: u32,
) -> Result<Queue, Failure> {
    let runtime = lease(&client_id).await?;
    let tracks = track_ids
        .iter()
        .map(|value| id(value))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(runtime
        .backend
        .set_queue(id(&session_id)?, tracks, position, true)
        .await
        .map_err(Failure::from)?
        .into())
}

pub async fn play_playlist(
    client_id: String,
    session_id: String,
    playlist_id: String,
    position: u32,
) -> Result<Queue, Failure> {
    let runtime = lease(&client_id).await?;
    Ok(runtime
        .backend
        .queue_playlist(id(&session_id)?, id(&playlist_id)?, position, true)
        .await
        .map_err(Failure::from)?
        .into())
}

#[derive(Clone, Debug)]
pub struct QueueVersion {
    pub id: String,
    pub revision: u64,
}

async fn edit(
    client_id: String,
    session_id: String,
    version: QueueVersion,
    action: vault_core::QueueAction,
) -> Result<Queue, Failure> {
    let runtime = lease(&client_id).await?;
    Ok(runtime
        .backend
        .edit_queue(id(&session_id)?, id(&version.id)?, version.revision, action)
        .await
        .map_err(Failure::from)?
        .into())
}
pub async fn queue_append(
    client_id: String,
    session_id: String,
    version: QueueVersion,
    track_ids: Vec<String>,
    next: bool,
) -> Result<Queue, Failure> {
    let tracks = track_ids
        .iter()
        .map(|value| id(value))
        .collect::<Result<Vec<_>, _>>()?;
    edit(
        client_id,
        session_id,
        version,
        vault_core::QueueAction::Append {
            track_ids: tracks,
            next,
        },
    )
    .await
}
pub async fn queue_move(
    client_id: String,
    session_id: String,
    version: QueueVersion,
    from: u32,
    to: u32,
) -> Result<Queue, Failure> {
    edit(
        client_id,
        session_id,
        version,
        vault_core::QueueAction::Move {
            from: from as usize,
            to: to as usize,
        },
    )
    .await
}
pub async fn queue_remove(
    client_id: String,
    session_id: String,
    version: QueueVersion,
    index: u32,
) -> Result<Queue, Failure> {
    edit(
        client_id,
        session_id,
        version,
        vault_core::QueueAction::Remove {
            index: index as usize,
        },
    )
    .await
}
pub async fn queue_jump(
    client_id: String,
    session_id: String,
    version: QueueVersion,
    index: u32,
) -> Result<Queue, Failure> {
    edit(
        client_id,
        session_id,
        version,
        vault_core::QueueAction::Jump {
            index: index as usize,
        },
    )
    .await
}
pub async fn set_shuffle(
    client_id: String,
    session_id: String,
    version: QueueVersion,
    enabled: bool,
) -> Result<Queue, Failure> {
    edit(
        client_id,
        session_id,
        version,
        vault_core::QueueAction::Shuffle { enabled },
    )
    .await
}
pub async fn set_repeat(
    client_id: String,
    session_id: String,
    version: QueueVersion,
    mode: Repeat,
) -> Result<Queue, Failure> {
    edit(
        client_id,
        session_id,
        version,
        vault_core::QueueAction::Repeat {
            mode: match mode {
                Repeat::Off => vault_core::RepeatMode::Off,
                Repeat::All => vault_core::RepeatMode::Queue,
                Repeat::One => vault_core::RepeatMode::Track,
            },
        },
    )
    .await
}

impl From<vault_domain::Track> for Track {
    fn from(value: vault_domain::Track) -> Self {
        let m = value.metadata;
        Self {
            id: value.track_id.to_string(),
            title: m.title,
            artist: m.performer,
            album: m.album,
            album_artist: m.album_artist,
            duration_ms: m.duration_ms,
            year: m.year,
            artwork_hash: m.artwork.map(|a| a.sha256),
            has_lyrics: m.lyrics.is_some(),
            updated_order: value.updated_order.to_string(),
        }
    }
}
impl From<vault_core::QueueInfo> for Queue {
    fn from(value: vault_core::QueueInfo) -> Self {
        Self {
            id: value.queue_id.to_string(),
            revision: value.revision,
            total: value.total as u32,
            position: value.position.map(|p| p as u32),
            current_track_id: value.current_track_id.map(|id| id.to_string()),
            shuffle: value.shuffle,
            repeat: match value.repeat {
                vault_core::RepeatMode::Off => Repeat::Off,
                vault_core::RepeatMode::Queue => Repeat::All,
                vault_core::RepeatMode::Track => Repeat::One,
            },
        }
    }
}
impl From<vault_media::PlaybackState> for Playback {
    fn from(value: vault_media::PlaybackState) -> Self {
        use vault_media::PlaybackState as S;
        let (phase, track_id, position_ms, duration_ms, failure_code) = match value {
            S::Idle => (PlaybackPhase::Idle, None, 0, 0, None),
            S::Ended { track_id, .. } => (PlaybackPhase::Ended, Some(track_id), 0, 0, None),
            S::Buffering {
                track_id,
                position_ms,
                duration_ms,
            } => (
                PlaybackPhase::Buffering,
                Some(track_id),
                position_ms,
                duration_ms,
                None,
            ),
            S::Playing {
                track_id,
                position_ms,
                duration_ms,
            } => (
                PlaybackPhase::Playing,
                Some(track_id),
                position_ms,
                duration_ms,
                None,
            ),
            S::Paused {
                track_id,
                position_ms,
                duration_ms,
            } => (
                PlaybackPhase::Paused,
                Some(track_id),
                position_ms,
                duration_ms,
                None,
            ),
            S::Failed { track_id, code } => (PlaybackPhase::Failed, track_id, 0, 0, Some(code)),
        };
        Self {
            phase,
            track_id: track_id.map(|id| id.to_string()),
            position_ms,
            duration_ms,
            failure_code,
        }
    }
}
