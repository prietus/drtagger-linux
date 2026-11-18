//! A small HTTP server on 127.0.0.1 that speaks CDDB, so a ripper with a
//! configurable freedb server (XLD, EAC, CUERipper, abcde…) can ask
//! drtagger for disc metadata: server localhost, port `port`, path
//! /~cddb/cddb.cgi. One request per connection, HTTP/1.0 style.

use identifykit::CDDBResponder;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 8880;

pub struct CddbServer {
    pub port: u16,
    running: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
}

impl CddbServer {
    /// Starts listening; answers come from MusicBrainz's fuzzy TOC lookup.
    pub fn start(port: u16) -> std::io::Result<CddbServer> {
        Self::start_with(port, Arc::new(CDDBResponder::live(crate::identify::USER_AGENT)))
    }

    pub fn start_with(port: u16, responder: Arc<CDDBResponder>) -> std::io::Result<CddbServer> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let running = Arc::new(AtomicBool::new(true));
        let requests = Arc::new(AtomicUsize::new(0));
        let (r, n) = (running.clone(), requests.clone());
        std::thread::Builder::new().name("drtagger-cddb".into()).spawn(move || {
            while r.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let responder = responder.clone();
                        let n = n.clone();
                        std::thread::spawn(move || {
                            serve(stream, &responder);
                            n.fetch_add(1, Ordering::Relaxed);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(100))
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        })?;
        Ok(CddbServer { port, running, requests })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl Drop for CddbServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The request head (and a POST body up to its Content-Length).
fn read_request(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut data = vec![];
    let mut buf = [0u8; 8192];
    while data.len() < 64 * 1024 {
        let Ok(n) = stream.read(&mut buf) else { break };
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..end]).to_lowercase();
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if data.len() >= end + 4 + length {
                break;
            }
        }
    }
    // UTF-8, else Latin-1 (as the Mac server reads it).
    String::from_utf8(data.clone()).unwrap_or_else(|_| data.iter().map(|&b| b as char).collect())
}

fn serve(mut stream: TcpStream, responder: &CDDBResponder) {
    let _ = stream.set_nonblocking(false);
    let request = read_request(&mut stream);
    let (status, body) = match query_from_request(&request) {
        Some(q) => ("200 OK", responder.respond(&q)),
        None => ("400 Bad Request", "500 Command syntax error.\n".to_string()),
    };
    let response = format!(
        "HTTP/1.0 {status}\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.shutdown(Shutdown::Both);
}

/// "GET /~cddb/cddb.cgi?cmd=… HTTP/1.1" → "cmd=…". POST bodies carry the
/// same form after the blank line.
pub fn query_from_request(request: &str) -> Option<String> {
    let first = request.split("\r\n").next()?;
    let parts: Vec<&str> = first.split(' ').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    if parts[0] == "POST"
        && let Some((_, body)) = request.split_once("\r\n\r\n")
    {
        let body = body.trim();
        return if body.is_empty() { None } else { Some(body.to_string()) };
    }
    parts[1].split_once('?').map(|(_, q)| q.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use providerkit::{Candidate, CandidateMedium, CandidateTrack, Source};

    #[test]
    fn requests_and_answers() {
        assert_eq!(
            query_from_request("GET /~cddb/cddb.cgi?cmd=cddb+lscat&proto=6 HTTP/1.1\r\nHost: x\r\n\r\n").as_deref(),
            Some("cmd=cddb+lscat&proto=6")
        );
        assert_eq!(
            query_from_request("POST /~cddb/cddb.cgi HTTP/1.1\r\nContent-Length: 13\r\n\r\ncmd=proto+6\r\n").as_deref(),
            Some("cmd=proto+6")
        );
        assert_eq!(query_from_request("garbage"), None);

        let tracks: Vec<CandidateTrack> = [200, 180, 240]
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let mut t = CandidateTrack::new(i as i64 + 1, &format!("T{}", i + 1));
                t.duration_ms = Some(d * 1000);
                t
            })
            .collect();
        let mut c = Candidate::new(Source::Musicbrainz, "one", "A", "B");
        c.media = vec![CandidateMedium::new(1, Some("CD".into()), None, tracks, vec![], None)];
        let responder = Arc::new(CDDBResponder::new(Box::new(move |_| vec![c.clone()])));
        let server = CddbServer::start_with(0, responder).unwrap();
        let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        s.write_all(b"GET /~cddb/cddb.cgi?cmd=cddb+query+2a024903+3+150+15150+28650+622&proto=6 HTTP/1.0\r\n\r\n")
            .unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.0 200 OK\r\n"), "{answer}");
        assert!(answer.ends_with("\r\n\r\n200 rock 2a024903 B / A\n"), "{answer}");
        server.stop();
    }
}
