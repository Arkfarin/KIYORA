//! Ogg/Opus adapter for the existing Rodio mixer. Decoding stays on the bounded
//! PCM producer. libopus is bundled; no external process or whole-file buffer.
use super::{CheckedReader, Source};
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use symphonia::core::{
    codecs::CODEC_TYPE_OPUS,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    probe::Hint,
};

const RATE: u32 = 48_000;
const MAX_PACKET_FRAMES: usize = 5760; // 120 ms at 48 kHz, RFC 6716.

pub(super) fn is_opus(reader: &mut CheckedReader) -> io::Result<bool> {
    let mut page = [0; 27];
    let detection = (|| {
        reader.read_exact(&mut page)?;
        if &page[..4] != b"OggS" {
            return Ok(false);
        }
        let mut lacing = [0; 255];
        reader.read_exact(&mut lacing[..usize::from(page[26])])?;
        let mut magic = [0; 8];
        reader.read_exact(&mut magic)?;
        Ok(&magic == b"OpusHead")
    })();
    reader.seek(SeekFrom::Start(0))?;
    detection
}

struct Input {
    reader: CheckedReader,
    size: u64,
}
impl Read for Input {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.reader.read(bytes)
    }
}
impl Seek for Input {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.reader.seek(pos)
    }
}
impl MediaSource for Input {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.size)
    }
}

pub(super) struct OpusSource {
    format: Box<dyn FormatReader>,
    decoder: ::opus::Decoder,
    track: u32,
    channels: u16,
    samples: Vec<f32>,
    offset: usize,
    end: usize,
    next_frame: Option<u64>,
    target: u64,
    last_frame: Option<u64>,
    duration: Option<Duration>,
    failed: Arc<AtomicBool>,
    eof: bool,
}

