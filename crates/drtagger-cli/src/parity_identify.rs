//! Identification parity dump: the same JSON as `MacDump identify`
//! (parity/MacDump/Sources/MacDump/Identify.swift), answered from the HTTP
//! exchanges MacDump recorded. AcoustID lookups whose fingerprint differs in
//! a few bits from the recorded one (KissFFT here, vDSP there) are matched
//! to the recording of the same track by fingerprint similarity.

use identifykit::*;
use librarykit::{DetectedAlbum, LibraryScanner};
use providerkit::http::{LiveTransport, Recorded, Recorder, Replayer, Request, set_default_transport};
use providerkit::{CUEToolsDBClient, acoustid::LOOKUP_URL};
use serde_json::{Value, json};
use splitkit::FFmpegTool;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tagkit::{PicardMapper, TagContext};

const UA: &str = "drtagger-parity/0.1 (+https://drtagger.priet.us)";

fn form(body: &str) -> HashMap<String, String> {
    body.split('&').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Picks the recorded AcoustID lookup of the same duration whose
/// fingerprint is closest; remembers how close it was.
fn acoustid_matcher(similarities: Arc<Mutex<Vec<f64>>>) -> providerkit::http::FuzzyMatcher {
    Box::new(move |request: &Request, entries: &[(String, Recorded)]| {
        if request.method != "POST" || request.url != LOOKUP_URL {
            return None;
        }
        let mine = form(&String::from_utf8_lossy(request.body.as_deref()?));
        let raw = chromaprintkit::decode(mine.get("fingerprint")?)?;
        let mut best: Option<(f64, &String)> = None;
        for (key, rec) in entries.iter().filter(|(_, r)| r.method == "POST" && r.url == LOOKUP_URL) {
            let theirs = form(&rec.body);
            if theirs.get("duration") != mine.get("duration") {
                continue;
            }
            let Some(other) = theirs.get("fingerprint").and_then(|f| chromaprintkit::decode(f)) else { continue };
            let s = chromaprintkit::similarity(&raw, &other, 2);
            if best.is_none_or(|(b, _)| s > b) {
                best = Some((s, key));
            }
        }
        let (s, key) = best?;
        similarities.lock().unwrap().push(s);
        (s >= 0.9).then(|| key.clone())
    })
}

fn result_json(r: &IdentificationResult) -> Value {
    let mut signals = serde_json::to_value(&r.signals).unwrap();
    signals["tracks"] = r
        .signals
        .tracks
        .iter()
        .map(|t| {
            let mut d = serde_json::to_value(t).unwrap();
            d["url"] = t
                .url
                .as_ref()
                .and_then(|u| u.file_name())
                .map(|n| Value::from(n.to_string_lossy().into_owned()))
                .unwrap_or(Value::Null);
            d
        })
        .collect();
    signals.as_object_mut().unwrap().remove("artworkScans");
    let mut out = json!({
        "signals": signals,
        "candidates": r.candidates,
        "discogsCandidates": r.discogs_candidates,
        "log": r.log,
        "isConfident": r.is_confident(),
        "isClearFingerprintMatch": r.is_clear_fingerprint_match(),
        "isClearBarcodeMatch": r.is_clear_barcode_match(),
        "coverContenders": r.cover_contenders().iter().map(|c| c.id()).collect::<Vec<_>>(),
    });
    if let Some(f) = &r.fingerprints {
        out["fingerprints"] = serde_json::to_value(f).unwrap();
    }
    if let Some(x) = r.runner_up() {
        out["runnerUp"] = x.id().into();
    }
    if let Some(x) = r.album_only_pick() {
        out["albumOnlyPick"] = x.id().into();
    }
    out
}

fn raw_fingerprints(tool: &FFmpegTool, r: &IdentificationResult) -> Vec<Value> {
    r.signals
        .tracks
        .iter()
        .map(|t| {
            let Some(url) = t.url.as_ref().filter(|u| !u.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso")))
            else {
                return Value::Null;
            };
            let Ok((samples, rate, channels)) = tool.decode_snippet(url, t.image_start.unwrap_or(0.0), 120.0) else {
                return Value::Null;
            };
            if samples.is_empty() {
                return Value::Null;
            }
            let seconds = t.duration_seconds.unwrap_or((samples.len() as i64 / channels.max(1)) as f64 / rate as f64);
            match chromaprintkit::fingerprint(&samples, rate, channels, (seconds.round() as i64).max(1)) {
                Ok(fp) => {
                    json!({ "sampleRate": rate, "channels": channels, "duration": fp.duration_seconds, "raw": fp.raw })
                }
                Err(_) => Value::Null,
            }
        })
        .collect()
}

pub fn dump(roots: &[PathBuf], ffmpeg_dir: &Path, replay_dir: &Path, fallback_live: bool) -> Value {
    let similarities = Arc::new(Mutex::new(vec![]));
    let mut replayer =
        Replayer::load(replay_dir).expect("recordings").with_fuzzy(acoustid_matcher(similarities.clone()));
    if fallback_live {
        replayer = replayer.with_fallback(Recorder::new(Arc::new(LiveTransport::default()), replay_dir).unwrap());
    }
    let replayer = Arc::new(replayer);
    set_default_transport(replayer.clone());

    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let tool = FFmpegTool::new(ffmpeg_dir.join("ffmpeg"), ffmpeg_dir.join("ffprobe"));
    let mut albums: Vec<DetectedAlbum> = LibraryScanner::new().scan(roots).albums;
    albums.sort_by_key(|a| a.url.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let mut out = vec![];
    for album in &albums {
        let name = album.url.file_name().unwrap().to_string_lossy().into_owned();
        eprintln!("identify {name}");
        let mut entry = json!({ "folder": name });
        let toc =
            if album.discs.len() == 1 { drtagger_core::disc::album_toc(album, &tool).ok().flatten() } else { None };
        let mut ctdb = vec![];
        if let Some(toc) = &toc {
            entry["toc"] = json!({
                "discID": toc.musicbrainz_disc_id(),
                "ctdb": toc.ctdb_toc_string(),
                "freedb": toc.freedb_disc_id(),
            });
            match CUEToolsDBClient::new(UA).lookup(&toc.ctdb_toc_string()) {
                Ok(r) => {
                    ctdb = r.metadata.clone();
                    entry["ctdb"] = serde_json::to_value(&r).unwrap();
                }
                Err(e) => entry["ctdbError"] = e.to_string().into(),
            }
        }
        let mut config = IdentifierConfig::new(UA);
        config.acoust_id_key = env("ACOUSTID_KEY");
        config.discogs_token = env("DISCOGS_TOKEN");
        config.scan_artwork = false;
        let result = Identifier::new(config, tool.clone()).identify(album, toc.as_ref(), &ctdb, &[], &|_| {});
        entry["result"] = result_json(&result);
        entry["rawFingerprints"] = raw_fingerprints(&tool, &result).into();

        if let Some(best) = result.best() {
            let ctx = TagContext {
                discogs: result.discogs_candidates.first().cloned(),
                musicbrainz_disc_id: toc.as_ref().map(|t| t.musicbrainz_disc_id()),
                freedb_disc_id: toc.as_ref().map(|t| t.freedb_disc_id()),
                acoust_ids: result.fingerprints.as_ref().map(|f| f.acoust_ids.clone()).unwrap_or_default(),
                ..Default::default()
            };
            let media = MatchScorer::relevant_media(&best.candidate, result.signals.local_format());
            // By disc position, as the tagger maps the files.
            let mut seen: BTreeMap<i64, usize> = BTreeMap::new();
            entry["tags"] = result
                .signals
                .tracks
                .iter()
                .map(|t| {
                    let n = seen.entry(t.disc_number).or_insert(0);
                    let track = *n;
                    *n += 1;
                    PicardMapper::track_tags_by_disc(&best.candidate, &media, t.disc_number as usize, track, &ctx)
                        .map(|tags| json!(tags.pairs().into_iter().map(|(k, v)| [k, v]).collect::<Vec<_>>()))
                        .unwrap_or(Value::Null)
                })
                .collect();
            let fetcher = CoverArtFetcher::new(UA, env("FANART_KEY").as_deref());
            let options = fetcher.options(&best.candidate, result.discogs_candidates.first(), Some(album), &|_| {});
            entry["artwork"] = options
                .iter()
                .map(|o| {
                    let url = match &o.location {
                        ArtworkLocation::Remote(u) => u.clone(),
                        ArtworkLocation::Local(p) => p.file_name().unwrap().to_string_lossy().into_owned(),
                    };
                    json!({ "source": o.source.raw(), "url": url, "label": o.label })
                })
                .collect();
        }
        out.push(entry);
    }
    let sims = similarities.lock().unwrap().clone();
    json!({
        "albums": out,
        "replay": { "misses": replayer.misses(), "acoustidSimilarities": sims },
    })
}
