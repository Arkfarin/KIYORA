use crate::{CacheLease, MediaError};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::BufReader, path::Path, time::Duration};
use tokio::sync::watch;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("no usable system audio output device is available")]
    DeviceUnavailable,
    #[error("audio decoder rejected the selected file")]
    Decode,
    #[error("audio seek is unsupported or outside the stream")]
    Seek,
    #[error("playback worker is not available")]
    Closed,
    #[error(transparent)]
    Media(#[from] MediaError),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PlaybackState {
    Idle,
    Playing {
        track_id: Uuid,
        position_ms: u64,
        duration_ms: u64,
    },
    Paused {
        track_id: Uuid,
        position_ms: u64,
        duration_ms: u64,
    },
    Failed {
        track_id: Option<Uuid>,
        code: String,
    },
}

/// Opens a real OS output device and keeps the cache lease while its decoder reads.
/// CPAL output initialization is disabled on Android until Tauri supplies its AAudio
/// JavaVM/Activity context; it fails explicitly instead of panicking in native code.
pub struct PlaybackEngine {
    commands: std::sync::mpsc::SyncSender<Command>,
    state: watch::Receiver<PlaybackState>,
}
enum Command {
    Play {
        track: Uuid,
        lease: CacheLease,
        duration_ms: u64,
    },
    Pause,
    Resume,
    Stop,
    Seek(u64),
    Volume(f32),
    Shutdown,
}
impl PlaybackEngine {
    #[cfg(not(target_os = "android"))]
    pub fn open() -> Result<Self, PlaybackError> {
        let (tx, rx) = std::sync::mpsc::sync_channel(32);
        let (state_tx, state) = watch::channel(PlaybackState::Idle);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("kiyora-audio".into())
            .spawn(move || {
                let stream = match rodio::OutputStreamBuilder::open_default_stream() {
                    Ok(stream) => stream,
                    Err(_) => {
                        let _ = ready_tx.send(false);
                        state_tx.send_replace(PlaybackState::Failed {
                            track_id: None,
                            code: "AUDIO_DEVICE_UNAVAILABLE".into(),
                        });
                        return;
                    }
                };
                let _ = ready_tx.send(true);
                let mut sink: Option<rodio::Sink> = None;
                let mut active: Option<(Uuid, CacheLease, u64)> = None;
                let mut paused = false;
                let mut volume = 1.0f32;
                loop {
                    match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(Command::Play {
                            track,
                            lease,
                            duration_ms,
                        }) => {
                            if let Some(old) = sink.take() {
                                old.stop();
                            }
                            active = None;
                            match File::open(&lease.path)
                                .map(BufReader::new)
                                .map_err(|_| PlaybackError::Decode)
                                .and_then(|file| {
                                    rodio::Decoder::try_from(file)
                                        .map_err(|_| PlaybackError::Decode)
                                }) {
                                Ok(decoder) => {
                                    let next = rodio::Sink::connect_new(stream.mixer());
                                    next.set_volume(volume);
                                    next.append(decoder);
                                    next.play();
                                    sink = Some(next);
                                    active = Some((track, lease, duration_ms));
                                    paused = false;
                                }
                                Err(_) => {
                                    state_tx.send_replace(PlaybackState::Failed {
                                        track_id: Some(track),
                                        code: "DECODER_ERROR".into(),
                                    });
                                    continue;
                                }
                            }
                        }
                        Ok(Command::Pause) => {
                            if let Some(current) = &sink {
                                current.pause();
                                paused = true;
                            }
                        }
                        Ok(Command::Resume) => {
                            if let Some(current) = &sink {
                                current.play();
                                paused = false;
                            }
                        }
                        Ok(Command::Stop) => {
                            if let Some(current) = sink.take() {
                                current.stop();
                            }
                            active = None;
                            paused = false;
                            state_tx.send_replace(PlaybackState::Idle);
                        }
                        Ok(Command::Seek(position)) => {
                            if let Some(current) = &sink
                                && current.try_seek(Duration::from_millis(position)).is_err()
                                && let Some((track, _, _)) = &active
                            {
                                state_tx.send_replace(PlaybackState::Failed {
                                    track_id: Some(*track),
                                    code: "SEEK_ERROR".into(),
                                });
                            }
                        }
                        Ok(Command::Volume(value)) => {
                            volume = value;
                            if let Some(current) = &sink {
                                current.set_volume(volume);
                            }
                        }
                        Ok(Command::Shutdown)
                        | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if let (Some(current), Some((track, _, duration_ms))) = (&sink, &active) {
                        let position_ms =
                            u64::try_from(current.get_pos().as_millis()).unwrap_or(u64::MAX);
                        let value = if current.empty() {
                            PlaybackState::Idle
                        } else if paused {
                            PlaybackState::Paused {
                                track_id: *track,
                                position_ms,
                                duration_ms: *duration_ms,
                            }
                        } else {
                            PlaybackState::Playing {
                                track_id: *track,
                                position_ms,
                                duration_ms: *duration_ms,
                            }
                        };
                        state_tx.send_replace(value);
                        if current.empty() {
                            sink = None;
                            active = None;
                            paused = false;
                        }
                    }
                }
            })
            .map_err(|_| PlaybackError::DeviceUnavailable)?;
        if !ready_rx
            .recv()
            .map_err(|_| PlaybackError::DeviceUnavailable)?
        {
            return Err(PlaybackError::DeviceUnavailable);
        }
        Ok(Self {
            commands: tx,
            state,
        })
    }
    #[cfg(target_os = "android")]
    pub fn open() -> Result<Self, PlaybackError> {
        Err(PlaybackError::DeviceUnavailable)
    }
    pub fn state(&self) -> PlaybackState {
        self.state.borrow().clone()
    }
    pub fn state_changes(&self) -> watch::Receiver<PlaybackState> {
        self.state.clone()
    }
    pub fn play(
        &self,
        track: Uuid,
        lease: CacheLease,
        duration_ms: u64,
    ) -> Result<(), PlaybackError> {
        self.send(Command::Play {
            track,
            lease,
            duration_ms,
        })
    }
    pub fn pause(&self) -> Result<(), PlaybackError> {
        self.send(Command::Pause)
    }
    pub fn resume(&self) -> Result<(), PlaybackError> {
        self.send(Command::Resume)
    }
    pub fn stop(&self) -> Result<(), PlaybackError> {
        self.send(Command::Stop)
    }
    pub fn seek(&self, position_ms: u64) -> Result<(), PlaybackError> {
        self.send(Command::Seek(position_ms))
    }
    pub fn set_volume(&self, volume: f32) -> Result<(), PlaybackError> {
        if !volume.is_finite() {
            return Err(PlaybackError::Seek);
        }
        self.send(Command::Volume(volume.clamp(0.0, 1.0)))
    }
    pub fn shutdown(&self) {
        let _ = self.commands.try_send(Command::Shutdown);
    }
    fn send(&self, command: Command) -> Result<(), PlaybackError> {
        self.commands
            .try_send(command)
            .map_err(|_| PlaybackError::Closed)
    }
}
impl Drop for PlaybackEngine {
    fn drop(&mut self) {
        let _ = self.commands.try_send(Command::Shutdown);
    }
}

/// Decodes a selected local file through the configured format decoders without
/// opening an audio device, used for diagnostics and format validation.
pub fn decode_all(path: &Path) -> Result<(u64, u64), PlaybackError> {
    let decoder = rodio::Decoder::try_from(BufReader::new(
        File::open(path).map_err(|_| PlaybackError::Decode)?,
    ))
    .map_err(|_| PlaybackError::Decode)?;
    let duration = rodio::Source::total_duration(&decoder).ok_or(PlaybackError::Decode)?;
    let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    let count = decoder.count();
    if count == 0 {
        return Err(PlaybackError::Decode);
    }
    Ok((duration_ms, u64::try_from(count).unwrap_or(u64::MAX)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::expect_used)]
    fn decodes_pcm_wav_without_audio_hardware() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("tone.wav");
        let samples = [0i16; 4_800];
        let data_len = (samples.len() * 2) as u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&48_000u32.to_le_bytes());
        wav.extend_from_slice(&96_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, wav).expect("write wav");

        let (duration_ms, decoded_samples) = decode_all(&path).expect("decode wav");
        assert_eq!(duration_ms, 100);
        assert_eq!(decoded_samples, 4_800);
    }
}
