use crate::{ApiError, Context, DownloadInfo, SnapshotInfo, Status};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use uuid::Uuid;
use vault_db::Command;
use vault_telegram::AuthStatus;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "name", content = "payload")]
pub enum EventData {
    #[serde(rename = "vault.operation.progress")]
    Operation(vault_sync::OperationProgress),
    #[serde(rename = "playback.queue.changed")]
    Queue(crate::QueueInfo),
    #[serde(rename = "job.progress")]
    Job(crate::JobInfo),
    #[serde(rename = "runtime.mode.changed")]
    Runtime(Status),
    #[serde(rename = "auth.state.changed")]
    Auth(AuthStatus),
    #[serde(rename = "sync.progress")]
    Sync { running: bool },
    #[serde(rename = "library.changed")]
    Library { cursor: String },
    #[serde(rename = "playlist.changed")]
    Playlist { playlist_id: Uuid },
    #[serde(rename = "command.result")]
    Command(Box<Command>),
    #[serde(rename = "import.progress")]
    Import(Box<crate::ImportInfo>),
    #[serde(rename = "share.result")]
    Share {
        #[schemars(with = "String")]
        recipient_chat_id: vault_domain::ChatId,
        #[schemars(with = "String")]
        message_id: vault_domain::MessageId,
    },
    #[serde(rename = "download.progress")]
    Download(DownloadInfo),
    #[serde(rename = "playback.state")]
    Playback(vault_media::PlaybackState),
    #[serde(rename = "backup.restore.progress")]
    BackupRestore {
        phase: String,
        completed: usize,
        total: usize,
    },
    #[serde(rename = "snapshot.progress")]
    Snapshot {
        running: bool,
        result: Option<SnapshotInfo>,
    },
    #[serde(rename = "gc.progress")]
    Gc { running: bool, completed: usize },
    #[serde(rename = "vault.warning")]
    Warning(ApiError),
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BackendEvent {
    /// Decimal string avoids JavaScript precision loss across a long-lived process.
    pub sequence: String,
    pub context: Context,
    #[serde(flatten)]
    pub data: EventData,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResyncRequired {
    /// Last event actually delivered to this subscriber; None on first delivery.
    pub after_sequence: Option<String>,
    pub missed_events: String,
}

pub enum EventDelivery {
    Event(BackendEvent),
    ResyncRequired(ResyncRequired),
}

/// A loss notification is delivered directly, never reinserted into the full
/// broadcast queue (which would itself evict another event and loop forever).
pub struct EventSubscription {
    receiver: broadcast::Receiver<BackendEvent>,
    last_sequence: Option<String>,
}
impl EventSubscription {
    pub async fn recv(&mut self) -> Option<EventDelivery> {
        match self.receiver.recv().await {
            Ok(event) => {
                self.last_sequence = Some(event.sequence.clone());
                Some(EventDelivery::Event(event))
            }
            Err(broadcast::error::RecvError::Lagged(count)) => {
                Some(EventDelivery::ResyncRequired(ResyncRequired {
                    after_sequence: self.last_sequence.clone(),
                    missed_events: count.to_string(),
                }))
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}
#[derive(Clone)]
pub struct EventBus {
    sequence: Arc<Mutex<u64>>,
    sender: broadcast::Sender<BackendEvent>,
}
impl Default for EventBus {
    fn default() -> Self {
        let (sender, _) = broadcast::channel(1024);
        Self {
            sequence: Arc::new(Mutex::new(0)),
            sender,
        }
    }
}
impl EventBus {
    pub fn subscribe_reliable(&self) -> EventSubscription {
        EventSubscription {
            receiver: self.sender.subscribe(),
            last_sequence: None,
        }
    }
    pub fn sequence(&self) -> Result<String, ApiError> {
        self.sequence
            .lock()
            .map(|value| value.to_string())
            .map_err(|_| ApiError::new(crate::ErrorCode::Internal))
    }
    pub fn subscribe(&self) -> broadcast::Receiver<BackendEvent> {
        self.sender.subscribe()
    }
    pub fn emit(&self, context: &Context, data: EventData) {
        crate::telemetry::event(context, &data);
        // Serialize allocation and publication together, so concurrent producers
        // cannot publish sequence 2 before sequence 1.
        if let Ok(mut sequence) = self.sequence.lock()
            && let Some(next) = sequence.checked_add(1)
        {
            *sequence = next;
            let _ = self.sender.send(BackendEvent {
                sequence: next.to_string(),
                context: context.clone(),
                data,
            });
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn lag_is_explicit_and_delivery_continues_without_manufacturing_events() {
        let bus = EventBus::default();
        let mut stream = bus.subscribe_reliable();
        let context = Context {
            profile_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            vault_id: None,
        };
        bus.emit(&context, EventData::Sync { running: true });
        assert!(
            matches!(stream.recv().await, Some(EventDelivery::Event(event)) if event.sequence == "1")
        );
        for _ in 0..1030 {
            bus.emit(&context, EventData::Sync { running: false });
        }
        assert!(
            matches!(stream.recv().await, Some(EventDelivery::ResyncRequired(lost)) if lost.after_sequence.as_deref() == Some("1") && lost.missed_events == "6")
        );
        assert!(
            matches!(stream.recv().await, Some(EventDelivery::Event(event)) if event.sequence == "8")
        );
        assert_eq!(bus.sequence().unwrap(), "1031");
        drop(bus);
        for _ in 0..1023 {
            assert!(matches!(stream.recv().await, Some(EventDelivery::Event(_))));
        }
        assert!(stream.recv().await.is_none());
    }
}
