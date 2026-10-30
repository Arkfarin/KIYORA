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
fn notification_thumbnail_is_bounded_and_rejects_malformed_images() {
    let source = image::RgbImage::from_pixel(1024, 512, image::Rgb([20, 80, 120]));
    let mut encoded = std::io::Cursor::new(Vec::new());
    source
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes = vault_media::playback_thumbnail(encoded.get_ref()).unwrap();
    let thumbnail = image::load_from_memory(&bytes).unwrap();
    assert_eq!((thumbnail.width(), thumbnail.height()), (256, 128));
    assert!(bytes.len() <= 256 * 1024);
    assert!(vault_media::playback_thumbnail(b"not an image").is_err());
}

#[tokio::test(start_paused = true)]
async fn downloads_honor_telegram_flood_wait_before_retrying() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("tone.wav");
    wav(&path, 10);
    let server = FakeServer::new(ChatId::new(-100).unwrap(), UserId::new(42).unwrap());
    let telegram = server.client().unwrap();
    let cancel = CancellationToken::new();
    let message = telegram
        .send_document(
            ChatId::new(-100).unwrap(),
            path.clone(),
            String::new(),
            &cancel,
        )
        .await
        .unwrap();
    let file = message.file.unwrap();
    let cache = CacheManager::open(dir.path().join("cache"), 1024).unwrap();
    server
        .set_faults(vault_testkit::Faults {
            flood_wait: Some(5),
            ..Default::default()
        })
        .unwrap();
    let started = tokio::time::Instant::now();
    let lease = download_to_cache(
        &telegram,
        &file,
        &cache,
        DownloadOptions {
            cache_key: "flood-wait".into(),
            expected_sha256: None,
            pinned: false,
        },
        &cancel,
        &|_, _| {},
    )
    .await
    .unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_secs(5));
    assert_eq!(
        std::fs::read(lease.path.clone()).unwrap(),
        std::fs::read(path).unwrap()
    );
    drop(lease);
    server
        .set_faults(vault_testkit::Faults {
            flood_wait: Some(3600),
            ..Default::default()
        })
        .unwrap();
    let waiting = tokio::sync::Notify::new();
    let progress = |_, _| waiting.notify_one();
    let download = download_to_cache(
        &telegram,
        &file,
        &cache,
        DownloadOptions {
            cache_key: "cancel-flood-wait".into(),
            expected_sha256: None,
            pinned: false,
        },
        &cancel,
        &progress,
    );
    let (result, ()) = tokio::join!(download, async {
        waiting.notified().await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        cancel.cancel();
    });
    assert!(matches!(
        result,
        Err(MediaError::Telegram(
            vault_telegram::TelegramError::Cancelled
        ))
    ));
    assert_eq!(cache.stats().unwrap().reserved_bytes, 0);
}

#[test]
fn import_formats_match_available_decoders_and_missing_tags_use_the_filename() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/test-vectors/audio");
    for name in [
        "tone.mp3",
        "tone.flac",
        "tone.m4a",
        "tone.ogg",
        "tone.opus",
        "tone.aac",
        "tone.aiff",
        "tone-alac.m4a",
    ] {
        let result = inspect(&root.join(name));
        assert!(result.is_ok(), "{name}: {result:?}");
        let asset = result.unwrap();
        assert_eq!(
            asset.metadata.title,
            Path::new(name).file_stem().unwrap().to_str().unwrap()
        );
        assert!(
            (200..=400).contains(&asset.metadata.duration_ms),
            "{name}: {}",
            asset.metadata.duration_ms
        );
        assert_eq!(
            asset.sha256,
            hex::encode(Sha256::digest(std::fs::read(root.join(name)).unwrap()))
        );
    }
}

