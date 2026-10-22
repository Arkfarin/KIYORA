#![allow(clippy::unwrap_used)]
use std::io::{Read, Seek, SeekFrom};
use tokio_util::sync::CancellationToken;
use vault_domain::{ChatId, UserId};
use vault_media::{RemoteStream, STREAM_CHUNK_BYTES};
use vault_telegram::Telegram;
use vault_testkit::{FakeServer, Faults};

async fn fixture() -> (tempfile::TempDir, FakeServer, RemoteStream, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let bytes: Vec<_> = (0..STREAM_CHUNK_BYTES * 10 + 73)
        .map(|n| (n % 251) as u8)
        .collect();
    let path = dir.path().join("remote.bin");
    std::fs::write(&path, &bytes).unwrap();
    let chat = ChatId::new(-100).unwrap();
    let server = FakeServer::new(chat, UserId::new(42).unwrap());
    let telegram = server.client().unwrap();
    let file = telegram
        .send_document(chat, path, String::new(), &CancellationToken::new())
        .await
        .unwrap()
        .file
        .unwrap();
    let stream = RemoteStream::new(telegram, file).unwrap();
    (dir, server, stream, bytes)
}
#[tokio::test]
async fn remote_reads_seek_both_directions_and_handle_eof_without_full_download() {
    let (_dir, server, stream, bytes) = fixture().await;
    tokio::task::spawn_blocking(move || {
        let mut reader = stream.reader(CancellationToken::new());
        let mut first = [0u8; 67];
        reader.read_exact(&mut first).unwrap();
        assert_eq!(first, bytes[..67]);
        let target = STREAM_CHUNK_BYTES as u64 * 8 + 9;
        reader.seek(SeekFrom::Start(target)).unwrap();
        reader.read_exact(&mut first).unwrap();
        assert_eq!(first, bytes[target as usize..target as usize + 67]);
        reader.seek(SeekFrom::Start(14)).unwrap();
        reader.read_exact(&mut first).unwrap();
        assert_eq!(first, bytes[14..81]);
        reader.seek(SeekFrom::End(-73)).unwrap();
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, bytes[bytes.len() - 73..]);
        assert!(reader.seek(SeekFrom::End(1)).is_err());
        assert!(reader.seek(SeekFrom::Current(i64::MIN)).is_err());
    })
    .await
    .unwrap();
    assert!(server.download_calls().unwrap() < 11);
}
#[tokio::test]
async fn cancelled_reader_does_not_wait_for_delayed_telegram_download() {
    let (_dir, server, stream, _) = fixture().await;
    server
        .set_faults(Faults {
            download_delay_ms: 60_000,
            ..Default::default()
        })
        .unwrap();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let read = tokio::task::spawn_blocking(move || {
        let mut reader = stream.reader(token);
        reader.read_exact(&mut [0u8; 10])
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while server.download_calls().unwrap() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), read)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn transient_range_failure_retries_at_the_same_position() {
    let (_dir, server, stream, bytes) = fixture().await;
    server
        .set_faults(Faults {
            fail_call: Some(1),
            ..Default::default()
        })
        .unwrap();
    let observed = tokio::task::spawn_blocking(move || {
        let mut reader = stream.reader(CancellationToken::new());
        reader.seek(SeekFrom::Start(97)).unwrap();
        let mut data = [0u8; 64];
        reader.read_exact(&mut data).unwrap();
        data
    })
    .await
    .unwrap();
    assert_eq!(observed, bytes[97..161]);
}

#[tokio::test]
async fn session_shutdown_cancels_prefetch_and_future_reads() {
    let (_dir, _server, stream, _) = fixture().await;
    let lifetime = CancellationToken::new();
    let stream = stream.with_lifetime(lifetime.clone());
    let (started, ready) = tokio::sync::oneshot::channel();
    let (resume, resumed) = std::sync::mpsc::channel();
    let read = tokio::task::spawn_blocking(move || {
        let mut reader = stream.reader(CancellationToken::new());
        reader.read_exact(&mut [0u8; 20]).unwrap();
        started.send(()).unwrap();
        resumed.recv().unwrap();
        reader.read_exact(&mut [0u8; 20])
    });
    ready.await.unwrap();
    lifetime.cancel();
    resume.send(()).unwrap();
    assert!(read.await.unwrap().is_err());
}
