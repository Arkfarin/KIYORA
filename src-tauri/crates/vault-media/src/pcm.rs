//! Decoder producer and nonblocking PCM consumer. Network, filesystem and codec
//! work stay on a dedicated thread, never inside Source::next on the mixer.
use crate::{CacheLease, PlaybackError, RemoteStream};
use rodio::Source;
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
mod opus;

const CHANNELS: u16 = 2;
const RATE: u32 = 48_000;
const BLOCK_SAMPLES: usize = 4096;
const BLOCKS: usize = 8;

pub enum PlaybackInput {
    Cached(CacheLease),
    Remote(RemoteStream),
}
trait InputRead: Read + Seek + Send + Sync {}
impl<T: Read + Seek + Send + Sync> InputRead for T {}
struct CheckedReader {
    input: Box<dyn InputRead>,
    failed: Arc<AtomicBool>,
}
impl Read for CheckedReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let result = self.input.read(bytes);
        if result.is_err() {
            self.failed.store(true, Ordering::Release);
        }
        result
    }
}
impl Seek for CheckedReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.input.seek(pos)
    }
}
// Vorbis can expose an empty initial decoded packet. Prime one sample before
// UniformSourceIterator inspects the span, otherwise it treats that packet as EOF.
struct PrimedDecoder {
    decoder: Box<dyn Source<Item = f32> + Send>,
    first: Option<f32>,
}
impl Iterator for PrimedDecoder {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.first.take().or_else(|| self.decoder.next())
    }
}
impl Source for PrimedDecoder {
    fn current_span_len(&self) -> Option<usize> {
        self.decoder.current_span_len()
    }
    fn channels(&self) -> u16 {
        self.decoder.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.decoder.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.decoder.total_duration()
    }
}
struct Queue {
    blocks: VecDeque<Vec<f32>>,
    seek: Option<u64>,
    cancel: CancellationToken,
    eof: bool,
    failure: Option<&'static str>,
}
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    stopped: AtomicBool,
    epoch: AtomicU64,
    position_samples: AtomicU64,
    buffering: AtomicBool,
    duration_ms: u64,
    lifetime: CancellationToken,
}
/// Owning controller. Drop cancels network reads and joins the decoder worker.
/// It must be dropped outside the OS audio callback.
pub(crate) struct PcmController {
    shared: Arc<Shared>,
    worker: Option<std::thread::JoinHandle<()>>,
}
pub(crate) struct PcmSource {
    shared: Arc<Shared>,
    current: std::vec::IntoIter<f32>,
    epoch: u64,
    silence_remaining: usize,
}
impl PcmController {
    pub fn start(
        input: PlaybackInput,
        duration_ms: u64,
    ) -> Result<(Self, PcmSource), PlaybackError> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                blocks: VecDeque::new(),
                seek: Some(0),
                cancel: CancellationToken::new(),
                eof: false,
                failure: None,
            }),
            wake: Condvar::new(),
            stopped: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            position_samples: AtomicU64::new(0),
            buffering: AtomicBool::new(true),
            duration_ms,
            lifetime: CancellationToken::new(),
        });
        let state = shared.clone();
        let worker = std::thread::Builder::new()
            .name("kiyora-decoder".into())
            .spawn(move || produce(input, state))
            .map_err(|_| PlaybackError::Closed)?;
        let source = PcmSource {
            shared: shared.clone(),
            current: Vec::new().into_iter(),
            epoch: 0,
            silence_remaining: 0,
        };
        Ok((
            Self {
                shared,
                worker: Some(worker),
            },
            source,
        ))
    }
    pub fn position_ms(&self) -> u64 {
        self.shared.position_samples.load(Ordering::Acquire) * 1000
            / (u64::from(RATE) * u64::from(CHANNELS))
    }
    pub fn buffering(&self) -> bool {
        self.shared.buffering.load(Ordering::Acquire)
    }
    pub fn failure(&self) -> Option<&'static str> {
        self.shared
            .queue
            .lock()
            .ok()
            .and_then(|queue| queue.failure)
    }
    pub fn seek(&self, position_ms: u64) -> Result<(), PlaybackError> {
        if self.shared.duration_ms != 0 && position_ms > self.shared.duration_ms {
            return Err(PlaybackError::Seek);
        }
        let samples = position_ms
            .checked_mul(u64::from(RATE) * u64::from(CHANNELS))
            .ok_or(PlaybackError::Seek)?
            / 1000;
        let mut queue = self
            .shared
            .queue
            .lock()
            .map_err(|_| PlaybackError::Closed)?;
        queue.cancel.cancel();
        queue.seek = Some(position_ms);
        queue.blocks.clear();
        queue.eof = false;
        queue.failure = None;
        self.shared
            .position_samples
            .store(samples, Ordering::Release);
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        self.shared.buffering.store(true, Ordering::Release);
        self.shared.wake.notify_all();
        Ok(())
    }
    pub fn stop(&self) {
        stop(&self.shared);
    }
}
fn stop(shared: &Shared) {
    shared.stopped.store(true, Ordering::Release);
    shared.lifetime.cancel();
    shared.wake.notify_all();
}
impl Drop for PcmController {
    fn drop(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Iterator for PcmSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.shared.stopped.load(Ordering::Acquire) {
            return None;
        }
        if self.silence_remaining != 0 {
            self.silence_remaining -= 1;
            return Some(0.0);
        }
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        if epoch != self.epoch {
            self.current = Vec::new().into_iter();
            self.epoch = epoch;
        }
        if self.current.len() == 0 {
            // The callback never waits for a producer lock or for data.
            let Ok(mut queue) = self.shared.queue.try_lock() else {
                self.silence_remaining = usize::from(CHANNELS) - 1;
                self.shared.buffering.store(true, Ordering::Release);
                return Some(0.0);
            };
            if let Some(block) = queue.blocks.pop_front() {
                self.current = block.into_iter();
                self.shared.wake.notify_one();
            } else if queue.eof || queue.failure.is_some() {
                return None;
            } else {
                self.shared.buffering.store(true, Ordering::Release);
                self.silence_remaining = usize::from(CHANNELS) - 1;
                return Some(0.0);
            }
        }
        let sample = self.current.next()?;
        self.shared.buffering.store(false, Ordering::Release);
        self.shared.position_samples.fetch_add(1, Ordering::AcqRel);
        Some(sample)
    }
}
impl Source for PcmSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        (self.shared.duration_ms != 0).then(|| Duration::from_millis(self.shared.duration_ms))
    }
}
impl Drop for PcmSource {
    fn drop(&mut self) {
        stop(&self.shared);
    }
}

