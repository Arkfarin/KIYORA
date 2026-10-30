use super::models::{Failure, RuntimeStatus};
use crate::runtime::{failure, id, lease};
use vault_core::{ErrorCode, LoginInput};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthPhase {
    Parameters,
    Phone,
    Code,
    Password,
    Qr,
    Email,
    EmailCode,
    Registration,
    Premium,
    Ready,
    LoggingOut,
    Closing,
    Closed,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Telegram,
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

// No Debug implementation: QR links are short-lived credentials.
pub struct AuthState {
    pub phase: AuthPhase,
    pub qr_link: Option<String>,
    pub expected_length: Option<u32>,
    pub timeout_seconds: Option<u32>,
    pub delivery: Option<Delivery>,
    pub next_delivery: Option<Delivery>,
}

#[derive(Clone, Copy, Debug)]
pub enum LoginStep {
    Phone,
    Code,
    Password,
    Qr,
    Email,
    EmailCode,
    ResendCode,
}

#[derive(Clone, Debug)]
pub struct Vault {
    pub id: String,
    pub chat_id: String,
    pub title: String,
    pub protocol: u32,
}

pub async fn connect_telegram(
    client_id: String,
    session_id: String,
    api_id: i32,
    api_hash: String,
) -> Result<AuthState, Failure> {
    let runtime = lease(&client_id).await?;
    Ok(runtime
        .backend
        .connect_native(id(&session_id)?, api_id, Zeroizing::new(api_hash))
        .await
        .map_err(Failure::from)?
        .into())
}

pub async fn auth_status(client_id: String, session_id: String) -> Result<AuthState, Failure> {
    Ok(lease(&client_id)
        .await?
        .backend
        .auth_status(id(&session_id)?)
        .await
        .map_err(Failure::from)?
        .into())
}

pub async fn authenticate(
    client_id: String,
    session_id: String,
    request_id: String,
    step: LoginStep,
    value: String,
) -> Result<AuthState, Failure> {
    let runtime = lease(&client_id).await?;
    let operation = runtime.operation(&request_id)?;
    let input = match step {
        LoginStep::Phone => LoginInput::Phone(value),
        LoginStep::Code => LoginInput::Code(value),
        LoginStep::Password => LoginInput::Password(value),
        LoginStep::Qr => LoginInput::Qr,
        LoginStep::Email => LoginInput::Email(value),
        LoginStep::EmailCode => LoginInput::EmailCode(value),
        LoginStep::ResendCode => LoginInput::ResendCode,
    };
    Ok(runtime
        .backend
        .authenticate(id(&session_id)?, input.into(), &operation.cancel)
        .await
        .map_err(Failure::from)?
        .into())
}

pub async fn reconnect(client_id: String, session_id: String) -> Result<AuthState, Failure> {
    Ok(lease(&client_id)
        .await?
        .backend
        .reconnect(id(&session_id)?)
        .await
        .map_err(Failure::from)?
        .into())
}
pub async fn discover_vaults(
    client_id: String,
    session_id: String,
    request_id: String,
) -> Result<Vec<Vault>, Failure> {
    let runtime = lease(&client_id).await?;
    let operation = runtime.operation(&request_id)?;
    Ok(runtime
        .backend
        .discover_vaults(id(&session_id)?, &operation.cancel)
        .await
        .map_err(Failure::from)?
        .into_iter()
        .map(|v| Vault {
            id: v.init.vault_id.to_string(),
            chat_id: v.channel.chat_id.to_string(),
            title: v.channel.title,
            protocol: v.protocol,
        })
        .collect())
}
pub async fn attach_vault(
    client_id: String,
    session_id: String,
    request_id: String,
    chat_id: String,
) -> Result<RuntimeStatus, Failure> {
    let runtime = lease(&client_id).await?;
    let operation = runtime.operation(&request_id)?;
    let chat = chat_id
        .parse()
        .map_err(|_| failure(ErrorCode::InvalidInput))?;
    Ok(runtime
        .backend
        .attach_vault(id(&session_id)?, chat, &operation.cancel)
        .await
        .map_err(Failure::from)?
        .into())
}
pub async fn create_vault(
    client_id: String,
    session_id: String,
    request_id: String,
    title: String,
) -> Result<RuntimeStatus, Failure> {
    let runtime = lease(&client_id).await?;
    let operation = runtime.operation(&request_id)?;
    Ok(runtime
        .backend
        .create_vault_named(id(&session_id)?, Some(title), &operation.cancel)
        .await
        .map_err(Failure::from)?
        .into())
}
pub async fn disconnect(client_id: String, session_id: String) -> Result<(), Failure> {
    lease(&client_id)
        .await?
        .backend
        .disconnect(id(&session_id)?)
        .await
        .map_err(Into::into)
}
pub async fn logout(client_id: String, session_id: String) -> Result<(), Failure> {
    lease(&client_id)
        .await?
        .backend
        .logout(id(&session_id)?)
        .await
        .map_err(Into::into)
}

impl From<vault_telegram::CodeDelivery> for Delivery {
    fn from(value: vault_telegram::CodeDelivery) -> Self {
        use vault_telegram::CodeDelivery as D;
        match value {
            D::TelegramMessage => Self::Telegram,
            D::Sms => Self::Sms,
            D::SmsWord => Self::SmsWord,
            D::SmsPhrase => Self::SmsPhrase,
            D::Call => Self::Call,
            D::FlashCall => Self::FlashCall,
            D::MissedCall => Self::MissedCall,
            D::Fragment => Self::Fragment,
            D::FirebaseAndroid => Self::FirebaseAndroid,
            D::FirebaseIos => Self::FirebaseIos,
            D::Unknown => Self::Unknown,
        }
    }
}
impl From<vault_telegram::AuthStatus> for AuthState {
    fn from(value: vault_telegram::AuthStatus) -> Self {
        use vault_telegram::AuthStatus as S;
        let mut state = Self {
            phase: AuthPhase::Unsupported,
            qr_link: None,
            expected_length: None,
            timeout_seconds: None,
            delivery: None,
            next_delivery: None,
        };
        state.phase = match value {
            S::ParametersRequired => AuthPhase::Parameters,
            S::PhoneRequired => AuthPhase::Phone,
            S::CodeRequired { code_info } => {
                state.expected_length = code_info.expected_length;
                state.timeout_seconds = code_info.timeout_seconds;
                state.delivery = Some(code_info.delivery.into());
                state.next_delivery = code_info.next_delivery.map(Into::into);
                AuthPhase::Code
            }
            S::PasswordRequired => AuthPhase::Password,
            S::QrConfirmation { link } => {
                state.qr_link = Some(link);
                AuthPhase::Qr
            }
            S::EmailRequired => AuthPhase::Email,
            S::EmailCodeRequired { expected_length } => {
                state.expected_length = expected_length;
                AuthPhase::EmailCode
            }
            S::RegistrationRequired => AuthPhase::Registration,
            S::PremiumRequired => AuthPhase::Premium,
            S::Ready => AuthPhase::Ready,
            S::LoggingOut => AuthPhase::LoggingOut,
            S::Closing => AuthPhase::Closing,
            S::Closed => AuthPhase::Closed,
            S::Unsupported => AuthPhase::Unsupported,
        };
        state
    }
}
