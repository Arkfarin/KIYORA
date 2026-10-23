use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use vault_media::{
    PlatformPlaybackSession, PlaybackControls, PlaybackEngine, PlaybackError, PlaybackPlatform,
};

#[test]
fn native_service_failure_prevents_starting_audio() {
    struct Unavailable(Arc<AtomicBool>);
    impl PlaybackPlatform for Unavailable {
        fn connect(
            &self,
            _: PlaybackControls,
        ) -> Result<Box<dyn PlatformPlaybackSession>, PlaybackError> {
            self.0.store(true, Ordering::Release);
            Err(PlaybackError::DeviceUnavailable)
        }
    }
    let called = Arc::new(AtomicBool::new(false));
    assert!(PlaybackEngine::open_with_platform(Arc::new(Unavailable(called.clone()))).is_err());
    assert!(called.load(Ordering::Acquire));
}
