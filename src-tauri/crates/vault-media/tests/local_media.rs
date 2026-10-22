#![allow(clippy::unwrap_used)]
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use vault_domain::{ChatId, UserId};
use vault_media::{
    CacheManager, CacheStats, DownloadOptions, MediaError, download_to_cache, inspect,
};
use vault_telegram::Telegram;
use vault_testkit::FakeServer;

fn wav(path: &Path, frames: u32) {
    let samples = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + samples).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(8000u32.to_le_bytes());
    bytes.extend(16000u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(samples.to_le_bytes());
    bytes.extend(vec![0u8; samples as usize]);
    std::fs::File::create(path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
}

#[test]
fn imports_probe_real_audio_metadata_and_hash_full_source() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sample.wav");
    wav(&path, 8000);
    let asset = inspect(&path).unwrap();
    assert_eq!(asset.filename, "sample.wav");
    assert_eq!(asset.size_bytes, 16044);
    assert_eq!(asset.metadata.codec, "wav");
    assert_eq!(asset.metadata.duration_ms, 1000);
    assert_eq!(
        asset.sha256,
        hex::encode(Sha256::digest(std::fs::read(path).unwrap()))
    );
}

#[test]
fn rejects_symlink_unsupported_and_empty_audio_paths() {
    let dir = tempdir().unwrap();
    let wav_path = dir.path().join("good.wav");
    wav(&wav_path, 10);
    let link = dir.path().join("alias.wav");
    std::os::unix::fs::symlink(&wav_path, &link).unwrap();
    assert!(matches!(inspect(&link), Err(MediaError::InvalidAudio)));
    let unsupported = dir.path().join("bad.txt");
    std::fs::write(&unsupported, b"not audio").unwrap();
    assert!(matches!(
        inspect(&unsupported),
        Err(MediaError::InvalidAudio)
    ));
    let empty = dir.path().join("empty.mp3");
    std::fs::write(&empty, []).unwrap();
    assert!(matches!(inspect(&empty), Err(MediaError::InvalidAudio)));
}

#[test]
fn cache_quota_respects_pins_and_active_leases_and_checks_content() {
    let dir = tempdir().unwrap();
    let cache = CacheManager::open(dir.path().join("cache"), 80).unwrap();
    let source_a = dir.path().join("a");
    let source_b = dir.path().join("b");
    let source_c = dir.path().join("c");
    std::fs::write(&source_a, [1u8; 40]).unwrap();
    std::fs::write(&source_b, [2u8; 40]).unwrap();
    std::fs::write(&source_c, [3u8; 40]).unwrap();
    let sha_a = hex::encode(Sha256::digest([1u8; 40]));
    let sha_b = hex::encode(Sha256::digest([2u8; 40]));
    let sha_c = hex::encode(Sha256::digest([3u8; 40]));
    let a = cache.insert("a".into(), &source_a, &sha_a, true).unwrap();
    let b = cache.insert("b".into(), &source_b, &sha_b, false).unwrap();
    assert_eq!(
        cache.stats().unwrap(),
        CacheStats {
            entries: 2,
            pinned_entries: 1,
            used_bytes: 80,
            quota_bytes: 80,
        }
    );
    assert!(matches!(
        cache.insert("c".into(), &source_c, &sha_c, false),
        Err(MediaError::CacheFull)
    ));
    drop(b);
    drop(a);
    let c = cache.insert("c".into(), &source_c, &sha_c, false).unwrap();
    assert!(cache.acquire("b").unwrap().is_none());
    assert!(cache.acquire("a").unwrap().is_some());
    std::fs::write(&c.path, b"tampered").unwrap();
    drop(c);
    assert!(matches!(cache.acquire("c"), Err(MediaError::CacheIndex)));
    assert_eq!(cache.clear_unpinned().unwrap(), 40);
}

#[test]
fn cache_lease_prevents_clear_until_decoder_releases_file() {
    let dir = tempdir().unwrap();
    let cache = CacheManager::open(dir.path().join("cache"), 100).unwrap();
    let path = dir.path().join("audio");
    std::fs::write(&path, b"audio bytes").unwrap();
    let hash = hex::encode(Sha256::digest(b"audio bytes"));
    let lease = cache.insert("track".into(), &path, &hash, false).unwrap();
    assert_eq!(cache.clear_unpinned().unwrap(), 0);
    assert!(lease.path.exists());
    drop(lease);
    assert_eq!(cache.clear_unpinned().unwrap(), 11);
    assert!(cache.acquire("track").unwrap().is_none());
}

