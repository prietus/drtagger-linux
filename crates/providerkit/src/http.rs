//! HTTP plumbing shared by the clients: a replaceable [`Transport`] (live
//! HTTPS, or recordings for offline parity runs), Foundation-compatible
//! URL encoding, and the slot-reserving rate limiter the Mac clients use.

use crate::error::{ProviderError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Request {
        Request { method: "GET".into(), url: url.into(), headers: vec![], body: None, timeout: Duration::from_secs(20) }
    }

    pub fn post(url: impl Into<String>, body: Vec<u8>) -> Request {
        Request {
            method: "POST".into(),
            url: url.into(),
            headers: vec![],
            body: Some(body),
            timeout: Duration::from_secs(20),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Request {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn timeout(mut self, secs: u64) -> Request {
        self.timeout = Duration::from_secs(secs);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait Transport: Send + Sync {
    fn send(&self, request: &Request) -> Result<Response>;

    /// False for recordings: the clients then skip their rate-limit waits.
    fn is_live(&self) -> bool {
        true
    }
}

/// HTTPS through ureq. Statuses ≥ 400 come back as responses; the clients
/// decide what they mean.
pub struct LiveTransport {
    agent: ureq::Agent,
}

impl Default for LiveTransport {
    fn default() -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent("")
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        LiveTransport { agent }
    }
}

impl Transport for LiveTransport {
    fn send(&self, request: &Request) -> Result<Response> {
        let err = |e: ureq::Error| ProviderError::Transport(e.to_string());
        let mut response = if request.method == "POST" {
            let mut b = self.agent.post(&request.url);
            for (k, v) in &request.headers {
                b = b.header(k, v);
            }
            b.config()
                .timeout_global(Some(request.timeout))
                .build()
                .send(request.body.as_deref().unwrap_or_default())
                .map_err(err)?
        } else {
            let mut b = self.agent.get(&request.url);
            for (k, v) in &request.headers {
                b = b.header(k, v);
            }
            b.config().timeout_global(Some(request.timeout)).build().call().map_err(err)?
        };
        let status = response.status().as_u16();
        let body = response.body_mut().with_config().limit(64 << 20).read_to_vec().map_err(err)?;
        Ok(Response { status, body })
    }
}

fn default_slot() -> &'static RwLock<Option<Arc<dyn Transport>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn Transport>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// The transport new clients get: live HTTPS unless replaced (parity runs).
pub fn default_transport() -> Arc<dyn Transport> {
    if let Some(t) = default_slot().read().unwrap().clone() {
        return t;
    }
    let mut slot = default_slot().write().unwrap();
    slot.get_or_insert_with(|| Arc::new(LiveTransport::default())).clone()
}

pub fn set_default_transport(transport: Arc<dyn Transport>) {
    *default_slot().write().unwrap() = Some(transport);
}

// MARK: - URL encoding

/// Bytes `URLComponents` leaves alone in a query name or value: the
/// unreserved set plus `!$'()*+,;:@/?` (`&`, `=` and `#` are encoded).
fn query_allowed(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!$'()*+,;:@/?".contains(&b)
}

/// Bytes a path segment keeps (`appendingPathComponent`).
fn path_allowed(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b)
}

fn encode(s: &str, allowed: fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if allowed(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn encode_query(s: &str) -> String {
    encode(s, query_allowed)
}

pub fn encode_path(s: &str) -> String {
    encode(s, path_allowed)
}

/// `base?name=value&…` as `URLComponents.queryItems` builds it.
pub fn with_query(base: &str, items: &[(&str, &str)]) -> String {
    if items.is_empty() {
        return base.to_string();
    }
    let q: Vec<String> = items.iter().map(|(k, v)| format!("{}={}", encode_query(k), encode_query(v))).collect();
    format!("{base}?{}", q.join("&"))
}

// MARK: - Rate limiting

/// Reserves a slot before waiting, so concurrent callers each get their
/// own, `interval` apart.
pub struct RateLimiter {
    interval: Duration,
    next: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn new(interval: Duration) -> RateLimiter {
        RateLimiter { interval, next: Mutex::new(None) }
    }

    pub fn wait(&self, live: bool) {
        if !live {
            return;
        }
        let slot = {
            let mut next = self.next.lock().unwrap();
            let now = Instant::now();
            let slot = next.map_or(now, |n| n.max(now));
            *next = Some(slot + self.interval);
            slot
        };
        let now = Instant::now();
        if slot > now {
            std::thread::sleep(slot - now);
        }
    }

    /// After a 503: nothing before `interval` from now.
    pub fn push_back(&self) {
        *self.next.lock().unwrap() = Some(Instant::now() + self.interval);
    }
}

// MARK: - Recordings

/// The identity of a request in a recording: method, URL and body with the
/// API keys blanked (AcoustID's `client`, fanart.tv's `api_key`), so
/// recordings carry no secrets and replay without them. drtagger for
/// Mac's parity dumper computes the same key.
pub fn canonical(request: &Request) -> String {
    let url = redact(&request.url, "api_key");
    let body = request.body.as_ref().map(|b| redact(&String::from_utf8_lossy(b), "client")).unwrap_or_default();
    format!("{} {}\n{}", request.method, url, body)
}

fn redact(s: &str, name: &str) -> String {
    let re = regex::Regex::new(&format!(r"(^|[?&]){}=[^&]*", regex::escape(name))).unwrap();
    re.replace_all(s, format!("${{1}}{name}=<KEY>")).into_owned()
}

pub fn recording_key(request: &Request) -> String {
    hex::encode(Sha256::digest(canonical(request).as_bytes()))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Recorded {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub body: String,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    #[serde(default, rename = "responseBase64", skip_serializing_if = "Option::is_none")]
    pub response_base64: Option<String>,
}

impl Recorded {
    pub fn response(&self) -> Response {
        use base64::Engine;
        let body = match (&self.response, &self.response_base64) {
            (Some(s), _) => s.clone().into_bytes(),
            (None, Some(b)) => base64::engine::general_purpose::STANDARD.decode(b).unwrap_or_default(),
            _ => vec![],
        };
        Response { status: self.status, body }
    }

    fn from(request: &Request, response: &Response) -> Recorded {
        use base64::Engine;
        let canon = canonical(request);
        let (head, body) = canon.split_once('\n').unwrap_or((&canon, ""));
        let url = head.split_once(' ').map_or("", |x| x.1).to_string();
        let (text, b64) = match std::str::from_utf8(&response.body) {
            Ok(s) => (Some(s.to_string()), None),
            Err(_) => (None, Some(base64::engine::general_purpose::STANDARD.encode(&response.body))),
        };
        Recorded {
            method: request.method.clone(),
            url,
            body: body.to_string(),
            status: response.status,
            response: text,
            response_base64: b64,
        }
    }
}

/// Sends through `inner` and writes every exchange to `dir/<key>.json`.
pub struct Recorder {
    inner: Arc<dyn Transport>,
    dir: PathBuf,
}

impl Recorder {
    pub fn new(inner: Arc<dyn Transport>, dir: &Path) -> std::io::Result<Recorder> {
        std::fs::create_dir_all(dir)?;
        Ok(Recorder { inner, dir: dir.to_path_buf() })
    }
}

impl Transport for Recorder {
    fn send(&self, request: &Request) -> Result<Response> {
        let response = self.inner.send(request)?;
        let rec = Recorded::from(request, &response);
        let path = self.dir.join(format!("{}.json", recording_key(request)));
        let _ = std::fs::write(path, serde_json::to_vec_pretty(&rec).unwrap());
        Ok(response)
    }

    fn is_live(&self) -> bool {
        self.inner.is_live()
    }
}

/// Picks a recording for a request that has no exact match (an AcoustID
/// lookup whose fingerprint differs in a few bits); returns its key.
pub type FuzzyMatcher = Box<dyn Fn(&Request, &[(String, Recorded)]) -> Option<String> + Send + Sync>;

/// Answers from recordings. A request with none fails as a transport
/// error, or goes to `fallback` (and is recorded) when there is one.
pub struct Replayer {
    entries: Vec<(String, Recorded)>,
    fuzzy: Option<FuzzyMatcher>,
    fallback: Option<Recorder>,
    /// Answers the fallback fetched, so a repeated request is not refetched.
    fetched: Mutex<std::collections::HashMap<String, Response>>,
    misses: Mutex<Vec<String>>,
}

impl Replayer {
    pub fn load(dir: &Path) -> std::io::Result<Replayer> {
        let mut entries = vec![];
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "json")
                && let Ok(rec) = serde_json::from_slice::<Recorded>(&std::fs::read(&p)?)
            {
                let key = p.file_stem().unwrap().to_string_lossy().into_owned();
                entries.push((key, rec));
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Replayer {
            entries,
            fuzzy: None,
            fallback: None,
            fetched: Mutex::new(Default::default()),
            misses: Mutex::new(vec![]),
        })
    }

    pub fn with_fuzzy(mut self, matcher: FuzzyMatcher) -> Replayer {
        self.fuzzy = Some(matcher);
        self
    }

    pub fn with_fallback(mut self, fallback: Recorder) -> Replayer {
        self.fallback = Some(fallback);
        self
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Requests that had no recording, as their canonical text.
    pub fn misses(&self) -> Vec<String> {
        self.misses.lock().unwrap().clone()
    }
}

impl Transport for Replayer {
    fn send(&self, request: &Request) -> Result<Response> {
        let key = recording_key(request);
        if let Ok(i) = self.entries.binary_search_by(|e| e.0.as_str().cmp(&key)) {
            return Ok(self.entries[i].1.response());
        }
        if let Some(k) = self.fuzzy.as_ref().and_then(|f| f(request, &self.entries))
            && let Ok(i) = self.entries.binary_search_by(|e| e.0.as_str().cmp(&k))
        {
            return Ok(self.entries[i].1.response());
        }
        if let Some(r) = self.fetched.lock().unwrap().get(&key) {
            return Ok(r.clone());
        }
        self.misses.lock().unwrap().push(canonical(request));
        match &self.fallback {
            Some(f) => {
                let r = f.send(request)?;
                // Server errors are worth retrying; anything else is the answer.
                if r.status < 500 {
                    self.fetched.lock().unwrap().insert(key, r.clone());
                }
                Ok(r)
            }
            None => Err(ProviderError::Transport(format!("No recording for {} {}", request.method, request.url))),
        }
    }

    /// With a network fallback the clients keep their rate limits.
    fn is_live(&self) -> bool {
        self.fallback.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encoding_matches_url_components() {
        let all: String = (32u8..127).map(|b| b as char).collect::<String>() + "ñé日";
        // Captured from Swift's URLComponents.queryItems on macOS.
        assert_eq!(
            with_query("https://x.org/ws/2/release", &[("q", &all)]),
            "https://x.org/ws/2/release?q=%20!%22%23$%25%26'()*+,-./0123456789:;%3C%3D%3E?@ABCDEFGHIJKLMNOPQRSTUVWXYZ%5B%5C%5D%5E_%60abcdefghijklmnopqrstuvwxyz%7B%7C%7D~%C3%B1%C3%A9%E6%97%A5"
        );
        assert_eq!(encode_path("a b?#ñ"), "a%20b%3F%23%C3%B1");
    }

    #[test]
    fn canonical_requests_carry_no_keys() {
        let r = Request::get("https://webservice.fanart.tv/v3/music/albums/rg?api_key=SECRET");
        assert_eq!(canonical(&r), "GET https://webservice.fanart.tv/v3/music/albums/rg?api_key=<KEY>\n");
        let p = Request::post(
            "https://api.acoustid.org/v2/lookup",
            b"client=SECRET&meta=recordings%20releaseids&duration=1&fingerprint=AQAA".to_vec(),
        );
        assert_eq!(
            canonical(&p),
            "POST https://api.acoustid.org/v2/lookup\nclient=<KEY>&meta=recordings%20releaseids&duration=1&fingerprint=AQAA"
        );
    }

    struct Fixed;
    impl Transport for Fixed {
        fn send(&self, _: &Request) -> Result<Response> {
            Ok(Response { status: 200, body: b"{\"ok\":1}".to_vec() })
        }
    }

    #[test]
    fn recordings_replay() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Recorder::new(Arc::new(Fixed), dir.path()).unwrap();
        let req = Request::get("https://musicbrainz.org/ws/2/release/x?fmt=json");
        rec.send(&req).unwrap();
        let replay = Replayer::load(dir.path()).unwrap();
        assert_eq!(replay.send(&req).unwrap().body, b"{\"ok\":1}");
        assert!(replay.send(&Request::get("https://other")).is_err());
        assert_eq!(replay.misses().len(), 1);
    }
}
