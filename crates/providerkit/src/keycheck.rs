//! Testing an API key against its service, for the settings screen.

use crate::acoustid::{AcoustIDClient, LOOKUP_URL};
use crate::art::FanartClient;
use crate::discogs::DiscogsClient;
use crate::http::{Request, with_query};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    /// "Key accepted", "Signed in as carlos".
    Valid(String),
    /// The service rejected the key.
    Invalid(String),
    /// Network or server trouble; says nothing about the key.
    Unreachable(String),
}

impl KeyCheck {
    pub fn message(&self) -> &str {
        match self {
            KeyCheck::Valid(m) | KeyCheck::Invalid(m) | KeyCheck::Unreachable(m) => m,
        }
    }
}

impl AcoustIDClient {
    /// A lookup with a junk fingerprint: a bad key answers error code 4;
    /// a good one complains about the fingerprint instead.
    pub fn validate_key(&self) -> KeyCheck {
        if self.client_key.is_empty() {
            return KeyCheck::Invalid("No key entered".into());
        }
        let key: String = self
            .client_key
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let request =
            Request::post(LOOKUP_URL, format!("client={key}&format=json&duration=1&fingerprint=AQAAAA").into_bytes())
                .header("User-Agent", self.user_agent.clone())
                .header("Content-Type", "application/x-www-form-urlencoded")
                .timeout(15);
        match self.transport().send(&request) {
            Err(e) => KeyCheck::Unreachable(e.to_string()),
            Ok(r) => {
                let Ok(root) = serde_json::from_slice::<serde_json::Value>(&r.body) else {
                    return KeyCheck::Unreachable("Unexpected answer from AcoustID".into());
                };
                if root.get("status").and_then(|s| s.as_str()) == Some("ok") {
                    return KeyCheck::Valid("Key accepted".into());
                }
                let error = root.get("error");
                let code = error.and_then(|e| e.get("code")).and_then(|c| c.as_i64()).unwrap_or(0);
                let message = error.and_then(|e| e.get("message")).and_then(|m| m.as_str()).unwrap_or("error");
                if code == 4 {
                    KeyCheck::Invalid(message.into())
                } else {
                    KeyCheck::Valid(format!("Key accepted ({message})"))
                }
            }
        }
    }
}

impl DiscogsClient {
    /// /oauth/identity names the account behind a personal token.
    pub fn validate_token(&self) -> KeyCheck {
        if self.token.is_empty() {
            return KeyCheck::Invalid("No token entered".into());
        }
        let mut request = self.request("https://api.discogs.com/oauth/identity").timeout(15);
        request.headers.retain(|(k, _)| k != "Accept");
        match self.transport().send(&request) {
            Err(e) => KeyCheck::Unreachable(e.to_string()),
            Ok(r) => {
                let user = serde_json::from_slice::<serde_json::Value>(&r.body)
                    .ok()
                    .and_then(|v| v.get("username").and_then(|u| u.as_str()).map(String::from));
                match (r.status, user) {
                    (200, Some(u)) => KeyCheck::Valid(format!("Signed in as {u}")),
                    (401 | 403, _) => KeyCheck::Invalid(format!("Discogs rejected the token (HTTP {})", r.status)),
                    (s, _) => KeyCheck::Unreachable(format!("Discogs answered HTTP {s}")),
                }
            }
        }
    }
}

impl FanartClient {
    /// Any artist request works as a probe: a bad key is refused outright.
    pub fn validate_key(&self) -> KeyCheck {
        if self.api_key.is_empty() {
            return KeyCheck::Invalid("No key entered".into());
        }
        let url = with_query(
            "https://webservice.fanart.tv/v3/music/561d854a-6a28-4aa7-8c99-323e6ce46c2a",
            &[("api_key", &self.api_key)],
        );
        let request = Request::get(url).header("User-Agent", self.user_agent.clone()).timeout(15);
        match self.transport().send(&request) {
            Err(e) => KeyCheck::Unreachable(e.to_string()),
            Ok(r) => match r.status {
                200 | 404 => KeyCheck::Valid("Key accepted".into()),
                401 | 403 => {
                    let msg = serde_json::from_slice::<serde_json::Value>(&r.body)
                        .ok()
                        .and_then(|v| v.get("error message").and_then(|m| m.as_str()).map(String::from));
                    KeyCheck::Invalid(msg.unwrap_or(format!("fanart.tv rejected the key (HTTP {})", r.status)))
                }
                s => KeyCheck::Unreachable(format!("fanart.tv answered HTTP {s}")),
            },
        }
    }
}
