use crate::{TdClient, TelegramError};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::{sync::watch, time::Instant};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

/// Safe delivery metadata. Phone numbers, code hints, receipts and token URLs
/// deliberately stay in TDLib and are not part of application diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CodeDelivery {
    TelegramMessage,
    Sms,
    SmsWord,
    SmsPhrase,
    Call,
    FlashCall,
    MissedCall,
    Fragment,
    FirebaseAndroid,
    FirebaseIos,
    Unknown,
}
impl CodeDelivery {
    fn from_td(value: &Value) -> Self {
        match value["@type"].as_str() {
            Some("authenticationCodeTypeTelegramMessage") => Self::TelegramMessage,
            Some("authenticationCodeTypeSms") => Self::Sms,
            Some("authenticationCodeTypeSmsWord") => Self::SmsWord,
            Some("authenticationCodeTypeSmsPhrase") => Self::SmsPhrase,
            Some("authenticationCodeTypeCall") => Self::Call,
            Some("authenticationCodeTypeFlashCall") => Self::FlashCall,
            Some("authenticationCodeTypeMissedCall") => Self::MissedCall,
            Some("authenticationCodeTypeFragment") => Self::Fragment,
            Some("authenticationCodeTypeFirebaseAndroid") => Self::FirebaseAndroid,
            Some("authenticationCodeTypeFirebaseIos") => Self::FirebaseIos,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuthCodeInfo {
    pub delivery: CodeDelivery,
    /// None for unknown length, word/phrase codes, or malformed metadata.
    pub expected_length: Option<u32>,
    /// Remaining delay at the time TDLib produced this state; None if unknown.
    pub timeout_seconds: Option<u32>,
    /// None means Telegram does not offer another delivery method.
    pub next_delivery: Option<CodeDelivery>,
}
impl AuthCodeInfo {
    fn from_td(value: &Value) -> Self {
        Self {
            delivery: CodeDelivery::from_td(&value["type"]),
            expected_length: code_length(&value["type"]),
            timeout_seconds: value["timeout"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok()),
            next_delivery: value
                .get("next_type")
                .filter(|v| {
                    v["@type"]
                        .as_str()
                        .is_some_and(|t| t.starts_with("authenticationCodeType"))
                })
                .map(CodeDelivery::from_td),
        }
    }
}
fn code_length(value: &Value) -> Option<u32> {
    value["length"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
}

/// Shared by all clones: auth mutations are serialized and FloodWait is honored
/// even if a caller immediately retries with another input or delivery method.
#[derive(Default)]
pub(crate) struct AuthAttempts {
    limited_at: Option<(Instant, u64)>,
}
impl AuthAttempts {
    fn check(&self) -> Result<(), TelegramError> {
        if let Some((since, seconds)) = self.limited_at {
            let remaining = seconds.saturating_sub(since.elapsed().as_secs());
            if remaining > 0 {
                return Err(TelegramError::RateLimited {
                    retry_after_seconds: remaining,
                });
            }
        }
        Ok(())
    }
    fn record(&mut self, error: &TelegramError) {
        if let TelegramError::RateLimited {
            retry_after_seconds,
        } = error
        {
            self.limited_at = Some((Instant::now(), *retry_after_seconds));
        }
    }
}

/// Intentionally has no Debug/Serialize implementation.
pub struct TdParameters {
    pub api_id: i32,
    pub api_hash: Zeroizing<String>,
    pub database_key: Zeroizing<Vec<u8>>,
    pub database_directory: PathBuf,
    pub files_directory: PathBuf,
}
impl TdParameters {
    pub async fn initialize(self, client: &TdClient) -> Result<(), TelegramError> {
        let started = Instant::now();
        tracing::info!(target: "kiyora::auth", event = "auth.request", operation = "initialize",
            state = client.auth_status().name(), tdlib_version = crate::TDLIB_VERSION);
        let result = self.initialize_inner(client).await;
        client.trace_auth_result("initialize", started, result.as_ref().err());
        result
    }
    async fn initialize_inner(self, client: &TdClient) -> Result<(), TelegramError> {
        if !valid_api_credentials(self.api_id, &self.api_hash) || self.database_key.len() != 32 {
            return Err(TelegramError::InvalidInput);
        }
        let database = self
            .database_directory
            .to_str()
            .ok_or(TelegramError::InvalidInput)?;
        let files = self
            .files_directory
            .to_str()
            .ok_or(TelegramError::InvalidInput)?;
        client.verify_version().await?;
        client.call(json!({"@type":"setTdlibParameters","use_test_dc":false,
            "database_directory":database,"files_directory":files,"database_encryption_key":STANDARD.encode(&*self.database_key),
            "use_file_database":true,"use_chat_info_database":true,"use_message_database":true,"use_secret_chats":false,
            "api_id":self.api_id,"api_hash":*self.api_hash,"system_language_code":"en","device_model":"kiyora",
            "system_version":std::env::consts::OS,"application_version":env!("CARGO_PKG_VERSION")
        })).await?;
        // The application coordinates local file eviction with range readers.
        // This option requires initialized database state; setting it before
        // setTdlibParameters can leave its response waiting for initialization.
        client
            .call(json!({"@type":"setOption", "name":"use_storage_optimizer",
            "value":{"@type":"optionValueBoolean","value":false}}))
            .await?;
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthStatus {
    ParametersRequired,
    PhoneRequired,
    CodeRequired { code_info: AuthCodeInfo },
    PasswordRequired,
    QrConfirmation { link: String },
    EmailRequired,
    EmailCodeRequired { expected_length: Option<u32> },
    RegistrationRequired,
    PremiumRequired,
    Ready,
    LoggingOut,
    Closing,
    Closed,
    Unsupported,
}
// AuthStatus travels through UI events, but Debug must never reveal QR credentials.
impl std::fmt::Debug for AuthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

pub fn valid_api_credentials(api_id: i32, api_hash: &str) -> bool {
    api_id > 0 && api_hash.len() == 32 && api_hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Latest values, coalesced rather than queued: a slow UI must not replay expired QR tokens.
/// Contains ephemeral credentials; never persist or log the returned authorization state.
pub struct AuthUpdates {
    pub(crate) auth: watch::Receiver<Value>,
    pub(crate) connection: watch::Receiver<Value>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthUpdate {
    pub status: AuthStatus,
    pub connected: bool,
}
impl AuthUpdates {
    pub fn current(&mut self) -> AuthUpdate {
        AuthUpdate {
            status: AuthStatus::from_td(&self.auth.borrow_and_update()),
            connected: self.connection.borrow_and_update()["@type"] == "connectionStateReady",
        }
    }
    pub async fn changed(&mut self, cancel: &CancellationToken) -> Result<(), TelegramError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(TelegramError::Cancelled),
            result = self.auth.changed() => result.map_err(|_| TelegramError::Closed),
            result = self.connection.changed() => result.map_err(|_| TelegramError::Closed),
        }
    }
}

impl AuthStatus {
    pub fn from_td(value: &Value) -> Self {
        match value["@type"].as_str() {
            Some("authorizationStateWaitTdlibParameters") => Self::ParametersRequired,
            Some("authorizationStateWaitPhoneNumber") => Self::PhoneRequired,
            Some("authorizationStateWaitCode") => Self::CodeRequired {
                code_info: AuthCodeInfo::from_td(&value["code_info"]),
            },
            Some("authorizationStateWaitPassword") => Self::PasswordRequired,
            Some("authorizationStateWaitOtherDeviceConfirmation") => Self::QrConfirmation {
                link: value["link"].as_str().unwrap_or("").into(),
            },
            Some("authorizationStateWaitEmailAddress") => Self::EmailRequired,
            Some("authorizationStateWaitEmailCode") => Self::EmailCodeRequired {
                expected_length: code_length(&value["code_info"]),
            },
            Some("authorizationStateWaitRegistration") => Self::RegistrationRequired,
            Some("authorizationStateWaitPremiumPurchase") => Self::PremiumRequired,
            Some("authorizationStateReady") => Self::Ready,
            Some("authorizationStateLoggingOut") => Self::LoggingOut,
            Some("authorizationStateClosing") => Self::Closing,
            Some("authorizationStateClosed") => Self::Closed,
            _ => Self::Unsupported,
        }
    }
    /// Stable allowlisted name, safe even for states containing QR credentials.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ParametersRequired => "PARAMETERS_REQUIRED",
            Self::PhoneRequired => "PHONE_REQUIRED",
            Self::CodeRequired { .. } => "CODE_REQUIRED",
            Self::PasswordRequired => "PASSWORD_REQUIRED",
            Self::QrConfirmation { .. } => "QR_CONFIRMATION",
            Self::EmailRequired => "EMAIL_REQUIRED",
            Self::EmailCodeRequired { .. } => "EMAIL_CODE_REQUIRED",
            Self::RegistrationRequired => "REGISTRATION_REQUIRED",
            Self::PremiumRequired => "PREMIUM_REQUIRED",
            Self::Ready => "READY",
            Self::LoggingOut => "LOGGING_OUT",
            Self::Closing => "CLOSING",
            Self::Closed => "CLOSED",
            Self::Unsupported => "UNSUPPORTED",
        }
    }
    pub(crate) fn trace(&self) {
        let info = match self {
            Self::CodeRequired { code_info } => Some(code_info),
            _ => None,
        };
        let expected_length = match self {
            Self::EmailCodeRequired { expected_length } => *expected_length,
            _ => info.and_then(|v| v.expected_length),
        };
        tracing::info!(target: "kiyora::auth", event = "auth.state", state = self.name(),
            delivery = info.map(|v| tracing::field::debug(v.delivery)), expected_length,
            timeout_seconds = info.and_then(|v| v.timeout_seconds),
            next_delivery = info.and_then(|v| v.next_delivery).map(tracing::field::debug));
    }
    /// TDLib 1.8.69 also requires no outstanding authentication query.
    /// This checks the state only; the client/worker enforce query exclusion.
    pub fn can_request_qr(&self) -> bool {
        matches!(
            self,
            Self::PhoneRequired
                | Self::PremiumRequired
                | Self::EmailRequired
                | Self::EmailCodeRequired { .. }
                | Self::CodeRequired { .. }
                | Self::RegistrationRequired
                | Self::PasswordRequired
        )
    }
    fn validate_input(&self, input: &AuthInput) -> Result<(), TelegramError> {
        if matches!(input, AuthInput::ResendCode) {
            return match self {
                Self::EmailCodeRequired { .. } => Ok(()),
                Self::CodeRequired { code_info } if code_info.next_delivery.is_some() => {
                    match code_info.timeout_seconds {
                        Some(0) => Ok(()),
                        Some(seconds) => Err(TelegramError::ResendTooEarly {
                            retry_after_seconds: u64::from(seconds),
                        }),
                        None => Err(TelegramError::ResendUnavailable),
                    }
                }
                _ => Err(TelegramError::ResendUnavailable),
            };
        }
        if matches!(input, AuthInput::Qr) {
            return if self.can_request_qr() {
                Ok(())
            } else {
                Err(TelegramError::AuthStateMismatch)
            };
        }
        let valid = matches!(
            (self, input),
            (Self::PhoneRequired, AuthInput::Phone(_))
                | (Self::CodeRequired { .. }, AuthInput::Code(_))
                | (Self::PasswordRequired, AuthInput::Password(_))
                | (Self::EmailRequired, AuthInput::Email(_))
                | (Self::EmailCodeRequired { .. }, AuthInput::EmailCode(_))
        );
        if valid {
            Ok(())
        } else {
            Err(TelegramError::AuthStateMismatch)
        }
    }
}

/// Secrets exist only for this request; never persist login codes or 2FA passwords.
pub enum AuthInput {
    Phone(Zeroizing<String>),
    Code(Zeroizing<String>),
    Password(Zeroizing<String>),
    Qr,
    Email(Zeroizing<String>),
    EmailCode(Zeroizing<String>),
    ResendCode,
}
impl AuthInput {
    fn operation(&self) -> &'static str {
        match self {
            Self::Phone(_) => "phone",
            Self::Code(_) => "code",
            Self::Password(_) => "password",
            Self::Qr => "qr",
            Self::Email(_) => "email",
            Self::EmailCode(_) => "email_code",
            Self::ResendCode => "resend_code",
        }
    }
}
impl TdClient {
    pub fn auth_status(&self) -> AuthStatus {
        AuthStatus::from_td(&self.authorization())
    }
    pub async fn authenticate(&self, input: AuthInput) -> Result<AuthStatus, TelegramError> {
        self.authenticate_cancellable(input, &CancellationToken::new())
            .await
    }
    pub async fn authenticate_cancellable(
        &self,
        input: AuthInput,
        cancel: &CancellationToken,
    ) -> Result<AuthStatus, TelegramError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(TelegramError::Cancelled),
            result = self.authenticate_inner(input) => result,
        }
    }
    async fn authenticate_inner(&self, input: AuthInput) -> Result<AuthStatus, TelegramError> {
        let mut attempts = self
            .auth_attempts
            .try_lock()
            .map_err(|_| TelegramError::AuthBusy)?;
        let operation = input.operation();
        let started = Instant::now();
        tracing::info!(target: "kiyora::auth", event = "auth.request", operation, state = self.auth_status().name(), connection = self.auth_connection_name());
        if let Err(error) = attempts.check() {
            // A locally blocked attempt must not move the server's deadline.
            self.trace_auth_result(operation, started, Some(&error));
            return Err(error);
        }
        let result = async {
            // getAuthorizationState recomputes TDLib's remaining resend delay.
            // Never authorize a resend from an old update or a CLI countdown.
            self.refresh_auth_status().await?.validate_input(&input)?;
            self.submit_auth_input(input).await
        }
        .await;
        if let Err(error) = &result {
            attempts.record(error);
        }
        self.trace_auth_result(operation, started, result.as_ref().err());
        result
    }
    pub async fn resend_authentication_code(&self) -> Result<AuthStatus, TelegramError> {
        self.authenticate(AuthInput::ResendCode).await
    }
    pub async fn refresh_auth_status(&self) -> Result<AuthStatus, TelegramError> {
        let state = self.call(json!({"@type":"getAuthorizationState"})).await?;
        if !state["@type"]
            .as_str()
            .is_some_and(|s| s.starts_with("authorizationState"))
        {
            return Err(TelegramError::InvalidResponse);
        }
        Ok(AuthStatus::from_td(&state))
    }
    fn auth_connection_name(&self) -> &'static str {
        match self.connection_state()["@type"].as_str() {
            Some("connectionStateWaitingForNetwork") => "waiting_for_network",
            Some("connectionStateConnectingToProxy") => "connecting_to_proxy",
            Some("connectionStateConnecting") => "connecting",
            Some("connectionStateUpdating") => "updating",
            Some("connectionStateReady") => "ready",
            _ => "unknown",
        }
    }
    fn trace_auth_result(
        &self,
        operation: &'static str,
        started: Instant,
        error: Option<&TelegramError>,
    ) {
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(error) = error {
            use TelegramError::*;
            let error_code = match error {
                InvalidCode => "invalid_code",
                CodeExpired => "code_expired",
                InvalidPassword => "invalid_password",
                RateLimited { .. } => "rate_limited",
                ResendTooEarly { .. } => "resend_too_early",
                ResendUnavailable => "resend_unavailable",
                AuthStateMismatch => "auth_state_mismatch",
                AuthBusy => "auth_busy",
                Request { code: 408 } | Timeout => "timeout",
                Network
                | Request {
                    code: 500..=599 | -1 | 0,
                } => "network",
                Request { .. } => "telegram_rejected",
                Cancelled => "cancelled",
                Closed => "closed",
                InvalidInput => "invalid_input",
                NativeLibrary | UnsupportedVersion => "native_unavailable",
                _ => "internal",
            };
            let tdlib_error_code = match error {
                Request { code } => Some(*code),
                _ => None,
            };
            let retry_after_seconds = match error {
                RateLimited {
                    retry_after_seconds,
                }
                | ResendTooEarly {
                    retry_after_seconds,
                } => Some(*retry_after_seconds),
                _ => None,
            };
            tracing::warn!(target: "kiyora::auth", event = "auth.result", operation, outcome = "error",
                error_code, tdlib_error_code, retry_after_seconds, elapsed_ms, connection = self.auth_connection_name());
        } else {
            tracing::info!(target: "kiyora::auth", event = "auth.result", operation, outcome = "ok", elapsed_ms);
        }
    }
    async fn submit_auth_input(&self, input: AuthInput) -> Result<AuthStatus, TelegramError> {
        let value = match input {
            AuthInput::Phone(phone) => {
                json!({"@type":"setAuthenticationPhoneNumber","phone_number":*phone,"settings":null})
            }
            AuthInput::Code(code) => json!({"@type":"checkAuthenticationCode","code":*code}),
            AuthInput::Password(password) => {
                json!({"@type":"checkAuthenticationPassword","password":*password})
            }
            AuthInput::Qr => json!({"@type":"requestQrCodeAuthentication","other_user_ids":[]}),
            AuthInput::Email(email) => {
                json!({"@type":"setAuthenticationEmailAddress","email_address":*email})
            }
            AuthInput::EmailCode(code) => {
                json!({"@type":"checkAuthenticationEmailCode","code":{"@type":"emailAddressAuthenticationCode","code":*code}})
            }
            AuthInput::ResendCode => json!({"@type":"resendAuthenticationCode",
                "reason":{"@type":"resendCodeReasonUserRequest"}}),
        };
        self.call(value).await?;
        self.refresh_auth_status().await
    }
    pub async fn logout(&self) -> Result<(), TelegramError> {
        self.call(json!({"@type":"logOut"})).await?;
        self.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn flood_wait_uses_monotonic_time_and_never_shortens_server_delay() {
        let mut attempts = AuthAttempts::default();
        attempts.record(&TelegramError::RateLimited {
            retry_after_seconds: 100_000,
        });
        tokio::time::advance(std::time::Duration::from_millis(99_999_500)).await;
        assert_eq!(
            attempts.check(),
            Err(TelegramError::RateLimited {
                retry_after_seconds: 1
            })
        );
        tokio::time::advance(std::time::Duration::from_millis(500)).await;
        assert_eq!(attempts.check(), Ok(()));
    }
}
