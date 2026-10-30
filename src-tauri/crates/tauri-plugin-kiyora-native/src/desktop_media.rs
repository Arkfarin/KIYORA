//! MPRIS / Windows SMTC publish the Rust player's state. No second audio engine.
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager, Runtime};
use uuid::Uuid;
use vault_media::{
    PlatformPlaybackSession, PlaybackControls, PlaybackError, PlaybackMetadata, PlaybackPlatform,
    PlaybackState,
};

pub struct DesktopMedia<R: Runtime>(AppHandle<R>);
impl<R: Runtime> DesktopMedia<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self(app)
    }
}

struct Session<R: Runtime> {
    app: AppHandle<R>,
    controls: PlaybackControls,
    media: Option<MediaControls>,
    metadata: Option<PlaybackMetadata>,
    state: Arc<Mutex<PlaybackState>>,
    thumbnail: Option<tempfile::NamedTempFile>,
    last_update: Option<(Instant, MediaPlayback)>,
}

impl<R: Runtime> PlaybackPlatform for DesktopMedia<R> {
    fn connect(
        &self,
        controls: PlaybackControls,
    ) -> Result<Box<dyn PlatformPlaybackSession>, PlaybackError> {
        Ok(Box::new(Session {
            app: self.0.clone(),
            controls,
            media: None,
            metadata: None,
            state: Arc::new(Mutex::new(PlaybackState::Idle)),
            thumbnail: None,
            last_update: None,
        }))
    }
}

fn position(state: &PlaybackState) -> (u64, u64) {
    match state {
        PlaybackState::Playing {
            position_ms,
            duration_ms,
            ..
        }
        | PlaybackState::Paused {
            position_ms,
            duration_ms,
            ..
        }
        | PlaybackState::Buffering {
            position_ms,
            duration_ms,
            ..
        } => (*position_ms, *duration_ms),
        _ => (0, 0),
    }
}
fn bounded_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
fn relative_seek(state: &PlaybackState, direction: SeekDirection, delta: Duration) -> u64 {
    let (at, duration) = position(state);
    let value = match direction {
        SeekDirection::Forward => at.saturating_add(bounded_millis(delta)),
        SeekDirection::Backward => at.saturating_sub(bounded_millis(delta)),
    };
    if duration == 0 {
        value
    } else {
        value.min(duration)
    }
}

