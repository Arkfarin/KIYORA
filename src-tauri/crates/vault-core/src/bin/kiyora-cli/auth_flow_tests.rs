#![allow(clippy::unwrap_used)]
use super::*;

#[test]
fn missing_and_invalid_credentials_fail_without_exposing_values() {
    for (id, hash) in [
        (None, None),
        (Some(""), Some("a")),
        (Some("-1"), Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")),
        (Some("123"), None),
        (Some("PRIVATE"), Some("PRIVATE")),
        (Some("123"), Some("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")),
    ] {
        let error = credentials(id.map(str::to_owned), hash.map(str::to_owned)).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidInput);
        assert!(!format!("{error:?}").contains("PRIVATE"));
    }
    assert!(credentials(Some("123".into()), Some("a".repeat(32))).is_ok());
}

#[test]
fn phone_and_email_codes_can_switch_to_qr_but_qr_does_not_restart_itself() {
    let code = AuthStatus::from_td(&serde_json::json!({"@type":"authorizationStateWaitCode"}));
    assert!(matches!(
        input_for(&code, "/qr".to_owned().into()).unwrap(),
        Some(AuthInput::Qr)
    ));
    assert!(matches!(
        input_for(&code, "/resend".to_owned().into()).unwrap(),
        Some(AuthInput::ResendCode)
    ));
    let qr = AuthStatus::QrConfirmation {
        link: "private".into(),
    };
    assert_eq!(
        input_for(&qr, "/qr".to_owned().into()).err().unwrap().code,
        ErrorCode::AuthStateMismatch
    );
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use std::{
        collections::BTreeMap,
        path::Path,
        sync::{Arc, Mutex},
    };
    use tokio::sync::mpsc;
    use vault_security::{ProfileStore, SecretStore, SecurityError};

    #[derive(Default)]
    struct Secrets(Mutex<BTreeMap<String, Vec<u8>>>);
    impl SecretStore for Secrets {
        fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecurityError> {
            Ok(self.0.lock().unwrap().get(key).cloned().map(Zeroizing::new))
        }
        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecurityError> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
    }
    enum Output {
        Text(String),
        Qr(String),
        Clear,
    }
    struct Ui {
        output: mpsc::UnboundedSender<Output>,
        input: mpsc::UnboundedReceiver<InputEvent>,
    }
    impl AuthUi for Ui {
        fn message(&mut self, s: &str) -> Result<(), ApiError> {
            self.output.send(Output::Text(s.into())).unwrap();
            Ok(())
        }
        fn prompt(&mut self, s: &str) -> Result<(), ApiError> {
            self.message(s)
        }
        fn qr(&mut self, s: &str) -> Result<(), ApiError> {
            self.output.send(Output::Qr(s.into())).unwrap();
            Ok(())
        }
        fn clear_qr(&mut self) -> Result<(), ApiError> {
            self.output.send(Output::Clear).unwrap();
            Ok(())
        }
        async fn input(&mut self) -> Result<InputEvent, ApiError> {
            Ok(self.input.recv().await.unwrap_or(InputEvent::Cancel))
        }
    }
    async fn output(rx: &mut mpsc::UnboundedReceiver<Output>) -> Output {
        tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn text(rx: &mut mpsc::UnboundedReceiver<Output>, expected: &str) {
        loop {
            if let Output::Text(s) = output(rx).await {
                assert!(!s.contains("PRIVATE") && !s.contains("tg://"));
                if s.contains(expected) {
                    break;
                }
            }
        }
    }
    async fn qr(rx: &mut mpsc::UnboundedReceiver<Output>) -> String {
        let mut cleared = false;
        loop {
            match output(rx).await {
                Output::Clear => cleared = true,
                Output::Qr(s) => {
                    assert!(cleared, "old QR must be cleared first");
                    return s;
                }
                Output::Text(_) => {}
            }
        }
    }
    fn control(path: &Path, value: &str) {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, value).unwrap();
        std::fs::rename(tmp, path).unwrap();
    }
    fn spawn(
        backend: &Backend,
        status: &Status,
        use_qr: bool,
        cancel: &CancellationToken,
    ) -> (
        mpsc::UnboundedSender<InputEvent>,
        mpsc::UnboundedReceiver<Output>,
        tokio::task::JoinHandle<Result<(), ApiError>>,
    ) {
        let (tx, input) = mpsc::unbounded_channel();
        let (output, rx) = mpsc::unbounded_channel();
        let backend = backend.clone();
        let status = status.clone();
        let cancel = cancel.clone();
        let task = tokio::spawn(async move {
            run_with_ui(
                &backend,
                &status,
                use_qr,
                &cancel,
                &mut Ui { output, input },
            )
            .await
        });
        (tx, rx, task)
    }
    #[tokio::test]
    async fn qr_flow_refresh_reconnect_password_cancel_restart_and_profile_exclusion() {
        let _guard = NATIVE_TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("auth.so");
        assert!(
            std::process::Command::new("cc")
                .args(["-shared", "-fPIC", "-o"])
                .arg(&library)
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../vault-telegram/tests/auth_fixture.c"
                ))
                .status()
                .unwrap()
                .success()
        );
        let data = dir.path().join("data");
        let secrets = Arc::new(Secrets::default());
        let open =
            || Backend::with_secrets(ProfileStore::at(&data).unwrap(), &library, secrets.clone());
        let backend = open();
        assert!(backend.restore_active_profile().await.unwrap().is_none());
        let profile = backend.create_profile("QR test".into()).unwrap();
        let mut status = backend.switch_profile(profile.profile_id).await.unwrap();
        for (id, hash) in [
            (0, "a".repeat(32)),
            (123, "bad".into()),
            (123, "z".repeat(32)),
        ] {
            assert_eq!(
                backend
                    .connect_native(status.context.session_id, id, hash.into())
                    .await
                    .unwrap_err()
                    .code,
                ErrorCode::InvalidInput
            );
        }
        backend
            .connect_native(status.context.session_id, 123, "a".repeat(32).into())
            .await
            .unwrap();
        let other = open();
        assert_eq!(
            other.restore_active_profile().await.unwrap_err().code,
            ErrorCode::ProfileBusy
        );
        let control_file = data
            .join("profiles")
            .join(profile.profile_id.to_string())
            .join("tdlib/fixture-control");
        let cancel = CancellationToken::new();
        let (tx, mut rx, task) = spawn(&backend, &status, true, &cancel);
        let first = qr(&mut rx).await;
        assert_eq!(first, "tg://login?token=PRIVATE_QR_TOKEN_0");
        control(&control_file, "rotate");
        let second = qr(&mut rx).await;
        assert_ne!(first, second);
        control(&control_file, "offline");
        text(&mut rx, "QR hidden").await;
        control(&control_file, "online");
        text(&mut rx, "fresh QR").await;
        // The old link is never redisplayed on reconnection.
        control(&control_file, "rotate");
        assert_ne!(qr(&mut rx).await, second);
        control(&control_file, "WaitPassword");
        text(&mut rx, "QR successfully scanned").await;
        text(&mut rx, "2FA password").await;
        tx.send(InputEvent::Line("WRONG_PASSWORD_PRIVATE".to_owned().into()))
            .unwrap();
        text(&mut rx, "InvalidPassword").await;
        text(&mut rx, "2FA password (or").await;
        tx.send(InputEvent::Line("PASSWORD_PRIVATE".to_owned().into()))
            .unwrap();
        text(&mut rx, "authorization successful").await;
        text(&mut rx, "Account: Test Account").await;
        assert!(task.await.unwrap().is_ok());
        assert_eq!(
            backend
                .auth_status(status.context.session_id)
                .await
                .unwrap(),
            AuthStatus::Ready
        );
        backend.shutdown().await.unwrap();
        status = backend.restore_active_profile().await.unwrap().unwrap();
        let (_tx, mut rx, task) = spawn(&backend, &status, true, &CancellationToken::new());
        text(&mut rx, "already authorized").await;
        assert!(task.await.unwrap().is_ok());

        // Re-enter a phone challenge: /qr uses the same session, then cancel via
        // the same InputEvent produced by a terminal Ctrl+C key.
        control(&control_file, "WaitCode");
        let mut updates = backend
            .auth_updates(status.context.session_id)
            .await
            .unwrap();
        while !matches!(updates.current().status, AuthStatus::CodeRequired { .. }) {
            updates.changed(&CancellationToken::new()).await.unwrap();
        }
        let (tx, mut rx, task) = spawn(&backend, &status, false, &CancellationToken::new());
        text(&mut rx, "Telegram code").await;
        tx.send(InputEvent::Line("/qr".to_owned().into())).unwrap();
        qr(&mut rx).await;
        tx.send(InputEvent::Cancel).unwrap();
        assert_eq!(task.await.unwrap().unwrap_err().code, ErrorCode::Cancelled);
        assert!(matches!(
            backend
                .auth_status(status.context.session_id)
                .await
                .unwrap(),
            AuthStatus::QrConfirmation { .. }
        ));
        backend.shutdown().await.unwrap();
        status = backend.restore_active_profile().await.unwrap().unwrap();
        let cancel = CancellationToken::new();
        let (_tx, mut rx, task) = spawn(&backend, &status, true, &cancel);
        qr(&mut rx).await;
        cancel.cancel(); // SIGINT path uses this token.
        assert_eq!(task.await.unwrap().unwrap_err().code, ErrorCode::Cancelled);
        let (tx, mut rx, task) = spawn(&backend, &status, true, &CancellationToken::new());
        qr(&mut rx).await;
        tx.send(InputEvent::Line("/cancel".to_owned().into()))
            .unwrap();
        assert_eq!(task.await.unwrap().unwrap_err().code, ErrorCode::Cancelled);
        let (_tx, mut rx, task) = spawn(&backend, &status, true, &CancellationToken::new());
        qr(&mut rx).await;
        control(&control_file, "FutureState");
        text(&mut rx, "unsupported").await;
        assert_eq!(
            task.await.unwrap().unwrap_err().code,
            ErrorCode::AuthRequired
        );
        backend.shutdown().await.unwrap();
        assert_eq!(backend.profiles().unwrap().len(), 1);
    }
    // Invoked only by the PTY driver in a separate process. Production CLI has
    // no test secret-store, fixture or environment override for authentication.
    #[test]
    fn terminal_fixture_child() {
        let Some(root) = std::env::var_os("KIYORA_TEST_AUTH_TERMINAL") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let library = root.join("auth.so");
        if !library.exists() {
            assert!(
                std::process::Command::new("cc")
                    .args(["-shared", "-fPIC", "-o"])
                    .arg(&library)
                    .arg(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../vault-telegram/tests/auth_fixture.c"
                    ))
                    .status()
                    .unwrap()
                    .success()
            );
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let secrets_file = root.join("test-secrets.json");
            let secrets = Arc::new(Secrets(Mutex::new(
                std::fs::read(&secrets_file)
                    .ok()
                    .map(|bytes| serde_json::from_slice(&bytes).unwrap())
                    .unwrap_or_default(),
            )));
            let backend = Backend::with_secrets(
                ProfileStore::at(root.join("data")).unwrap(),
                &library,
                secrets.clone(),
            );
            let status = if let Some(status) = backend.restore_active_profile().await.unwrap() {
                status
            } else {
                let profile = backend.create_profile("PTY test".into()).unwrap();
                backend.switch_profile(profile.profile_id).await.unwrap()
            };
            // Test-only credentials/keys persist within the Python TempDir.
            // Production uses OS secrets; this child never accesses real accounts.
            backend
                .connect_native(status.context.session_id, 123, "a".repeat(32).into())
                .await
                .unwrap();
            std::fs::write(
                &secrets_file,
                serde_json::to_vec(&*secrets.0.lock().unwrap()).unwrap(),
            )
            .unwrap();
            let cancel = CancellationToken::new();
            let signal_cancel = cancel.clone();
            let signal = tokio::spawn(async move {
                tokio::signal::ctrl_c().await.unwrap();
                signal_cancel.cancel();
            });
            let result = login(
                &backend,
                status,
                std::env::var("KIYORA_TEST_AUTH_PHONE").unwrap() != "1",
                &cancel,
            )
            .await;
            assert!(result.is_ok() || result.unwrap_err().code == ErrorCode::Cancelled);
            backend.shutdown().await.unwrap();
            signal.abort();
        });
        println!("PTY_CLOSED");
    }

    #[test]
    fn terminal_pty_rotation_hidden_password_and_real_ctrl_c() {
        let result = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/qr_terminal_pty.py"
            ))
            .arg(std::env::current_exe().unwrap())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
