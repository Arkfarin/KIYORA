use kiyora_flutter_bridge::api::events::*;
use kiyora_flutter_bridge::api::{lifecycle::*, models::*};

// One integration test owns the process singleton; test processes are isolated.
#[tokio::test]
async fn duplicate_start_shutdown_and_stale_handles_preserve_profiles()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let options = InitOptions {
        data_directory: Some(root.path().to_string_lossy().into_owned()),
        resource_directory: None,
    };
    let (left, right) = tokio::join!(initialize(options.clone()), initialize(options.clone()));
    let first = left?;
    assert_eq!(first.client_id, right?.client_id);
    assert!(first.profiles.is_empty());
    let subscription = open_events(first.client_id.clone()).await?;
    let pending = next_event(first.client_id.clone(), subscription.clone());
    tokio::pin!(pending);
    tokio::select! {
        biased;
        _ = &mut pending => panic!("empty stream must wait"),
        _ = std::future::ready(()) => {},
    }
    close_events(first.client_id.clone(), subscription).await?;
    assert!(pending.await?.is_none());
    let profile = create_profile(first.client_id.clone(), "Test library".into()).await?;
    let selected = switch_profile(first.client_id.clone(), profile.id.clone()).await?;
    assert_eq!(selected.context.profile_id, profile.id);
    let again = initialize(options.clone()).await?;
    assert_eq!(again.client_id, first.client_id);
    assert_eq!(
        again.status.map(|s| s.context.session_id),
        Some(selected.context.session_id)
    );

    let another_root = tempfile::tempdir()?;
    let other = InitOptions {
        data_directory: Some(another_root.path().to_string_lossy().into_owned()),
        resource_directory: None,
    };
    let error = initialize(other)
        .await
        .err()
        .ok_or("conflicting startup succeeded")?;
    assert_eq!(error.code, FailureCode::Conflict);
    shutdown(first.client_id.clone()).await?;
    shutdown(first.client_id.clone()).await?;
    let reopened = initialize(options).await?;
    assert_ne!(reopened.client_id, first.client_id);
    assert_eq!(reopened.profiles.len(), 1);
    assert_eq!(reopened.profiles[0].id, profile.id);
    assert_eq!(
        profiles(first.client_id)
            .await
            .err()
            .ok_or("stale generation was accepted")?
            .code,
        FailureCode::Closed
    );
    assert!(reopened.status.is_some());
    shutdown(reopened.client_id).await?;
    Ok(())
}