#[tokio::test]
async fn ranged_download_hashes_and_publishes_only_verified_complete_media() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("remote.bin");
    let bytes = (0..1_600_000usize)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    std::fs::write(&source, &bytes).unwrap();
    let chat = ChatId::new(-100).unwrap();
    let server = FakeServer::new(chat, UserId::new(42).unwrap());
    let telegram = server.client().unwrap();
    let remote = telegram
        .send_document(chat, source, String::new(), &CancellationToken::new())
        .await
        .unwrap();
    let file = remote.file.unwrap();
    let cache = CacheManager::open(dir.path().join("cache"), 2_000_000).unwrap();
    let progress = std::sync::Mutex::new(Vec::new());
    let lease = download_to_cache(
        &telegram,
        &file,
        &cache,
        DownloadOptions {
            cache_key: "track:source".into(),
            expected_sha256: Some(hex::encode(Sha256::digest(&bytes))),
            pinned: true,
        },
        &CancellationToken::new(),
        &|done, total| progress.lock().unwrap().push((done, total)),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&lease.path).unwrap(), bytes);
    assert_eq!(lease.entry.size_bytes, 1_600_000);
    assert!(lease.entry.pinned);
    assert_eq!(
        progress.lock().unwrap().last(),
        Some(&(1_600_000, 1_600_000))
    );
    drop(lease);

    assert!(
        download_to_cache(
            &telegram,
            &file,
            &cache,
            DownloadOptions {
                cache_key: "bad-digest".into(),
                expected_sha256: Some("0".repeat(64)),
                pinned: false,
            },
            &CancellationToken::new(),
            &|_, _| {},
        )
        .await
        .is_err()
    );
    assert!(cache.acquire("bad-digest").unwrap().is_none());
}

#[tokio::test]
async fn cancelled_range_download_resumes_after_cache_manager_reopens() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("remote-large.bin");
    let bytes = (0..2_500_000usize)
        .map(|index| (index % 239) as u8)
        .collect::<Vec<_>>();
    std::fs::write(&source, &bytes).unwrap();
    let chat = ChatId::new(-101).unwrap();
    let server = FakeServer::new(chat, UserId::new(43).unwrap());
    let telegram = server.client().unwrap();
    let remote = telegram
        .send_document(chat, source, String::new(), &CancellationToken::new())
        .await
        .unwrap();
    let file = remote.file.unwrap();
    let cache_root = dir.path().join("cache");
    let cache = CacheManager::open(&cache_root, 3_000_000).unwrap();
    let cancel = CancellationToken::new();
    let cancel_on_progress = cancel.clone();
    let interrupted = download_to_cache(
        &telegram,
        &file,
        &cache,
        DownloadOptions {
            cache_key: "resume-track:source".into(),
            expected_sha256: Some(hex::encode(Sha256::digest(&bytes))),
            pinned: false,
        },
        &cancel,
        &move |downloaded, _| {
            if downloaded >= 1024 * 1024 {
                cancel_on_progress.cancel();
            }
        },
    )
    .await;
    assert!(matches!(
        interrupted,
        Err(MediaError::Telegram(
            vault_telegram::TelegramError::Cancelled
        ))
    ));
    drop(cache);

    let cache = CacheManager::open(&cache_root, 3_000_000).unwrap();
    let (partial, offset) = cache
        .prepare_partial_download(
            "resume-track:source",
            i64::from(file.file_id),
            bytes.len() as u64,
            Some(&hex::encode(Sha256::digest(&bytes))),
        )
        .unwrap();
    assert_eq!(offset, 1024 * 1024);
    assert_eq!(std::fs::metadata(partial).unwrap().len(), offset);
    assert_eq!(
        cache
            .partial_download_progress("resume-track:source")
            .unwrap(),
        Some((offset, bytes.len() as u64))
    );

    let resumed_progress = std::sync::Mutex::new(Vec::new());
    let lease = download_to_cache(
        &telegram,
        &file,
        &cache,
        DownloadOptions {
            cache_key: "resume-track:source".into(),
            expected_sha256: Some(hex::encode(Sha256::digest(&bytes))),
            pinned: false,
        },
        &CancellationToken::new(),
        &|downloaded, _| resumed_progress.lock().unwrap().push(downloaded),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&lease.path).unwrap(), bytes);
    assert_eq!(resumed_progress.lock().unwrap().first(), Some(&offset));
    assert_eq!(cache.entries().unwrap().len(), 1);
    assert!(std::fs::read_dir(&cache_root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("partial-")
    }));
}
