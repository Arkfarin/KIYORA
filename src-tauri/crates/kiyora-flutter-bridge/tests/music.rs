use kiyora_flutter_bridge::api::{lifecycle::*, models::*, music::*};
mod support;
use support::{fixture, TestResult};

#[tokio::test]
async fn library_pages_and_search_reach_past_ten_thousand_tracks() -> TestResult {
    let root = tempfile::tempdir()?;
    let (init, _) = fixture(root.path(), 10_001).await?;
    let session = init.status.ok_or("no session")?.context.session_id;
    let mut offset = 0;
    let mut count = 0;
    loop {
        let page = library(init.client_id.clone(), session.clone(), None, offset, 500).await?;
        count += page.tracks.len();
        match page.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(count, 10_001);
    let found = library(
        init.client_id.clone(),
        session,
        Some("Track 10000".into()),
        0,
        50,
    )
    .await?;
    assert_eq!(found.tracks.len(), 1);
    assert_eq!(found.tracks[0].title, "Track 10000");
    shutdown(init.client_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an isolated ALSA output; decodes and plays generated digital silence"]
async fn typed_bridge_controls_real_rust_audio_and_fences_queue_revisions() -> TestResult {
    let root = tempfile::tempdir()?;
    let (init, tracks) = fixture(root.path(), 2).await?;
    let client = init.client_id;
    let session = init.status.ok_or("no session")?.context.session_id;
    let q = play_tracks(client.clone(), session.clone(), tracks.clone(), 0).await?;
    assert_eq!(q.current_track_id.as_deref(), Some(tracks[0].as_str()));
    wait_for(&client, &session, PlaybackPhase::Playing).await?;
    transport(client.clone(), session.clone(), Transport::Pause).await?;
    wait_for(&client, &session, PlaybackPhase::Paused).await?;
    seek(client.clone(), session.clone(), 30_000).await?;
    wait_until(&client, &session, |state| {
        (29_000..=31_000).contains(&state.position_ms)
    })
    .await?;
    let version = QueueVersion {
        id: q.id.clone(),
        revision: q.revision,
    };
    let changed = set_repeat(
        client.clone(),
        session.clone(),
        version.clone(),
        Repeat::One,
    )
    .await?;
    assert_eq!(changed.repeat, Repeat::One);
    let conflict = set_shuffle(client.clone(), session.clone(), version, true)
        .await
        .err()
        .ok_or("stale queue revision was accepted")?;
    assert_eq!(conflict.code, FailureCode::Conflict);
    transport(client.clone(), session.clone(), Transport::Next).await?;
    let next = wait_for(&client, &session, PlaybackPhase::Playing).await?;
    assert_eq!(next.track_id.as_deref(), Some(tracks[1].as_str()));
    shutdown(client).await?;
    Ok(())
}

async fn wait_for(client: &str, session: &str, phase: PlaybackPhase) -> TestResult<Playback> {
    wait_until(client, session, |state| state.phase == phase).await
}

async fn wait_until(
    client: &str,
    session: &str,
    ready: impl Fn(&Playback) -> bool,
) -> TestResult<Playback> {
    Ok(
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let state = playback(client.into(), session.into()).await?;
                if ready(&state) {
                    return Ok::<_, Failure>(state);
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await??,
    )
}