impl OpusSource {
    pub(super) fn new(
        reader: CheckedReader,
        size: u64,
        position_ms: u64,
    ) -> Result<Self, &'static str> {
        let failed = reader.failed.clone();
        let stream = MediaSourceStream::new(Box::new(Input { reader, size }), Default::default());
        let mut format = symphonia::default::get_probe()
            .format(
                &Hint::new(),
                stream,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &Default::default(),
            )
            .map_err(|_| "DECODER_ERROR")?
            .format;
        let track = format.default_track().ok_or("DECODER_ERROR")?;
        let params = &track.codec_params;
        if params.codec != CODEC_TYPE_OPUS || params.sample_rate != Some(RATE) {
            return Err("DECODER_ERROR");
        }
        let header = params.extra_data.as_deref().ok_or("DECODER_ERROR")?;
        if header.len() < 19 || &header[..8] != b"OpusHead" || header[8] > 15 || header[18] != 0 {
            return Err("UNSUPPORTED_OPUS_MAPPING");
        }
        let channels = match header[9] {
            1 => ::opus::Channels::Mono,
            2 => ::opus::Channels::Stereo,
            _ => return Err("UNSUPPORTED_OPUS_MAPPING"),
        };
        let pre_skip = u64::from(u16::from_le_bytes([header[10], header[11]]));
        let gain = i32::from(i16::from_le_bytes([header[16], header[17]]));
        let mut decoder = ::opus::Decoder::new(RATE, channels).map_err(|_| "DECODER_ERROR")?;
        decoder.set_gain(gain).map_err(|_| "DECODER_ERROR")?;
        let last_frame = params.n_frames;
        let duration = last_frame
            .and_then(|last| last.checked_sub(pre_skip))
            .map(|frames| Duration::from_secs_f64(frames as f64 / f64::from(RATE)));
        let track = track.id;
        let target = position_ms
            .checked_mul(48)
            .and_then(|frame| frame.checked_add(pre_skip))
            .ok_or("SEEK_ERROR")?;
        // RFC 7845 requires at least 80 ms of convergence. Use 250 ms for stereo
        // prediction. Decode short seeks from the beginning: Symphonia 0.5 gives
        // overlapping packet timestamps when the first audio page is also EOS.
        if position_ms > 1000 {
            format
                .seek(
                    SeekMode::Accurate,
                    SeekTo::TimeStamp {
                        ts: target.saturating_sub(12_000),
                        track_id: track,
                    },
                )
                .map_err(|_| "SEEK_ERROR")?;
        }
        Ok(Self {
            format,
            decoder,
            track,
            channels: channels as u16,
            samples: vec![0.; MAX_PACKET_FRAMES * channels as usize],
            offset: 0,
            end: 0,
            next_frame: None,
            target,
            last_frame,
            duration,
            failed,
            eof: false,
        })
    }

    fn fill(&mut self) -> Result<bool, &'static str> {
        // A corrupt/multiplexed stream cannot keep one next() call busy forever.
        for _ in 0..4096 {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::IoError(error))
                    if error.kind() == io::ErrorKind::UnexpectedEof =>
                {
                    if self
                        .last_frame
                        .is_some_and(|last| self.next_frame.is_none_or(|next| next < last))
                    {
                        return Err("TRUNCATED_OPUS");
                    }
                    return Ok(false);
                }
                Err(_) => return Err("OPUS_READ_ERROR"),
            };
            if packet.track_id() != self.track {
                continue;
            }
            if packet.data.is_empty() {
                return Err("OPUS_PACKET_ERROR");
            }
            let frames = self
                .decoder
                .decode_float(&packet.data, &mut self.samples, false)
                .map_err(|_| "OPUS_DECODE_ERROR")?;
            let start = self.next_frame.unwrap_or(packet.ts);
            self.next_frame = Some(
                start
                    .checked_add(frames as u64)
                    .ok_or("OPUS_PACKET_ERROR")?,
            );
            let skip = self.target.saturating_sub(start).min(frames as u64) as usize;
            let keep = self.last_frame.map_or(frames, |end| {
                end.saturating_sub(start).min(frames as u64) as usize
            });
            self.offset = skip * usize::from(self.channels);
            self.end = keep * usize::from(self.channels);
            if self.offset < self.end {
                return Ok(true);
            }
            if self
                .last_frame
                .is_some_and(|last| start + frames as u64 >= last)
            {
                return Ok(false);
            }
        }
        Err("OPUS_PACKET_LIMIT")
    }
}
impl Iterator for OpusSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.eof {
            return None;
        }
        if self.offset >= self.end {
            match self.fill() {
                Ok(true) => (),
                Ok(false) => {
                    self.eof = true;
                    return None;
                }
                Err(_) => {
                    self.failed.store(true, Ordering::Release);
                    self.eof = true;
                    return None;
                }
            }
        }
        let sample = self.samples[self.offset];
        self.offset += 1;
        Some(sample)
    }
}
impl Source for OpusSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn decode(name: &str, position: u64) -> (Vec<f32>, Arc<AtomicBool>) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../spec/test-vectors/audio")
            .join(name);
        let file = std::fs::File::open(path).unwrap();
        let size = file.metadata().unwrap().len();
        let failed = Arc::new(AtomicBool::new(false));
        let reader = CheckedReader {
            input: Box::new(file),
            failed: failed.clone(),
        };
        (
            OpusSource::new(reader, size, position).unwrap().collect(),
            failed,
        )
    }
    #[test]
    fn opus_preskip_end_trim_and_seek_preserve_sample_positions() {
        let (complete, failed) = decode("tone.opus", 0);
        assert!(!failed.load(Ordering::Acquire));
        assert_eq!(complete.len(), 12_000 * 2, "250 ms at 48 kHz, stereo");
        for position in [1, 80, 100, 200] {
            let (after_seek, failed) = decode("tone.opus", position);
            assert!(!failed.load(Ordering::Acquire));
            let expected = &complete[position as usize * 48 * 2..];
            assert_eq!(after_seek.len(), expected.len(), "seek {position}");
            assert!(
                after_seek
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| (actual - expected).abs() < 0.001),
                "seek {position}"
            );
        }
    }
    #[test]
    fn opus_seeks_across_pages_with_preroll_and_exact_duration() {
        let (complete, failed) = decode("tone-long.opus", 0);
        assert!(!failed.load(Ordering::Acquire));
        assert_eq!(complete.len(), 156_000 * 2);
        for position in [950, 1000, 1050, 2000, 3100] {
            let (actual, failed) = decode("tone-long.opus", position);
            assert!(!failed.load(Ordering::Acquire));
            let expected = &complete[position as usize * 96..];
            assert_eq!(actual.len(), expected.len(), "seek {position}");
            let max_error = actual
                .iter()
                .zip(expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max);
            assert!(max_error < 0.002, "seek {position}: {max_error}");
        }
    }
    #[test]
    fn truncated_opus_never_reports_a_complete_track() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../spec/test-vectors/audio/tone-long.opus");
        let complete = std::fs::read(path).unwrap();
        for length in [8, 100, complete.len() / 2, complete.len() - 100] {
            let failed = Arc::new(AtomicBool::new(false));
            let reader = CheckedReader {
                input: Box::new(io::Cursor::new(complete[..length].to_vec())),
                failed: failed.clone(),
            };
            if let Ok(source) = OpusSource::new(reader, complete.len() as u64, 0) {
                let decoded: Vec<_> = source.collect();
                assert!(failed.load(Ordering::Acquire) || decoded.len() < 156_000 * 2);
            }
        }
    }
}