impl<R: Runtime> Session<R> {
    fn attach(&mut self) -> Option<MediaControls> {
        #[cfg(target_os = "windows")]
        let hwnd = Some(self.app.get_webview_window("main")?.hwnd().ok()?.0.cast());
        #[cfg(not(target_os = "windows"))]
        let hwnd = None;
        let mut media = MediaControls::new(PlatformConfig {
            dbus_name: "kiyora",
            display_name: "KIYORA",
            hwnd,
        })
        .ok()?;
        let controls = self.controls.clone();
        let state = self.state.clone();
        let app = self.app.clone();
        media
            .attach(move |event| {
                let snapshot = state
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or(PlaybackState::Idle);
                let result = match event {
                    MediaControlEvent::Play => controls.resume(),
                    MediaControlEvent::Pause => controls.pause(),
                    MediaControlEvent::Toggle => {
                        if matches!(
                            snapshot,
                            PlaybackState::Playing { .. } | PlaybackState::Buffering { .. }
                        ) {
                            controls.pause()
                        } else {
                            controls.resume()
                        }
                    }
                    MediaControlEvent::Next => controls.next(),
                    MediaControlEvent::Previous => controls.previous(),
                    MediaControlEvent::Stop => controls.stop(),
                    MediaControlEvent::SetPosition(MediaPosition(value)) => {
                        controls.seek(bounded_millis(value).min(position(&snapshot).1))
                    }
                    MediaControlEvent::Seek(direction) => {
                        controls.seek(relative_seek(&snapshot, direction, Duration::from_secs(10)))
                    }
                    MediaControlEvent::SeekBy(direction, delta) => {
                        controls.seek(relative_seek(&snapshot, direction, delta))
                    }
                    MediaControlEvent::Raise => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                        Ok(())
                    }
                    // Volume remains the OS audio route's responsibility. URI opening
                    // and application exit are outside the transport interface.
                    _ => Ok(()),
                };
                if result.is_err() {
                    tracing::debug!("desktop media command could not be delivered");
                }
            })
            .ok()?;
        Some(media)
    }
    fn publish_metadata(&mut self) {
        let Some(metadata) = &self.metadata else {
            return;
        };
        let cover = self
            .thumbnail
            .as_ref()
            .and_then(|file| tauri::Url::from_file_path(file.path()).ok());
        if let Some(media) = &mut self.media {
            let _ = media.set_metadata(MediaMetadata {
                title: Some(&metadata.title),
                artist: Some(&metadata.performer),
                duration: Some(Duration::from_millis(
                    metadata.duration_ms.min(i64::MAX as u64 / 1000),
                )),
                cover_url: cover.as_ref().map(tauri::Url::as_str),
                ..Default::default()
            });
        }
    }
}
impl<R: Runtime> PlatformPlaybackSession for Session<R> {
    fn prepare(&mut self, metadata: &PlaybackMetadata) -> Result<(), PlaybackError> {
        self.thumbnail = None;
        self.metadata = Some(metadata.clone());
        self.last_update = None;
        if self.media.is_none() {
            self.media = self.attach();
        }
        // Desktop transport integration is optional; a missing session bus must
        // never prevent local audio. Android's audio-focus adapter remains strict.
        self.publish_metadata();
        Ok(())
    }
    fn pause(&mut self) -> Result<(), PlaybackError> {
        Ok(())
    }
    fn resume(&mut self) -> Result<(), PlaybackError> {
        Ok(())
    }
    fn update(&mut self, state: &PlaybackState) -> Result<(), PlaybackError> {
        if let Ok(mut snapshot) = self.state.lock() {
            *snapshot = state.clone();
        }
        if matches!(
            state,
            PlaybackState::Idle | PlaybackState::Ended { .. } | PlaybackState::Failed { .. }
        ) {
            self.stop();
            return Ok(());
        }
        let progress = Some(MediaPosition(Duration::from_millis(
            position(state).0.min(i64::MAX as u64 / 1000),
        )));
        let playback = if matches!(state, PlaybackState::Playing { .. }) {
            MediaPlayback::Playing { progress }
        } else {
            MediaPlayback::Paused { progress }
        };
        let same_kind = self.last_update.as_ref().is_some_and(|(time, last)| {
            std::mem::discriminant(last) == std::mem::discriminant(&playback)
                && time.elapsed() < Duration::from_secs(1)
        });
        if !same_kind {
            if let Some(media) = &mut self.media {
                let _ = media.set_playback(playback.clone());
            }
            self.last_update = Some((Instant::now(), playback));
        }
        Ok(())
    }
    fn stop(&mut self) {
        self.media = None;
        self.thumbnail = None;
        self.last_update = None;
    }
    fn artwork(&mut self, track: Uuid, bytes: &[u8]) -> Result<(), PlaybackError> {
        if self.metadata.as_ref().is_none_or(|m| m.track_id != track) || bytes.len() > 256 * 1024 {
            return Ok(());
        }
        // Use a private temporary file, with no profile paths or remote URLs.
        let file = tempfile::Builder::new()
            .prefix("kiyora-cover-")
            .suffix(".jpg")
            .tempfile();
        if let Ok(mut file) = file
            && file.write_all(bytes).and_then(|_| file.flush()).is_ok()
        {
            self.thumbnail = Some(file);
            self.publish_metadata();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_seek_is_saturated_and_clamped_to_the_current_track() {
        let state = PlaybackState::Paused {
            track_id: Uuid::nil(),
            position_ms: 50_000,
            duration_ms: 60_000,
        };
        assert_eq!(
            relative_seek(&state, SeekDirection::Backward, Duration::from_secs(90)),
            0
        );
        assert_eq!(
            relative_seek(&state, SeekDirection::Forward, Duration::MAX),
            60_000
        );
        assert_eq!(
            relative_seek(&state, SeekDirection::Forward, Duration::from_secs(3)),
            53_000
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires an isolated D-Bus session and a working audio output; plays digital silence"]
    #[allow(clippy::unwrap_used)]
    fn linux_mpris_controls_the_rust_engine() {
        use vault_media::{CacheManager, PlaybackEngine, PlaybackInput};
        fn wait(mut condition: impl FnMut() -> bool) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !condition() {
                assert!(Instant::now() < deadline, "native state did not converge");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        fn dbus(method: &str, args: &[&str]) -> String {
            let output = std::process::Command::new("gdbus")
                .args([
                    "call",
                    "--session",
                    "--timeout",
                    "3",
                    "--dest",
                    "org.mpris.MediaPlayer2.kiyora",
                    "--object-path",
                    "/org/mpris/MediaPlayer2",
                    "--method",
                    method,
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }
        let temp = tempfile::tempdir().unwrap();
        let frames: u32 = 8000 * 120;
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + frames * 2).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(8000u32.to_le_bytes());
        wav.extend(16000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((frames * 2).to_le_bytes());
        wav.resize(wav.len() + frames as usize * 2, 0);
        let path = temp.path().join("silence.wav");
        std::fs::write(&path, wav).unwrap();
        let source = vault_media::inspect(&path).unwrap();
        let cache = CacheManager::open(temp.path().join("cache"), 4_000_000).unwrap();
        let lease = cache
            .insert("native-test".into(), &path, &source.sha256, false)
            .unwrap();
        let app = tauri::test::mock_app();
        let mut engine =
            PlaybackEngine::open_with_platform(Arc::new(DesktopMedia::new(app.handle().clone())))
                .unwrap();
        let mut navigation = engine.take_navigation().unwrap();
        engine
            .play_input_with_metadata(
                PlaybackMetadata {
                    track_id: Uuid::new_v4(),
                    title: "KIYORA native test".into(),
                    performer: "Digital silence".into(),
                    duration_ms: 120_000,
                },
                PlaybackInput::Cached(lease),
            )
            .unwrap();
        wait(|| matches!(engine.state(), PlaybackState::Playing { .. }));
        let metadata = dbus(
            "org.freedesktop.DBus.Properties.Get",
            &["org.mpris.MediaPlayer2.Player", "Metadata"],
        );
        assert!(metadata.contains("KIYORA native test") && metadata.contains("Digital silence"));
        dbus("org.mpris.MediaPlayer2.Player.Pause", &[]);
        wait(|| matches!(engine.state(), PlaybackState::Paused { .. }));
        dbus(
            "org.mpris.MediaPlayer2.Player.SetPosition",
            &["/", "30000000"],
        );
        wait(
            || matches!(engine.state(), PlaybackState::Paused {position_ms, ..} if (29_000..=31_000).contains(&position_ms)),
        );
        dbus("org.mpris.MediaPlayer2.Player.Play", &[]);
        wait(|| matches!(engine.state(), PlaybackState::Playing { .. }));
        dbus("org.mpris.MediaPlayer2.Player.Next", &[]);
        wait(|| navigation.try_recv().is_ok_and(|forward| forward));
        dbus("org.mpris.MediaPlayer2.Player.Previous", &[]);
        wait(|| navigation.try_recv().is_ok_and(|forward| !forward));
        dbus("org.mpris.MediaPlayer2.Player.Stop", &[]);
        wait(|| matches!(engine.state(), PlaybackState::Idle));
    }
}
