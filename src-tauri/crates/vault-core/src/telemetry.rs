//! Opt-in JSON diagnostics. Filter out third-party targets and never log DTOs.
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

pub fn initialize_diagnostics() {
    if std::env::var_os("KIYORA_DIAGNOSTICS").is_none_or(|value| value != "1") {
        return;
    }
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
            matches!(metadata.target(), "kiyora::backend" | "kiyora::auth")
        }));
    let _ = tracing_subscriber::registry().with(layer).try_init();
}

pub(crate) fn event(context: &crate::Context, data: &crate::EventData) {
    use crate::EventData::*;
    let name = match data {
        Operation(_) => "vault.operation.progress",
        Queue(_) => "playback.queue.changed",
        Job(_) => "job.progress",
        Runtime(_) => "runtime.mode.changed",
        Auth(_) => "auth.state.changed",
        Sync { .. } => "sync.progress",
        Library { .. } => "library.changed",
        Playlist { .. } => "playlist.changed",
        Command(_) => "command.result",
        Import(_) => "import.progress",
        Share { .. } => "share.result",
        Download(_) => "download.progress",
        Playback(_) => "playback.state",
        BackupRestore { .. } => "backup.restore.progress",
        Snapshot { .. } => "snapshot.progress",
        Gc { .. } => "gc.progress",
        Warning(_) => "vault.warning",
    };
    // Enum tags/UUIDs/counts only: no paths, titles, usernames, credentials or
    // QR links, even if their surrounding operation fails.
    tracing::info!(target: "kiyora::backend", event = name, profile = %context.profile_id, session = %context.session_id);
    match data {
        Auth(status) => tracing::info!(target: "kiyora::backend", state = status.name()),
        Warning(error) => {
            tracing::warn!(target: "kiyora::backend", code = ?error.code, retryable = error.retryable)
        }
        Command(command) => {
            tracing::info!(target: "kiyora::backend", operation = %command.intent.op_id, status = ?command.status)
        }
        Job(job) => {
            tracing::info!(target: "kiyora::backend", job = %job.job_id, state = ?job.state, completed = job.completed, total = job.total)
        }
        Runtime(status) => tracing::info!(target: "kiyora::backend", mode = ?status.mode),
        _ => {}
    }
}
