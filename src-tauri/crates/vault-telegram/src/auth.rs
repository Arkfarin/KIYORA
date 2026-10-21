use crate::{TdClient, TelegramError};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use zeroize::Zeroizing;

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
        if self.api_id <= 0 || self.api_hash.len() != 32 || self.database_key.len() != 32 {
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
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthStatus {
    ParametersRequired,
    PhoneRequired,
    CodeRequired,
    PasswordRequired,
    QrConfirmation { link: String },
    EmailRequired,
    EmailCodeRequired,
    RegistrationRequired,
    PremiumRequired,
    Ready,
    LoggingOut,
    Closing,
    Closed,
    Unsupported,
}
impl AuthStatus {
    pub fn from_td(value: &Value) -> Self {
        match value["@type"].as_str() {
            Some("authorizationStateWaitTdlibParameters") => Self::ParametersRequired,
            Some("authorizationStateWaitPhoneNumber") => Self::PhoneRequired,
            Some("authorizationStateWaitCode") => Self::CodeRequired,
            Some("authorizationStateWaitPassword") => Self::PasswordRequired,
            Some("authorizationStateWaitOtherDeviceConfirmation") => Self::QrConfirmation {
                link: value["link"].as_str().unwrap_or("").into(),
            },
            Some("authorizationStateWaitEmailAddress") => Self::EmailRequired,
            Some("authorizationStateWaitEmailCode") => Self::EmailCodeRequired,
            Some("authorizationStateWaitRegistration") => Self::RegistrationRequired,
            Some("authorizationStateWaitPremiumPurchase") => Self::PremiumRequired,
            Some("authorizationStateReady") => Self::Ready,
            Some("authorizationStateLoggingOut") => Self::LoggingOut,
            Some("authorizationStateClosing") => Self::Closing,
            Some("authorizationStateClosed") => Self::Closed,
            _ => Self::Unsupported,
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
}
impl TdClient {
    pub fn auth_status(&self) -> AuthStatus {
        AuthStatus::from_td(&self.authorization())
    }
    pub async fn authenticate(&self, input: AuthInput) -> Result<AuthStatus, TelegramError> {
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
        };
        self.call(value).await?;
        let state = self.call(json!({"@type":"getAuthorizationState"})).await?;
        Ok(AuthStatus::from_td(&state))
    }
    pub async fn logout(&self) -> Result<(), TelegramError> {
        self.call(json!({"@type":"logOut"})).await?;
        self.close().await
    }
}