#[test]
fn rejects_symlink_unsupported_and_empty_audio_paths() {
    let dir = tempdir().unwrap();
    let wav_path = dir.path().join("good.wav");
    wav(&wav_path, 10);
    #[cfg(unix)]
    {
        let link = dir.path().join("alias.wav");
        std::os::unix::fs::symlink(&wav_path, &link).unwrap();
        assert!(matches!(inspect(&link), Err(MediaError::InvalidAudio)));
    }
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
            partial_entries: 0,
            partial_bytes: 0,
            reserved_bytes: 0,
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

#[test]
fn download_reservations_cannot_exceed_cache_quota() {
    let dir = tempdir().unwrap();
    let cache = CacheManager::open(dir.path().join("cache"), 100).unwrap();
    let _first = cache
        .prepare_partial_download("first", 1, 60, None)
        .unwrap();
    assert!(matches!(
        cache.prepare_partial_download("second", 2, 60, None),
        Err(MediaError::CacheFull)
    ));
}

#[test]
fn resizing_quota_protects_pins_leases_and_download_reservations() {
    let dir = tempdir().unwrap();
    let cache = CacheManager::open(dir.path().join("cache"), 100).unwrap();
    let path = dir.path().join("audio");
    std::fs::write(&path, [1u8; 40]).unwrap();
    let sha = hex::encode(Sha256::digest([1u8; 40]));
    let a = cache.insert("a".into(), &path, &sha, true).unwrap();
    let b = cache.insert("b".into(), &path, &sha, false).unwrap();
    assert!(matches!(cache.set_quota(60), Err(MediaError::CacheFull)));
    assert_eq!(cache.stats().unwrap().quota_bytes, 100);
    drop(b);
    assert_eq!(cache.set_quota(60).unwrap().used_bytes, 40);
    assert!(cache.acquire("b").unwrap().is_none());
    let partial = cache
        .prepare_partial_download("download", 1, 20, None)
        .unwrap();
    assert!(matches!(cache.set_quota(40), Err(MediaError::CacheFull)));
    assert_eq!(cache.stats().unwrap().quota_bytes, 60);
    drop(partial);
    assert_eq!(cache.set_quota(40).unwrap().quota_bytes, 40);
    drop(a);
    assert!(matches!(cache.set_quota(39), Err(MediaError::CacheFull)));
    assert!(cache.set_quota(0).is_err());
}

#[test]
fn partial_bytes_survive_reopen_and_count_toward_cache_quota() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("cache");
    let cache = CacheManager::open(&root, 2_000_000).unwrap();
    let partial = cache
        .prepare_partial_download("partial", 1, 1_500_000, None)
        .unwrap();
    std::fs::write(partial.path(), vec![7u8; 1_048_576]).unwrap();
    drop(partial);
    drop(cache);
    let cache = CacheManager::open(&root, 2_000_000).unwrap();
    assert_eq!(cache.used_bytes().unwrap(), 1_048_576);
    assert_eq!(cache.clear_unpinned().unwrap(), 1_048_576);
    assert_eq!(cache.used_bytes().unwrap(), 0);
    assert!(
        cache
            .partial_download_progress("partial")
            .unwrap()
            .is_none()
    );
}

#[test]
fn partial_writer_lease_blocks_replacement_and_cleanup_until_released() {
    let dir = tempdir().unwrap();
    let cache = CacheManager::open(dir.path().join("cache"), 100).unwrap();
    let partial = cache
        .prepare_partial_download("active", 1, 60, None)
        .unwrap();
    std::fs::write(partial.path(), [1u8; 20]).unwrap();
    let other = cache.clone();
    assert!(matches!(
        other.prepare_partial_download("active", 2, 80, None),
        Err(MediaError::CacheBusy)
    ));
    assert!(matches!(
        other.finish_partial_download("active"),
        Err(MediaError::CacheBusy)
    ));
    assert!(matches!(
        other.insert(
            "active".into(),
            partial.path(),
            &hex::encode(Sha256::digest([1u8; 20])),
            false
        ),
        Err(MediaError::CacheBusy)
    ));
    assert_eq!(other.clear_unpinned().unwrap(), 0);
    assert_eq!(std::fs::read(partial.path()).unwrap(), [1u8; 20]);
    let stats = cache.stats().unwrap();
    assert_eq!(stats.partial_entries, 1);
    assert_eq!(stats.partial_bytes, 20);
    assert_eq!(stats.used_bytes, 20);
    assert_eq!(stats.reserved_bytes, 40);
    drop(partial);
    assert_eq!(other.stats().unwrap().reserved_bytes, 0);
    assert_eq!(other.clear_unpinned().unwrap(), 20);
    assert_eq!(other.stats().unwrap().partial_entries, 0);
}