fn produce(input: PlaybackInput, shared: Arc<Shared>) {
    let failed = Arc::new(AtomicBool::new(false));
    let mut decoder = None;
    loop {
        let (seek, cancel, epoch) = {
            let Ok(mut queue) = shared.queue.lock() else {
                return;
            };
            while !shared.stopped.load(Ordering::Acquire)
                && queue.seek.is_none()
                && (queue.blocks.len() >= BLOCKS || queue.eof || queue.failure.is_some())
            {
                queue = match shared.wake.wait(queue) {
                    Ok(queue) => queue,
                    Err(_) => return,
                };
            }
            if shared.stopped.load(Ordering::Acquire) {
                return;
            }
            let seek = queue.seek.take();
            if seek.is_some() {
                queue.cancel = shared.lifetime.child_token();
            }
            (
                seek,
                queue.cancel.clone(),
                shared.epoch.load(Ordering::Acquire),
            )
        };
        let result = (|| {
            if let Some(position_ms) = seek {
                failed.store(false, Ordering::Release);
                if position_ms != 0 && position_ms == shared.duration_ms {
                    decoder = None;
                    return Ok(Vec::new());
                }
                let (reader, size): (Box<dyn InputRead>, u64) = match &input {
                    PlaybackInput::Cached(lease) => (
                        Box::new(BufReader::new(
                            File::open(&lease.path).map_err(|_| "MEDIA_READ_ERROR")?,
                        )),
                        lease.entry.size_bytes,
                    ),
                    PlaybackInput::Remote(remote) => {
                        (Box::new(remote.reader(cancel.clone())), remote.size())
                    }
                };
                let mut reader = CheckedReader {
                    input: reader,
                    failed: failed.clone(),
                };
                let mut next: Box<dyn Source<Item = f32> + Send> =
                    if opus::is_opus(&mut reader).map_err(|_| "MEDIA_READ_ERROR")? {
                        Box::new(opus::OpusSource::new(reader, size, position_ms)?)
                    } else {
                        let mut next = rodio::Decoder::builder()
                            .with_data(reader)
                            .with_byte_len(size)
                            .with_seekable(true)
                            .build()
                            .map_err(|_| "DECODER_ERROR")?;
                        if position_ms != 0 {
                            next.try_seek(Duration::from_millis(position_ms))
                                .map_err(|_| "SEEK_ERROR")?;
                        }
                        Box::new(next)
                    };
                let first = next.next().ok_or("DECODER_ERROR")?;
                let next = PrimedDecoder {
                    decoder: next,
                    first: Some(first),
                };
                decoder = Some(rodio::source::UniformSourceIterator::new(
                    next, CHANNELS, RATE,
                ));
            }
            let decoder = decoder.as_mut().ok_or("DECODER_ERROR")?;
            let mut block = Vec::with_capacity(BLOCK_SAMPLES);
            for _ in 0..BLOCK_SAMPLES {
                if cancel.is_cancelled() {
                    break;
                }
                match decoder.next() {
                    Some(sample) => block.push(sample),
                    None => break,
                }
            }
            if failed.load(Ordering::Acquire) {
                return Err("STREAM_READ_ERROR");
            }
            Ok(block)
        })();
        let Ok(mut queue) = shared.queue.lock() else {
            return;
        };
        if epoch != shared.epoch.load(Ordering::Acquire) || cancel.is_cancelled() {
            continue;
        }
        match result {
            Ok(block) if block.is_empty() => queue.eof = true,
            Ok(block) => queue.blocks.push_back(block),
            Err(error) => {
                queue.failure = Some(if failed.load(Ordering::Acquire) {
                    "STREAM_READ_ERROR"
                } else {
                    error
                });
                queue.blocks.clear();
            }
        }
        shared.wake.notify_all();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    fn wav(frames: usize) -> Vec<u8> {
        let data: Vec<u8> = (0..frames).flat_map(|_| 8192i16.to_le_bytes()).collect();
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + data.len() as u32).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(48_000u32.to_le_bytes());
        bytes.extend(96_000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes
    }
    fn input() -> (tempfile::TempDir, crate::CacheManager, PlaybackInput) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        let bytes = wav(48_000);
        File::create(&path).unwrap().write_all(&bytes).unwrap();
        let cache = crate::CacheManager::open(dir.path().join("cache"), 200_000).unwrap();
        let lease = cache
            .insert(
                "tone".into(),
                &path,
                &hex::encode(Sha256::digest(&bytes)),
                false,
            )
            .unwrap();
        (dir, cache, PlaybackInput::Cached(lease))
    }
    fn wait_block(controller: &PcmController) {
        let queue = controller.shared.queue.lock().unwrap();
        let (queue, timeout) = controller
            .shared
            .wake
            .wait_timeout_while(queue, Duration::from_secs(3), |q| {
                q.blocks.is_empty() && q.failure.is_none() && !q.eof
            })
            .unwrap();
        assert!(!timeout.timed_out());
        assert!(queue.failure.is_none(), "{:?}", queue.failure);
    }
    fn next_audio(controller: &PcmController, source: &mut PcmSource) -> f32 {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let before = controller.shared.position_samples.load(Ordering::Acquire);
            let sample = source.next().unwrap_or_else(|| {
                panic!(
                    "early EOF: failure={:?}, consumed={before}",
                    controller.failure()
                )
            });
            if controller.shared.position_samples.load(Ordering::Acquire) != before {
                return sample;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "decoder did not supply PCM"
            );
            std::thread::yield_now();
        }
    }
    #[test]
    fn producer_decodes_real_audio_with_bounded_queue_and_retains_cache_lease() {
        let (_dir, cache, input) = input();
        let (controller, mut source) = PcmController::start(input, 1000).unwrap();
        wait_block(&controller);
        assert_eq!(next_audio(&controller, &mut source), 0.25);
        assert!(controller.shared.queue.lock().unwrap().blocks.len() <= BLOCKS);
        assert_eq!(cache.clear_unpinned().unwrap(), 0);
        drop(source);
        drop(controller);
        assert!(cache.clear_unpinned().unwrap() > 0);
    }
    #[test]
    fn seek_discards_buffered_audio_and_updates_playback_position() {
        let (_dir, _cache, input) = input();
        let (controller, mut source) = PcmController::start(input, 1000).unwrap();
        wait_block(&controller);
        assert_eq!(next_audio(&controller, &mut source), 0.25);
        controller.seek(500).unwrap();
        assert_eq!(controller.position_ms(), 500);
        wait_block(&controller);
        assert_eq!(next_audio(&controller, &mut source), 0.25);
        assert_eq!(source.epoch, 1);
        assert!(controller.seek(1001).is_err());
        controller.stop();
        assert!(source.next().is_none());
    }
    #[test]
    fn callback_does_not_wait_for_decoder_lock_or_advance_position_on_silence() {
        let (_dir, _cache, input) = input();
        let (controller, mut source) = PcmController::start(input, 1000).unwrap();
        let _queue = controller.shared.queue.lock().unwrap();
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(controller.position_ms(), 0);
    }
    async fn remote_input(
        bytes: &[u8],
    ) -> (tempfile::TempDir, vault_testkit::FakeServer, PlaybackInput) {
        use vault_telegram::Telegram;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream.audio");
        std::fs::write(&path, bytes).unwrap();
        let chat = vault_domain::ChatId::new(-100).unwrap();
        let server = vault_testkit::FakeServer::new(chat, vault_domain::UserId::new(42).unwrap());
        let telegram = server.client().unwrap();
        let file = telegram
            .send_document(chat, path, String::new(), &CancellationToken::new())
            .await
            .unwrap()
            .file
            .unwrap();
        let remote = crate::RemoteStream::new(telegram, file).unwrap();
        (dir, server, PlaybackInput::Remote(remote))
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_pcm_starts_before_complete_download_and_seeks_across_ranges() {
        let (_dir, server, input) = remote_input(&wav(48_000 * 30)).await;
        let (controller, mut source) = PcmController::start(input, 30_000).unwrap();
        wait_block(&controller);
        assert_eq!(next_audio(&controller, &mut source), 0.25);
        assert!(server.download_calls().unwrap() < 11);
        controller.seek(25_000).unwrap();
        wait_block(&controller);
        assert_eq!(next_audio(&controller, &mut source), 0.25);
        assert_eq!(controller.position_ms(), 25_000);
        controller.seek(1000).unwrap();
        wait_block(&controller);
        assert_eq!(next_audio(&controller, &mut source), 0.25);
        assert_eq!(controller.position_ms(), 1000);
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stopping_during_remote_probe_cancels_worker_without_waiting_for_network() {
        let (_dir, server, input) = remote_input(&wav(48_000 * 30)).await;
        server
            .set_faults(vault_testkit::Faults {
                download_delay_ms: 60_000,
                ..Default::default()
            })
            .unwrap();
        let (controller, mut source) = PcmController::start(input, 30_000).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.download_calls().unwrap() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(controller.position_ms(), 0);
        tokio::time::timeout(
            Duration::from_secs(2),
            tokio::task::spawn_blocking(move || drop(controller)),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(source.next().is_none());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn permanent_network_failure_is_reported_instead_of_clean_eof() {
        let (_dir, server, input) = remote_input(&wav(48_000)).await;
        server
            .set_faults(vault_testkit::Faults {
                offline: true,
                ..Default::default()
            })
            .unwrap();
        let (controller, mut source) = PcmController::start(input, 1000).unwrap();
        let queue = controller.shared.queue.lock().unwrap();
        let (queue, timeout) = controller
            .shared
            .wake
            .wait_timeout_while(queue, Duration::from_secs(3), |queue| {
                queue.failure.is_none()
            })
            .unwrap();
        assert!(!timeout.timed_out());
        assert!(queue.failure.is_some());
        assert!(!queue.eof);
        drop(queue);
        assert!(source.next().is_none());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_decoder_accepts_import_formats_and_seeks() {
        for name in [
            "tone.mp3",
            "tone.flac",
            "tone.m4a",
            "tone.ogg",
            "tone.aac",
            "tone.aiff",
            "tone-alac.m4a",
            "tone.opus",
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../spec/test-vectors/audio")
                .join(name);
            let bytes = std::fs::read(path).unwrap();
            let (_dir, _server, input) = remote_input(&bytes).await;
            let (controller, mut source) = PcmController::start(input, 250).unwrap();
            wait_block(&controller);
            // AAC may begin with encoder-delay silence; inspect beyond it.
            let samples: Vec<_> = (0..4096)
                .map(|_| next_audio(&controller, &mut source))
                .collect();
            assert!(samples.iter().any(|sample| sample.abs() > 0.001), "{name}");
            controller.seek(100).unwrap();
            wait_block(&controller);
            assert!(next_audio(&controller, &mut source).is_finite(), "{name}");
        }
    }
}
