#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]
use std::process::Command;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::{ChatId, MessageId};
use vault_telegram::{AudioUpload, MessageContent, TdClient, TdTransport, Telegram, TelegramError};

#[tokio::test]
async fn copied_media_waits_for_final_ack_and_enforces_copy_permission() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("forwarding.so");
    assert!(
        Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&library)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/forwarding_fixture.c"
            ))
            .status()
            .unwrap()
            .success()
    );
    let client = TdClient::open(library).await.unwrap();
    let transport = TdTransport::new(client.clone());
    let source = ChatId::new(-100).unwrap();
    let target = ChatId::new(-200).unwrap();
    let cancel = CancellationToken::new();
    let copied = transport
        .forward_media(
            source,
            MessageId::new(1 << 20).unwrap(),
            Uuid::new_v4(),
            target,
            &cancel,
        )
        .await;
    assert!(
        copied.is_ok(),
        "copy must wait for updateMessageSendSucceeded: {copied:?}"
    );
    let copied = copied.unwrap();
    assert_eq!(copied.chat_id, target);
    assert_eq!(copied.id.get(), 10 << 20);
    assert_eq!(copied.file.unwrap().size, 42);
    assert!(
        copied.text.is_empty(),
        "private vault caption must not be shared"
    );

    // Forward permission alone does not authorize sending a copy.
    assert_eq!(
        transport
            .forward_media(
                source,
                MessageId::new(2 << 20).unwrap(),
                Uuid::new_v4(),
                target,
                &cancel
            )
            .await,
        Err(TelegramError::Unshareable)
    );
    // A permanent failure after a pending response must never be called sent.
    assert_eq!(
        transport
            .forward_media(
                source,
                MessageId::new(1 << 20).unwrap(),
                Uuid::new_v4(),
                ChatId::new(-201).unwrap(),
                &cancel
            )
            .await,
        Err(TelegramError::Request { code: 403 })
    );
    let path = dir.path().join("song.mp3");
    std::fs::write(&path, [0u8; 42]).unwrap();
    let uploaded = transport
        .send_audio(
            target,
            AudioUpload {
                path,
                title: "Track title".into(),
                performer: "Artist".into(),
                duration_seconds: 123,
            },
            "media marker".into(),
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(uploaded.content, MessageContent::Audio);
    assert_eq!(uploaded.id.get(), 10 << 20);
    let caption = vault_protocol::encode_text(&vault_protocol::Record::Media(
        vault_protocol::MediaMarker {
            vault_id: Uuid::new_v4(),
            import_id: Uuid::new_v4(),
            sha256: None,
        },
    ))
    .unwrap();
    let copied = transport
        .copy_media(
            source,
            MessageId::new(1 << 20).unwrap(),
            target,
            caption.clone(),
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(copied.id.get(), 10 << 20);
    assert_eq!(copied.content, MessageContent::Audio);
    assert_eq!(
        transport
            .copy_media(
                source,
                MessageId::new(2 << 20).unwrap(),
                target,
                caption,
                &cancel
            )
            .await,
        Err(TelegramError::Unshareable)
    );
    client.close().await.unwrap();
}
