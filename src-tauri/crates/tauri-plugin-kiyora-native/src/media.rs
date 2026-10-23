use serde::Deserialize;
use std::time::{Duration, Instant};
use tauri::{Runtime, ipc::Channel, plugin::PluginHandle};
use uuid::Uuid;
use vault_media::{
    PlatformPlaybackSession, PlaybackControls, PlaybackError, PlaybackMetadata, PlaybackPlatform,
    PlaybackState,
};

pub struct AndroidMedia<R: Runtime>(PluginHandle<R>);
impl<R: Runtime> Clone for AndroidMedia<R> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<R: Runtime> AndroidMedia<R> {
    pub(crate) fn new(handle: PluginHandle<R>) -> Self {
        Self(handle)
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum NativeControl {
    Pause,
    Resume,
    Stop,
    Seek { position_ms: u64 },
}

struct MediaSession<R: Runtime> {
    handle: PluginHandle<R>,
    owner: Uuid,
    _controls: Channel<NativeControl>,
    last_update: Option<(Instant, u8)>,
}
impl<R: Runtime> MediaSession<R> {
    fn call(&self, command: &str, payload: serde_json::Value) -> Result<(), PlaybackError> {
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(
                Duration::from_secs(10),
                self.handle.run_mobile_plugin_async::<()>(command, payload),
            )
            .await
            .map_err(|_| PlaybackError::DeviceUnavailable)?
            .map_err(|_| PlaybackError::DeviceUnavailable)
        })
    }
}
impl<R: Runtime> PlaybackPlatform for AndroidMedia<R> {
    fn connect(
        &self,
        controls: PlaybackControls,
    ) -> Result<Box<dyn PlatformPlaybackSession>, PlaybackError> {
        // Locked Tao 0.37.1 initializes ndk-context during Activity.onCreate and
        // retains a global Application reference. Never initialize it a second
        // time here. Fail before CPAL if called outside that Android lifecycle.
        let context = std::panic::catch_unwind(ndk_context::android_context)
            .map_err(|_| PlaybackError::DeviceUnavailable)?;
        if context.vm().is_null() || context.context().is_null() {
            return Err(PlaybackError::DeviceUnavailable);
        }
        let channel = Channel::new(move |body| {
            let event: NativeControl = body.deserialize()?;
            let result = match event {
                NativeControl::Pause => controls.pause(),
                NativeControl::Resume => controls.resume(),
                NativeControl::Stop => controls.stop(),
                NativeControl::Seek { position_ms } => controls.seek(position_ms),
            };
            // A closed/full worker cannot accept the control; report failure to
            // Tauri instead of claiming that a native transport command applied.
            result.map_err(|_| control_error())
        });
        let session = MediaSession {
            handle: self.0.clone(),
            owner: Uuid::new_v4(),
            _controls: channel,
            last_update: None,
        };
        session.call(
            "openMediaSession",
            serde_json::json!({"owner": session.owner, "handler": session._controls}),
        )?;
        Ok(Box::new(session))
    }
}

fn control_error() -> tauri::Error {
    tauri::Error::Io(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "playback controls unavailable",
    ))
}

impl<R: Runtime> PlatformPlaybackSession for MediaSession<R> {
    fn prepare(&mut self, metadata: &PlaybackMetadata) -> Result<(), PlaybackError> {
        self.last_update = None;
        self.call(
            "preparePlayback",
            serde_json::json!({"owner": self.owner, "metadata": metadata}),
        )
    }
    fn pause(&mut self) -> Result<(), PlaybackError> {
        self.last_update = None;
        self.call(
            "pauseNativePlayback",
            serde_json::json!({"owner": self.owner}),
        )
    }
    fn resume(&mut self) -> Result<(), PlaybackError> {
        self.last_update = None;
        self.call(
            "resumeNativePlayback",
            serde_json::json!({"owner": self.owner}),
        )
    }
    fn update(&mut self, state: &PlaybackState) -> Result<(), PlaybackError> {
        if matches!(state, PlaybackState::Ended { .. }) {
            self.stop();
            return Ok(());
        }
        let kind = match state {
            PlaybackState::Idle | PlaybackState::Ended { .. } => 0,
            PlaybackState::Buffering { .. } => 1,
            PlaybackState::Playing { .. } => 2,
            PlaybackState::Paused { .. } => 3,
            PlaybackState::Failed { .. } => 4,
        };
        if self
            .last_update
            .as_ref()
            .is_some_and(|(time, last)| *last == kind && time.elapsed() < Duration::from_secs(1))
        {
            return Ok(());
        }
        self.call(
            "updateNativePlayback",
            serde_json::json!({"owner": self.owner, "state": state}),
        )?;
        self.last_update = Some((Instant::now(), kind));
        Ok(())
    }
    fn stop(&mut self) {
        self.last_update = None;
        let _ = self.call(
            "stopNativePlayback",
            serde_json::json!({"owner": self.owner}),
        );
    }
}
impl<R: Runtime> Drop for MediaSession<R> {
    fn drop(&mut self) {
        let _ = self.call(
            "closeMediaSession",
            serde_json::json!({"owner": self.owner}),
        );
    }
}