#[test]
fn download_reservation_evicts_only_unused_unpinned_files() {
    let dir = tempdir().unwrap();
    let cache = CacheManager::open(dir.path().join("cache"), 100).unwrap();
    let source = dir.path().join("source");
    std::fs::write(&source, [1u8; 40]).unwrap();
    let hash = hex::encode(Sha256::digest([1u8; 40]));
    let pinned = cache.insert("pinned".into(), &source, &hash, true).unwrap();
    let unpinned = cache
        .insert("unused".into(), &source, &hash, false)
        .unwrap();
    assert!(matches!(
        cache.prepare_partial_download("next", 1, 60, None),
        Err(MediaError::CacheFull)
    ));
    assert!(cache.acquire("unused").unwrap().is_some());
    drop(unpinned);
    let partial = cache.prepare_partial_download("next", 1, 60, None).unwrap();
    assert!(pinned.path.exists());
    assert!(cache.acquire("unused").unwrap().is_none());
    assert!(matches!(
        cache.insert("third".into(), &source, &hash, false),
        Err(MediaError::CacheFull)
    ));
    std::fs::write(partial.path(), [3u8; 60]).unwrap();
    let lease = partial
        .publish(&hex::encode(Sha256::digest([3u8; 60])), false)
        .unwrap();
    assert_eq!(std::fs::read(&lease.path).unwrap(), [3u8; 60]);
    let stats = cache.stats().unwrap();
    assert_eq!(stats.used_bytes, 100);
    assert_eq!(stats.reserved_bytes, 0);
    assert_eq!(stats.partial_entries, 0);
}

#[test]
fn incomplete_partial_cannot_be_published_even_with_matching_digest() {
    let dir = tempdir().unwrap();
    let cache = CacheManager::open(dir.path().join("cache"), 100).unwrap();
    let partial = cache
        .prepare_partial_download("incomplete", 1, 60, None)
        .unwrap();
    std::fs::write(partial.path(), [1u8; 20]).unwrap();
    assert!(matches!(
        partial.publish(&hex::encode(Sha256::digest([1u8; 20])), false),
        Err(MediaError::Range)
    ));
    assert!(cache.acquire("incomplete").unwrap().is_none());
    assert_eq!(cache.stats().unwrap().reserved_bytes, 0);
    assert_eq!(cache.clear_unpinned().unwrap(), 20);
}

#[test]
fn failed_unpin_keeps_media_protected_in_memory_and_after_reopen() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("cache");
    let cache = CacheManager::open(&root, 100).unwrap();
    let source = dir.path().join("source");
    std::fs::write(&source, [7u8; 60]).unwrap();
    let digest = hex::encode(Sha256::digest([7u8; 60]));
    let lease = cache
        .insert("pinned".into(), &source, &digest, true)
        .unwrap();
    let media = lease.path.clone();
    drop(lease);
    std::fs::rename(root.join("index.json"), root.join("saved-index.json")).unwrap();
    std::fs::create_dir(root.join("index.json")).unwrap();
    assert!(cache.pin("pinned", false).is_err());
    assert!(cache.entries().unwrap()[0].pinned);
    std::fs::remove_dir(root.join("index.json")).unwrap();
    std::fs::rename(root.join("saved-index.json"), root.join("index.json")).unwrap();
    assert_eq!(cache.clear_unpinned().unwrap(), 0);
    assert!(media.is_file());
    drop(cache);
    let reopened = CacheManager::open(&root, 100).unwrap();
    assert!(reopened.entries().unwrap()[0].pinned);
    reopened.pin("pinned", false).unwrap();
    assert_eq!(reopened.clear_unpinned().unwrap(), 60);
}

