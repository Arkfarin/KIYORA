//! Bounded, seekable remote input. Blocking reads belong on a decoder worker,
//! never on the Tokio executor or the OS audio callback.
use crate::{MAX_IMPORT_BYTES, MediaError};
use std::{
    future::Future,
    io::{self, Read, Seek, SeekFrom},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::sync::CancellationToken;
use vault_telegram::{RemoteFile, Telegram, TelegramError};

pub const STREAM_CHUNK_BYTES: usize = 256 * 1024;
type FetchFuture = Pin<Box<dyn Future<Output = Result<Vec<u8>, MediaError>> + Send>>;
type Fetch = dyn Fn(u64, usize, CancellationToken) -> FetchFuture + Send + Sync;

#[derive(Clone)]
pub struct RemoteStream {
    size: u64,
    fetch: Arc<Fetch>,
    runtime: tokio::runtime::Handle,
    lifetime: CancellationToken,
}
impl RemoteStream {
    /// Captures a live Tokio runtime; only the resulting reader's blocking worker
    /// waits for network data. At most the current and one prefetched chunk live.
    pub fn new<T: Telegram + Clone + 'static>(
        telegram: T,
        file: RemoteFile,
    ) -> Result<Self, MediaError> {
        let size = u64::try_from(file.size).map_err(|_| MediaError::Range)?;
        if file.file_id <= 0 || size == 0 || size > MAX_IMPORT_BYTES {
            return Err(MediaError::Range);
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| MediaError::Range)?;
        let fetch = Arc::new(
            move |offset: u64, limit: usize, cancel: CancellationToken| -> FetchFuture {
                let telegram = telegram.clone();
                let file = file.clone();
                Box::pin(async move { fetch_range(&telegram, &file, offset, limit, &cancel).await })
            },
        );
        Ok(Self {
            size,
            fetch,
            runtime,
            lifetime: CancellationToken::new(),
        })
    }
    pub fn with_lifetime(mut self, lifetime: CancellationToken) -> Self {
        self.lifetime = lifetime;
        self
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn reader(&self, cancel: CancellationToken) -> RemoteReader {
        RemoteReader {
            source: self.clone(),
            cancel,
            position: 0,
            chunk_start: 0,
            chunk: Vec::new(),
            prefetch: None,
        }
    }
}
struct Prefetch {
    offset: u64,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Result<Vec<u8>, MediaError>>,
}
impl Drop for Prefetch {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
pub struct RemoteReader {
    source: RemoteStream,
    cancel: CancellationToken,
    position: u64,
    chunk_start: u64,
    chunk: Vec<u8>,
    prefetch: Option<Prefetch>,
}
impl RemoteReader {
    fn request(&self, offset: u64) -> Prefetch {
        let cancel = self.source.lifetime.child_token();
        let limit = (self.source.size - offset).min(STREAM_CHUNK_BYTES as u64) as usize;
        let future = (self.source.fetch)(offset, limit, cancel.clone());
        Prefetch {
            offset,
            cancel,
            task: self.source.runtime.spawn(future),
        }
    }
    fn fill(&mut self) -> io::Result<()> {
        if self.cancel.is_cancelled() || self.source.lifetime.is_cancelled() {
            return Err(cancelled());
        }
        let offset = self.position / STREAM_CHUNK_BYTES as u64 * STREAM_CHUNK_BYTES as u64;
        let mut pending = match self.prefetch.take() {
            Some(pending) if pending.offset == offset => pending,
            _ => self.request(offset),
        };
        let result = self.source.runtime.block_on(async {
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => Err(cancelled()),
                _ = self.source.lifetime.cancelled() => Err(cancelled()),
                value = &mut pending.task => value.map_err(|_| io::Error::other("stream worker closed"))?
                    .map_err(|_| io::Error::other("stream range unavailable")),
            }
        });
        self.chunk = result?;
        self.chunk_start = offset;
        let next = offset + self.chunk.len() as u64;
        if next < self.source.size {
            self.prefetch = Some(self.request(next));
        }
        Ok(())
    }
}
impl Read for RemoteReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() || self.source.lifetime.is_cancelled() {
            return Err(cancelled());
        }
        if output.is_empty() || self.position == self.source.size {
            return Ok(0);
        }
        if self.position < self.chunk_start
            || self.position >= self.chunk_start + self.chunk.len() as u64
        {
            self.fill()?;
        }
        let index = (self.position - self.chunk_start) as usize;
        let count = output.len().min(self.chunk.len() - index);
        output[..count].copy_from_slice(&self.chunk[index..index + count]);
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for RemoteReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let next = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.source.size) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
        };
        if next < 0 || next > i128::from(self.source.size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek outside remote file",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}
fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "stream cancelled")
}

async fn fetch_range<T: Telegram>(
    telegram: &T,
    file: &RemoteFile,
    offset: u64,
    limit: usize,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, MediaError> {
    let mut last = TelegramError::Timeout;
    for attempt in 0..3 {
        let request_cancel = cancel.child_token();
        let request = telegram.download(file.clone(), offset as i64, limit as i64, &request_cancel);
        tokio::pin!(request);
        let range = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                request_cancel.cancel();
                let _ = request.await;
                return Err(TelegramError::Cancelled.into());
            },
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                request_cancel.cancel();
                let _ = request.await;
                Err(TelegramError::Timeout)
            },
            result = &mut request => result,
        };
        match range {
            Ok(range) => {
                if range.offset != offset as i64
                    || range.file_offset < 0
                    || range.file_offset > range.offset
                    || range.available < limit as i64
                {
                    return Err(MediaError::Range);
                }
                let local = (range.offset - range.file_offset) as u64;
                let metadata = tokio::fs::symlink_metadata(&range.path).await?;
                if !metadata.is_file()
                    || metadata.file_type().is_symlink()
                    || metadata.len() < local + limit as u64
                {
                    return Err(MediaError::Range);
                }
                let mut input = tokio::fs::File::open(range.path).await?;
                input.seek(SeekFrom::Start(local)).await?;
                let mut bytes = vec![0u8; limit];
                input.read_exact(&mut bytes).await?;
                return Ok(bytes);
            }
            Err(
                error @ (TelegramError::Timeout
                | TelegramError::Backpressure
                | TelegramError::Closed
                | TelegramError::RateLimited { .. }),
            ) => last = error,
            Err(error) => return Err(error.into()),
        }
        if attempt < 2 {
            let wait = match last {
                TelegramError::RateLimited {
                    retry_after_seconds,
                } => Duration::from_secs(retry_after_seconds),
                _ => Duration::from_millis(150 * (attempt + 1)),
            };
            tokio::select! { _ = cancel.cancelled() => return Err(TelegramError::Cancelled.into()), _ = tokio::time::sleep(wait) => {} }
        }
    }
    Err(last.into())
}
