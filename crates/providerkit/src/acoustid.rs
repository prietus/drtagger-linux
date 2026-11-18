//! AcoustID lookup (https://acoustid.org/webservice). The fingerprint goes
//! in a form-encoded POST body, as AcoustID recommends: long fingerprints
//! overflow URL limits. Three requests a second per client key.

use crate::error::{ProviderError, Result};
use crate::http::{RateLimiter, Request, Transport, default_transport};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

pub const LOOKUP_URL: &str = "https://api.acoustid.org/v2/lookup";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcoustIDMatch {
    /// 0…1.
    pub score: f64,
    /// MusicBrainz recording MBID.
    #[serde(rename = "recordingID")]
    pub recording_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_title: Option<String>,
    /// Releases containing the recording.
    #[serde(rename = "releaseIDs")]
    pub release_ids: Vec<String>,
    /// The AcoustID track id, for ACOUSTID_ID.
    #[serde(rename = "acoustID", default, skip_serializing_if = "Option::is_none")]
    pub acoust_id: Option<String>,
}

pub struct AcoustIDClient {
    pub client_key: String,
    pub user_agent: String,
    transport: Arc<dyn Transport>,
    limiter: RateLimiter,
}

/// `addingPercentEncoding(withAllowedCharacters: urlQueryAllowed − "&=?")`.
fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!$'()*+,;:@/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl AcoustIDClient {
    pub fn new(client_key: &str, user_agent: &str) -> AcoustIDClient {
        AcoustIDClient::with_transport(client_key, user_agent, default_transport())
    }

    pub fn with_transport(client_key: &str, user_agent: &str, transport: Arc<dyn Transport>) -> AcoustIDClient {
        AcoustIDClient {
            client_key: client_key.into(),
            user_agent: user_agent.into(),
            transport,
            limiter: RateLimiter::new(Duration::from_millis(350)),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.client_key.is_empty()
    }

    pub(crate) fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    /// The lookup body; the recordings key on it (with the key blanked).
    pub fn lookup_body(client_key: &str, fingerprint: &str, duration_seconds: i64) -> String {
        [
            ("client", client_key.to_string()),
            ("meta", "recordings releaseids".to_string()),
            ("duration", duration_seconds.to_string()),
            ("fingerprint", fingerprint.to_string()),
        ]
        .iter()
        .map(|(k, v)| format!("{k}={}", form_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
    }

    /// Matches by score, each recording with the releases containing it.
    pub fn lookup(&self, fingerprint: &str, duration_seconds: i64) -> Result<Vec<AcoustIDMatch>> {
        if fingerprint.is_empty() {
            return Ok(vec![]);
        }
        let body = Self::lookup_body(&self.client_key, fingerprint, duration_seconds);
        let request = Request::post(LOOKUP_URL, body.into_bytes())
            .header("User-Agent", self.user_agent.clone())
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded");
        self.limiter.wait(self.transport.is_live());
        let response = self.transport.send(&request)?;
        Self::parse(response.status, &response.body)
    }

    pub fn parse(status: u16, data: &[u8]) -> Result<Vec<AcoustIDMatch>> {
        // AcoustID explains its 4xx answers ("invalid fingerprint"…).
        if status >= 400 {
            if let Ok(decoded) = serde_json::from_slice::<LookupResponse>(data)
                && let Some(err) = decoded.error
            {
                return Err(ProviderError::AcoustId(format!("{} (HTTP {status})", err.message)));
            }
            return Err(ProviderError::Http(status));
        }
        let decoded: LookupResponse = serde_json::from_slice(data)?;
        if decoded.status != "ok" {
            return Err(ProviderError::AcoustId(decoded.error.map(|e| e.message).unwrap_or(decoded.status)));
        }
        let mut matches = vec![];
        for result in decoded.results.unwrap_or_default() {
            let score = result.score.unwrap_or(0.0);
            for recording in result.recordings.unwrap_or_default() {
                // Fingerprints matched to recordings without an MBID lead
                // to no release.
                let Some(rec_id) = recording.id.filter(|i| !i.is_empty()) else { continue };
                let mut release_ids: Vec<String> =
                    recording.releaseids.unwrap_or_default().into_iter().filter(|i| !i.is_empty()).collect();
                if release_ids.is_empty() {
                    release_ids = recording
                        .releases
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|r| r.id)
                        .filter(|i| !i.is_empty())
                        .collect();
                }
                matches.push(AcoustIDMatch {
                    score,
                    recording_id: rec_id,
                    recording_title: recording.title,
                    release_ids,
                    acoust_id: result.id.clone(),
                });
            }
        }
        Ok(matches)
    }
}

#[derive(Deserialize)]
struct LookupResponse {
    status: String,
    results: Option<Vec<LookupResult>>,
    error: Option<ErrorPayload>,
}

#[derive(Deserialize)]
struct LookupResult {
    id: Option<String>,
    score: Option<f64>,
    recordings: Option<Vec<Recording>>,
}

#[derive(Deserialize)]
struct Recording {
    id: Option<String>,
    title: Option<String>,
    releaseids: Option<Vec<String>>,
    releases: Option<Vec<ReleaseRef>>,
}

#[derive(Deserialize)]
struct ReleaseRef {
    id: Option<String>,
}

#[derive(Deserialize)]
struct ErrorPayload {
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_is_form_encoded() {
        assert_eq!(
            AcoustIDClient::lookup_body("k3y", "AQAA_-x", 245),
            "client=k3y&meta=recordings%20releaseids&duration=245&fingerprint=AQAA_-x"
        );
    }

    #[test]
    fn parses_matches_and_errors() {
        let ok = br#"{"status":"ok","results":[{"id":"ac-1","score":0.97,"recordings":[{"id":"rec-1","title":"So What","releaseids":["r1","r2"]},{"title":"no mbid"}]},{"id":"ac-2","score":0.5,"recordings":[{"id":"rec-2","releases":[{"id":"r3"}]}]}]}"#;
        let m = AcoustIDClient::parse(200, ok).unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].release_ids, ["r1", "r2"]);
        assert_eq!(m[0].acoust_id.as_deref(), Some("ac-1"));
        assert_eq!(m[1].release_ids, ["r3"]);
        let bad = br#"{"status":"error","error":{"code":3,"message":"invalid fingerprint"}}"#;
        assert_eq!(
            AcoustIDClient::parse(400, bad),
            Err(ProviderError::AcoustId("invalid fingerprint (HTTP 400)".into()))
        );
    }
}
