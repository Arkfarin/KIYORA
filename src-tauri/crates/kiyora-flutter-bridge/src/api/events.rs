//! Demand-driven event delivery keeps backend backpressure observable. Flutter
//! wraps next_event in a Stream and explicitly closes it when the subscriber ends.
use super::{
    models::{Context, Failure},
    music::{Playback, Queue},
};
use crate::runtime::{failure, id, lease};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::{ErrorCode, EventData, EventDelivery, EventSubscription};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateKind {
    Resync,
    Runtime,
    Auth,
    Library,
    Queue,
    Playback,
    Jobs,
    Downloads,
    Maintenance,
    Warning,
}

#[derive(Clone, Debug)]
pub struct BackendUpdate {
    pub kind: UpdateKind,
    pub sequence: Option<String>,
    pub context: Option<Context>,
    pub playback: Option<Playback>,
    pub queue: Option<Queue>,
    pub failure: Option<Failure>,
    pub missed_events: Option<String>,
}

struct Subscription {
    client: Uuid,
    receiver: tokio::sync::Mutex<EventSubscription>,
    cancel: CancellationToken,
}
fn subscriptions() -> &'static Mutex<HashMap<Uuid, Arc<Subscription>>> {
    static SUBSCRIPTIONS: OnceLock<Mutex<HashMap<Uuid, Arc<Subscription>>>> = OnceLock::new();
    SUBSCRIPTIONS.get_or_init(Mutex::default)
}

/// Subscribe BEFORE reading state. Events after the snapshot's lower watermark
/// are idempotent invalidations, including changes concurrent with that read.
pub async fn open_events(client_id: String) -> Result<String, Failure> {
    let runtime = lease(&client_id).await?;
    let mut subscriptions = subscriptions()
        .lock()
        .map_err(|_| failure(ErrorCode::Internal))?;
    subscriptions.retain(|_, s| !s.cancel.is_cancelled());
    if subscriptions.len() >= 8 {
        return Err(failure(ErrorCode::Backpressure));
    }
    let token = Uuid::new_v4();
    subscriptions.insert(
        token,
        Arc::new(Subscription {
            client: runtime.id,
            receiver: tokio::sync::Mutex::new(runtime.backend.events().subscribe_reliable()),
            cancel: runtime.lifetime.child_token(),
        }),
    );
    Ok(token.to_string())
}

pub async fn next_event(
    client_id: String,
    subscription_id: String,
) -> Result<Option<BackendUpdate>, Failure> {
    let client = id(&client_id)?;
    let subscription = subscriptions()
        .lock()
        .map_err(|_| failure(ErrorCode::Internal))?
        .get(&id(&subscription_id)?)
        .filter(|s| s.client == client)
        .cloned()
        .ok_or_else(|| failure(ErrorCode::Closed))?;
    // A second pending read on one subscription is a caller bug, not a second consumer.
    let mut receiver = subscription
        .receiver
        .try_lock()
        .map_err(|_| failure(ErrorCode::Conflict))?;
    tokio::select! {
        biased;
        _ = subscription.cancel.cancelled() => Ok(None),
        event = receiver.recv() => Ok(event.map(convert)),
    }
}

pub async fn close_events(client_id: String, subscription_id: String) -> Result<(), Failure> {
    let client = id(&client_id)?;
    let token = id(&subscription_id)?;
    let mut subscriptions = subscriptions()
        .lock()
        .map_err(|_| failure(ErrorCode::Internal))?;
    if let Some(subscription) = subscriptions.get(&token) {
        if subscription.client != client {
            return Err(failure(ErrorCode::Closed));
        }
        subscription.cancel.cancel();
        subscriptions.remove(&token);
    }
    Ok(())
}

pub async fn event_watermark(client_id: String) -> Result<String, Failure> {
    lease(&client_id)
        .await?
        .backend
        .events()
        .sequence()
        .map_err(Into::into)
}

fn convert(delivery: EventDelivery) -> BackendUpdate {
    let mut result = BackendUpdate {
        kind: UpdateKind::Resync,
        sequence: None,
        context: None,
        playback: None,
        queue: None,
        failure: None,
        missed_events: None,
    };
    let event = match delivery {
        EventDelivery::ResyncRequired(lost) => {
            result.sequence = lost.after_sequence;
            result.missed_events = Some(lost.missed_events);
            return result;
        }
        EventDelivery::Event(event) => event,
    };
    result.sequence = Some(event.sequence);
    result.context = Some(event.context.into());
    result.kind = match event.data {
        EventData::Playback(state) => {
            result.playback = Some(state.into());
            UpdateKind::Playback
        }
        EventData::Queue(queue) => {
            result.queue = Some(queue.into());
            UpdateKind::Queue
        }
        EventData::Runtime(_) | EventData::Sync { .. } => UpdateKind::Runtime,
        EventData::Auth(_) => UpdateKind::Auth,
        EventData::Library { .. } | EventData::Playlist { .. } => UpdateKind::Library,
        EventData::Job(_)
        | EventData::Import(_)
        | EventData::Command(_)
        | EventData::Share { .. } => UpdateKind::Jobs,
        EventData::Download(_) => UpdateKind::Downloads,
        EventData::Operation(_)
        | EventData::BackupRestore { .. }
        | EventData::Snapshot { .. }
        | EventData::Gc { .. } => UpdateKind::Maintenance,
        EventData::Warning(error) => {
            result.failure = Some(error.into());
            UpdateKind::Warning
        }
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn loss_is_explicit_and_events_resume_in_order() -> Result<(), Box<dyn std::error::Error>>
    {
        let bus = vault_core::EventBus::default();
        let mut events = bus.subscribe_reliable();
        let context = vault_core::Context {
            profile_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            vault_id: None,
        };
        for _ in 0..1030 {
            bus.emit(&context, EventData::Sync { running: true });
        }
        let lost = convert(events.recv().await.ok_or("closed")?);
        assert_eq!(lost.kind, UpdateKind::Resync);
        assert_eq!(lost.missed_events.as_deref(), Some("6"));
        let next = convert(events.recv().await.ok_or("closed")?);
        assert_eq!(next.kind, UpdateKind::Runtime);
        assert_eq!(next.sequence.as_deref(), Some("7"));
        assert_eq!(
            next.context.ok_or("missing context")?.session_id,
            context.session_id.to_string()
        );
        Ok(())
    }
}