#[test]
fn failed_publication_preserves_completed_partial_for_retry_after_reopen() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("cache");
    let cache = CacheManager::open(&root, 100).unwrap();
    let partial = cache
        .prepare_partial_download("retry", 1, 60, None)
        .unwrap();
    let bytes = [3u8; 60];
    let hash = hex::encode(Sha256::digest(bytes));
    std::fs::write(partial.path(), bytes).unwrap();
    let path = partial.path().to_owned();
    // Make the atomic index replacement fail after the media rename.
    std::fs::rename(root.join("index.json"), root.join("saved-index.json")).unwrap();
    std::fs::create_dir(root.join("index.json")).unwrap();
    assert!(partial.publish(&hash, false).is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    assert!(cache.entries().unwrap().is_empty());
    assert_eq!(cache.stats().unwrap().used_bytes, 60);
    assert_eq!(cache.stats().unwrap().reserved_bytes, 0);
    drop(cache);
    std::fs::remove_dir(root.join("index.json")).unwrap();
    std::fs::rename(root.join("saved-index.json"), root.join("index.json")).unwrap();
    let cache = CacheManager::open(&root, 100).unwrap();
    let partial = cache
        .prepare_partial_download("retry", 1, 60, None)
        .unwrap();
    assert_eq!(partial.offset(), 60);
    let complete = partial.publish(&hash, true).unwrap();
    assert_eq!(std::fs::read(&complete.path).unwrap(), bytes);
    assert_eq!(cache.stats().unwrap().partial_entries, 0);
}

#[test]
fn cache_reopen_removes_unpublished_media_and_owned_staging_files() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("cache");
    let cache = CacheManager::open(&root, 100).unwrap();
    let partial = cache
        .prepare_partial_download("crash", 1, 60, None)
        .unwrap();
    std::fs::write(partial.path(), [3u8; 60]).unwrap();
    let orphan = root.join(format!("{}.media", uuid::Uuid::new_v4()));
    // Simulate process loss between media rename and atomic index publication.
    std::fs::rename(partial.path(), &orphan).unwrap();
    drop(partial);
    drop(cache);
    let staging = root.join(".kiyora-cache-interrupted");
    std::fs::write(&staging, [9u8; 20]).unwrap();
    let unrelated = root.join("notes.txt");
    std::fs::write(&unrelated, "keep").unwrap();
    let cache = CacheManager::open(&root, 100).unwrap();
    assert!(!orphan.exists());
    assert!(!staging.exists());
    assert!(unrelated.exists());
    assert!(cache.acquire("crash").unwrap().is_none());
    assert_eq!(cache.stats().unwrap().partial_entries, 0);
    let retry = cache
        .prepare_partial_download("crash", 1, 60, None)
        .unwrap();
    assert_eq!(retry.offset(), 0);
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
    cache.pin("track:source", false).unwrap();

    assert!(matches!(
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
        .await,
        Err(MediaError::Range)
    ));
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
    let partial = cache
        .prepare_partial_download(
            "resume-track:source",
            i64::from(file.file_id),
            bytes.len() as u64,
            Some(&hex::encode(Sha256::digest(&bytes))),
        )
        .unwrap();
    let offset = partial.offset();
    assert_eq!(offset, 1024 * 1024);
    assert_eq!(std::fs::metadata(partial.path()).unwrap().len(), offset);
    assert_eq!(
        cache
            .partial_download_progress("resume-track:source")
            .unwrap(),
        Some((offset, bytes.len() as u64))
    );
    drop(partial);

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
