use kiyora_flutter_bridge::api::{lifecycle::*, models::*, music::*};
use std::path::Path;
use uuid::Uuid;
use vault_domain::{Action, ChatId, Event, MessageId, Metadata, Source, UserId};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub async fn fixture(root: &Path, count: u32) -> TestResult<(Initialization, Vec<String>)> {
    let options = InitOptions {
        data_directory: Some(root.to_string_lossy().into_owned()),
        resource_directory: None,
    };
    let init = initialize(options.clone()).await?;
    let profile = create_profile(init.client_id.clone(), "Bridge test".into()).await?;
    switch_profile(init.client_id.clone(), profile.id.clone()).await?;
    shutdown(init.client_id).await?;
    let directory = root.join("profiles").join(profile.id);
    let binding = vault_db::Binding {
        vault_id: Uuid::new_v4(),
        owner_id: UserId::new(42)?,
        chat_id: ChatId::new(-100)?,
    };
    let config = directory.join("backend.json");
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&config)?)?;
    value["binding"] = serde_json::to_value(binding)?;
    std::fs::write(config, serde_json::to_vec(&value)?)?;
    let path = root.join("silence.wav");
    let frames = 8000u32 * 120;
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
    std::fs::write(&path, wav)?;
    let media = vault_media::inspect(&path)?;
    let cache = vault_media::CacheManager::open(directory.join("cache"), 10_000_000)?;
    let mut records = Vec::new();
    let mut tracks = Vec::new();
    for i in 0..count {
        let track_id = Uuid::new_v4();
        let source_id = Uuid::new_v4();
        if i < 2 {
            drop(cache.insert(
                format!("{track_id}:{source_id}"),
                &path,
                &media.sha256,
                false,
            )?);
        }
        tracks.push(track_id.to_string());
        records.push((
            MessageId::new(i64::from(i) + 1)?,
            vault_protocol::Record::Event(Box::new(Event {
                vault_id: binding.vault_id,
                op_id: Uuid::new_v4(),
                device_id: "bridge-test".into(),
                base_generation: 0,
                entity_id: track_id,
                action: Action::TrackRegister {
                    metadata: Metadata {
                        title: format!("Track {i:05}"),
                        performer: "Test artist".into(),
                        duration_ms: 120_000,
                        ..Metadata::default()
                    },
                    source: Source {
                        source_id,
                        chat_id: binding.chat_id,
                        message_id: MessageId::new(100_000 + i64::from(i))?,
                        sha256: Some(media.sha256.clone()),
                        size_bytes: Some(std::fs::metadata(&path)?.len()),
                    },
                },
            })),
        ));
    }
    let mut db = vault_db::Database::open(directory.join("vault.sqlite"), binding)?;
    db.apply_records(&records)?;
    db.checkpoint()?;
    drop(db);
    drop(cache);
    Ok((initialize(options).await?, tracks))
}

