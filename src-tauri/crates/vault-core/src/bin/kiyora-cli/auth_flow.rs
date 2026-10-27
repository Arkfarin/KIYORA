use super::{
    auth_terminal::{AuthUi, InputEvent, Terminal},
    auth_ui,
};
use tokio_util::sync::CancellationToken;
use vault_core::{ApiError, Backend, ErrorCode, Status};
use vault_telegram::{AuthInput, AuthStatus};
use zeroize::Zeroizing;

fn cancelled() -> ApiError {
    ApiError::new(ErrorCode::Cancelled)
}

pub(super) fn credentials(
    id: Option<String>,
    hash: Option<String>,
) -> Result<(i32, Zeroizing<String>), ApiError> {
    let id = id
        .and_then(|v| v.parse::<i32>().ok())
        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
    let hash = Zeroizing::new(hash.ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?);
    if !vault_telegram::valid_api_credentials(id, &hash) {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    Ok((id, hash))
}

pub(super) async fn login(
    backend: &Backend,
    status: Status,
    qr: bool,
    cancel: &CancellationToken,
) -> Result<(), ApiError> {
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    if backend.auth_status(status.context.session_id).await? == AuthStatus::ParametersRequired {
        let (id, hash) = credentials(std::env::var("KIYORA_API_ID").ok(), std::env::var("KIYORA_API_HASH").ok())
            .inspect_err(|_| eprintln!("Set KIYORA_API_ID to a positive integer and KIYORA_API_HASH to its 32 hexadecimal characters."))?;
        // Initialization owns the database lease until completion. A signal is
        // latched, never drops startup halfway through native database opening.
        backend
            .connect_native(status.context.session_id, id, hash)
            .await?;
    }
    if cancel.is_cancelled() {
        return Err(cancelled());
    }
    let mut terminal = Terminal::new()?;
    terminal.message("Kiyora — Telegram Authentication")?;
    let result = run_with_ui(backend, &status, qr, cancel, &mut terminal).await;
    terminal.clear_qr()?;
    if matches!(&result, Err(error) if error.code == ErrorCode::Cancelled) {
        terminal.message(
            "Authorization cancelled. Closing Telegram safely; the profile is retained.",
        )?;
    }
    result
}

/// Raw terminal mode turns Ctrl+C into a key event, so continue reading keys
/// even while an authentication request or account read is in flight.
async fn interruptible<T>(
    operation: impl std::future::Future<Output = Result<T, ApiError>>,
    cancel: &CancellationToken,
    ui: &mut impl AuthUi,
) -> Result<T, ApiError> {
    tokio::pin!(operation);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled()),
            result = &mut operation => return result,
            event = ui.input() => match event? {
                InputEvent::Cancel => { cancel.cancel(); return Err(cancelled()); }
                InputEvent::Line(line) if line.as_str() == "/cancel" => { cancel.cancel(); return Err(cancelled()); }
                _ => {}
            }
        }
    }
}

