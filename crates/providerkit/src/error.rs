//! Transport-level failures shared by every provider client. Service
//! errors with their own wording (AcoustID's messages, CTDB's bad answers)
//! stay with their clients.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    InvalidUrl,
    Http(u16),
    RateLimited(Option<i64>),
    /// The request never got an answer (DNS, TLS, timeout…).
    Transport(String),
    /// The answer was not what the service documents.
    Decode(String),
    /// AcoustID's own error message.
    AcoustId(String),
    /// CUETools DB answered something that is not its XML.
    CtdbInvalidResponse,
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::InvalidUrl => write!(f, "Could not build a valid request URL."),
            ProviderError::Http(code) => write!(f, "The service returned HTTP {code}."),
            ProviderError::RateLimited(Some(secs)) => write!(f, "Rate limited by the service; retry in {secs} s."),
            ProviderError::RateLimited(None) => write!(f, "Rate limited by the service."),
            ProviderError::Transport(e) => write!(f, "{e}"),
            // Foundation's wording for a DecodingError; the detail is for logs.
            ProviderError::Decode(_) => {
                write!(f, "The data couldn’t be read because it isn’t in the correct format.")
            }
            ProviderError::AcoustId(e) => write!(f, "AcoustID service error: {e}"),
            ProviderError::CtdbInvalidResponse => write!(f, "CUETools DB returned an unexpected response."),
        }
    }
}

impl std::error::Error for ProviderError {}

impl From<serde_json::Error> for ProviderError {
    fn from(e: serde_json::Error) -> Self {
        ProviderError::Decode(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ProviderError>;