pub(super) async fn run_with_ui(
    backend: &Backend,
    status: &Status,
    qr: bool,
    cancel: &CancellationToken,
    ui: &mut impl AuthUi,
) -> Result<(), ApiError> {
    let id = status.context.session_id;
    let mut updates = backend.auth_updates(id).await?;
    let initial = updates.current().status;
    let already_ready = initial == AuthStatus::Ready;
    // Resume 2FA after an interrupted login rather than replacing that challenge.
    if qr && initial.can_request_qr() && initial != AuthStatus::PasswordRequired {
        ui.message("Requesting a QR from Telegram… Press Ctrl+C to cancel.")?;
        interruptible(backend.authenticate(id, AuthInput::Qr, cancel), cancel, ui).await?;
    }
    let mut scanned = false;
    let mut previous = None;
    let mut stale_link = None;
    loop {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        let update = updates.current();
        let state = &update.status;
        if previous.as_ref() != Some(&update) {
            ui.clear_qr()?;
            match state {
                AuthStatus::QrConfirmation { link } => {
                    scanned = true;
                    if !update.connected {
                        stale_link = Some(link.clone());
                        ui.message("Telegram connection unavailable. QR hidden; waiting for reconnection and a fresh QR...")?;
                    } else if stale_link.as_ref() == Some(link) {
                        ui.message(
                            "Telegram reconnected. Waiting for a fresh QR from Telegram...",
                        )?;
                    } else {
                        stale_link = None;
                        ui.qr(link)?;
                    }
                }
                AuthStatus::Ready => {
                    let account =
                        interruptible(backend.authorized_account(id, cancel), cancel, ui).await?;
                    let profile = backend
                        .profiles()?
                        .into_iter()
                        .find(|p| p.profile_id == status.context.profile_id)
                        .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
                    ui.message(if already_ready {
                        "Telegram is already authorized."
                    } else {
                        "Telegram authorization successful!"
                    })?;
                    // Account names and profile labels are untrusted terminal text.
                    let clean = |s: &str| s.chars().filter(|c| !c.is_control()).collect::<String>();
                    ui.message(&format!(
                        "\nAccount: {}\nUser ID: {}\nProfile: {} ({})\n\nKiyora is ready.",
                        clean(&account.display_name),
                        account.user_id,
                        clean(&profile.label),
                        profile.profile_id
                    ))?;
                    return Ok(());
                }
                AuthStatus::PhoneRequired => {
                    ui.prompt("Telegram phone number (or /qr, /cancel): ")?;
                }
                AuthStatus::CodeRequired { .. } => {
                    ui.message(&auth_ui::status_description(state))?;
                    ui.prompt("Telegram code (or /qr, /resend, /cancel): ")?;
                }
                AuthStatus::PasswordRequired => {
                    if scanned {
                        ui.message(
                            "QR successfully scanned.\n\nTelegram requires your 2FA password.",
                        )?;
                    }
                    ui.prompt("Telegram 2FA password (or /cancel): ")?;
                }
                AuthStatus::EmailRequired => {
                    ui.prompt("Telegram email (or /qr, /cancel): ")?;
                }
                AuthStatus::EmailCodeRequired { .. } => {
                    ui.message(&auth_ui::status_description(state))?;
                    ui.prompt("Telegram email code (or /qr, /resend, /cancel): ")?;
                }
                _ => {
                    ui.message(&auth_ui::status_description(state))?;
                    return Err(ApiError::new(ErrorCode::AuthRequired));
                }
            }
            previous = Some(update.clone());
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(cancelled()),
            changed = updates.changed(cancel) => { changed?; }
            input = ui.input() => match input? {
                InputEvent::Cancel => { cancel.cancel(); return Err(cancelled()); }
                InputEvent::Resize => { previous = None; }
                InputEvent::Line(line) => {
                    if line.as_str() == "/cancel" { cancel.cancel(); return Err(cancelled()); }
                    // Input can race an update. Discard it when the challenge changed.
                    if updates.current().status != *state { previous = None; continue; }
                    let Some(input) = input_for(state, line)? else { continue; };
                    match interruptible(backend.authenticate(id, input, cancel), cancel, ui).await {
                        Ok(_) => { previous = None; }
                        Err(error) if auth_ui::retry_in_login(&error) => {
                            ui.message(&format!("error[{:?}]: {}", error.code, error.message))?;
                            if error.code == ErrorCode::ResendUnavailable && state.can_request_qr() {
                                ui.message("Telegram offers no other code delivery method. Enter /qr to sign in with your phone's Telegram.")?;
                            }
                            previous = None;
                        }
                        Err(error) => {
                            ui.message("Run auth qr or auth login again to continue the saved session. No login request will be retried automatically.")?;
                            return Err(error);
                        }
                    }
                }
            }
        }
    }
}
fn input_for(state: &AuthStatus, line: Zeroizing<String>) -> Result<Option<AuthInput>, ApiError> {
    if line.as_str() == "/qr" {
        return if state.can_request_qr() {
            Ok(Some(AuthInput::Qr))
        } else {
            Err(ApiError::new(ErrorCode::AuthStateMismatch))
        };
    }
    if line.as_str() == "/resend" {
        return Ok(matches!(
            state,
            AuthStatus::CodeRequired { .. } | AuthStatus::EmailCodeRequired { .. }
        )
        .then_some(AuthInput::ResendCode));
    }
    Ok(match state {
        AuthStatus::PhoneRequired => Some(AuthInput::Phone(line)),
        AuthStatus::CodeRequired { .. } => Some(AuthInput::Code(line)),
        AuthStatus::PasswordRequired => Some(AuthInput::Password(line)),
        AuthStatus::EmailRequired => Some(AuthInput::Email(line)),
        AuthStatus::EmailCodeRequired { .. } => Some(AuthInput::EmailCode(line)),
        _ => None,
    })
}

#[cfg(test)]
pub(super) async fn with_input(
    backend: &Backend,
    status: Status,
    read: impl FnMut(&str) -> Result<Zeroizing<String>, ApiError>,
) -> Result<(), ApiError> {
    struct Script<F> {
        read: F,
        prompt: String,
    }
    impl<F: FnMut(&str) -> Result<Zeroizing<String>, ApiError>> AuthUi for Script<F> {
        fn message(&mut self, _: &str) -> Result<(), ApiError> {
            Ok(())
        }
        fn prompt(&mut self, text: &str) -> Result<(), ApiError> {
            self.prompt = text.into();
            Ok(())
        }
        fn qr(&mut self, _: &str) -> Result<(), ApiError> {
            Ok(())
        }
        fn clear_qr(&mut self) -> Result<(), ApiError> {
            Ok(())
        }
        async fn input(&mut self) -> Result<InputEvent, ApiError> {
            if self.prompt.is_empty() {
                return std::future::pending().await;
            }
            let prompt = std::mem::take(&mut self.prompt);
            (self.read)(&prompt).map(InputEvent::Line)
        }
    }
    run_with_ui(
        backend,
        &status,
        false,
        &CancellationToken::new(),
        &mut Script {
            read,
            prompt: String::new(),
        },
    )
    .await
}

#[cfg(test)]
pub(super) static NATIVE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[cfg(test)]
#[path = "auth_flow_tests.rs"]
mod tests;
