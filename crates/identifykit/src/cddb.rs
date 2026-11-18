//! Answers CDDB-over-HTTP requests (the freedb/gnudb protocol) so a ripper
//! such as XLD or whipper can use drtagger as its "FreeDB" server. Only the
//! commands rippers send are implemented: `cddb query`, `cddb read`,
//! `cddb lscat`, `hello`, `proto`, `motd` and `stat`. Releases come from
//! MusicBrainz's fuzzy TOC lookup, ranked by MatchScorer.

use crate::scoring::{CandidateOrigin, MatchScorer};
use crate::signals::{AlbumSignals, LocalTrack};
use librarykit::DiscTOC;
use providerkit::{Candidate, CandidateMedium, MusicBrainzClient};
use std::collections::HashMap;
use std::sync::Mutex;
use unicode_segmentation::UnicodeSegmentation;

pub type Lookup = Box<dyn Fn(&DiscTOC) -> Vec<Candidate> + Send + Sync>;

/// CDDB tells matches apart by (category, discid): every candidate of a
/// query gets its own category.
pub const CATEGORIES: [&str; 10] =
    ["rock", "misc", "jazz", "classical", "blues", "folk", "country", "newage", "reggae", "soundtrack"];

struct Entry {
    category: &'static str,
    candidate: Candidate,
    medium: CandidateMedium,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Vec<Entry>>,
    tocs: HashMap<String, (Vec<i64>, i64)>,
}

pub struct CDDBResponder {
    lookup: Lookup,
    state: Mutex<State>,
    pub max_matches: usize,
}

impl CDDBResponder {
    pub fn new(lookup: Lookup) -> CDDBResponder {
        CDDBResponder { lookup, state: Mutex::new(State::default()), max_matches: 6 }
    }

    /// Live lookup: MusicBrainz fuzzy TOC search (CDDB sends the disc length
    /// in whole seconds, so the exact disc ID cannot be computed).
    pub fn live(user_agent: &str) -> CDDBResponder {
        let mb = MusicBrainzClient::new(user_agent);
        CDDBResponder::new(Box::new(move |toc| {
            mb.lookup_disc_id("-", Some(&toc.musicbrainz_toc_string())).unwrap_or_default()
        }))
    }

    /// `query` is the URL query string ("cmd=cddb+query+…&hello=…&proto=6").
    pub fn respond(&self, query: &str) -> String {
        let fields = Self::parse_form(query);
        let Some(cmd) = fields.get("cmd") else { return "500 Command syntax error.\n".into() };
        let words: Vec<&str> = cmd.split(' ').filter(|w| !w.is_empty()).collect();
        let Some(first) = words.first().map(|w| w.to_lowercase()) else {
            return "500 Command syntax error.\n".into();
        };
        match first.as_str() {
            "cddb" => {
                if words.len() < 2 {
                    return "500 Command syntax error.\n".into();
                }
                match words[1].to_lowercase().as_str() {
                    "query" => self.handle_query(&words[2..]),
                    "read" => self.handle_read(&words[2..]),
                    "lscat" => format!(
                        "210 OK, category list follows (until terminating `.')\n{}\n.\n",
                        CATEGORIES.join("\n")
                    ),
                    _ => "500 Unrecognized command.\n".into(),
                }
            }
            "proto" => "201 OK, protocol version now: 6\n".into(),
            "hello" => "200 Hello and welcome.\n".into(),
            "motd" => "210 Last modified: 01/01/2026 00:00:00 MOTD follows (until terminating `.')\ndrtagger CDDB service.\n.\n"
                .into(),
            "stat" => "210 OK, status information follows (until terminating `.')\nCurrent proto: 6\nMax proto: 6\n.\n".into(),
            _ => "500 Unrecognized command.\n".into(),
        }
    }

    fn swift_int(s: &str) -> Option<i64> {
        let d = s.strip_prefix(['+', '-']).unwrap_or(s);
        if d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()) { None } else { s.parse().ok() }
    }

    /// cddb query <discid> <ntrks> <off1> … <offN> <nsecs>
    fn handle_query(&self, args: &[&str]) -> String {
        let syntax = || "500 Command syntax error.\n".to_string();
        if args.len() < 3 {
            return syntax();
        }
        let Some(n) = Self::swift_int(args[1]).filter(|n| *n > 0).map(|n| n as usize) else { return syntax() };
        if args.len() != n + 3 {
            return syntax();
        }
        let Some(seconds) = Self::swift_int(args[n + 2]) else { return syntax() };
        let discid = args[0].to_lowercase();
        let offsets: Vec<i64> = args[2..2 + n].iter().filter_map(|a| Self::swift_int(a)).collect();
        if offsets.len() != n {
            return syntax();
        }
        self.state.lock().unwrap().tocs.insert(discid.clone(), (offsets.clone(), seconds));
        let toc =
            DiscTOC { first_track: 1, last_track: n as i64, track_offsets: offsets.clone(), lead_out: seconds * 75 };

        // Local durations from the TOC, for the scorer.
        let mut ends: Vec<i64> = offsets.iter().skip(1).copied().collect();
        ends.push(seconds * 75);
        let signals = AlbumSignals {
            tracks: offsets
                .iter()
                .enumerate()
                .map(|(i, o)| LocalTrack::new(i, 1, Some((ends[i] - o) as f64 / 75.0)))
                .collect(),
            sample_rate: Some(44100),
            bits_per_sample: Some(16),
            ..Default::default()
        };
        let durations: Vec<f64> = signals.tracks.iter().filter_map(|t| t.duration_seconds).collect();

        let mut ranked: Vec<(f64, Candidate, CandidateMedium)> = vec![];
        for c in (self.lookup)(&toc) {
            let Some(medium) = Self::best_medium(&c, &durations) else { continue };
            let mut single = Candidate::new(c.source, &c.provider_id, &c.title, &c.artist);
            single.release_group_id = c.release_group_id.clone();
            single.year = c.year.clone();
            single.country = c.country.clone();
            single.label = c.label.clone();
            single.catalog_number = c.catalog_number.clone();
            single.media_format = medium.format.clone();
            single.track_count = Some(medium.tracks.len() as i64);
            single.disambiguation = c.disambiguation.clone();
            single.tracks = medium.tracks.clone();
            single.released_date = c.released_date.clone();
            single.barcode = c.barcode.clone();
            single.media = vec![medium.clone()];
            single.status = c.status.clone();
            single.primary_type = c.primary_type.clone();
            let score = MatchScorer::score(&single, &signals, &[CandidateOrigin::TocMatch], None, 0).score;
            ranked.push((score, c, medium));
        }
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let list: Vec<Entry> = ranked
            .into_iter()
            .take(self.max_matches.min(CATEGORIES.len()))
            .enumerate()
            .map(|(i, (_, candidate, medium))| Entry { category: CATEGORIES[i], candidate, medium })
            .collect();
        let answer = if list.is_empty() {
            "202 No match found.\n".to_string()
        } else if list.len() == 1 {
            format!("200 {} {discid} {}\n", list[0].category, Self::dtitle(&list[0]))
        } else {
            // The list is shown to the user to pick from, so it says which
            // pressing each one is.
            let mut out = "211 Found inexact matches, list follows (until terminating `.')\n".to_string();
            for e in &list {
                out += &format!("{} {discid} {}{}\n", e.category, Self::dtitle(e), Self::edition(e));
            }
            out + ".\n"
        };
        self.state.lock().unwrap().entries.insert(discid, list);
        answer
    }

    /// cddb read <category> <discid>
    fn handle_read(&self, args: &[&str]) -> String {
        if args.len() < 2 {
            return "500 Command syntax error.\n".into();
        }
        let category = args[0].to_lowercase();
        let discid = args[1].to_lowercase();
        let state = self.state.lock().unwrap();
        let Some(entry) = state.entries.get(&discid).and_then(|l| l.iter().find(|e| e.category == category)) else {
            return format!("401 {category} {discid} No such CD entry in database.\n");
        };
        let c = &entry.candidate;
        let m = &entry.medium;
        let mut out = format!("210 {category} {discid} CD database entry follows (until terminating `.')\n");
        out += "# xmcd\n#\n# Track frame offsets:\n";
        if let Some((offsets, seconds)) = state.tocs.get(&discid) {
            for o in offsets {
                out += &format!("#\t{o}\n");
            }
            out += &format!("#\n# Disc length: {seconds} seconds\n");
        }
        out += "#\n# Revision: 0\n# Submitted via: drtagger\n#\n";
        out += &format!("DISCID={discid}\n");
        out += &Self::field("DTITLE", &Self::dtitle(entry));
        out += &Self::field("DYEAR", c.year.as_deref().unwrap_or(""));
        out += &Self::field("DGENRE", c.genres.first().map_or("", |g| g.as_str()));
        for (i, t) in m.tracks.iter().enumerate() {
            let title = match &t.artist {
                Some(a) => format!("{a} / {}", t.title),
                None => t.title.clone(),
            };
            out += &Self::field(&format!("TTITLE{i}"), &title);
        }
        let mut extd: Vec<String> = vec![];
        extd.extend(c.label.clone());
        extd.extend(c.catalog_number.clone());
        extd.extend(c.country.clone());
        extd.extend(c.barcode.as_ref().map(|b| format!("Barcode {b}")));
        extd.extend(m.format.clone());
        if c.source == providerkit::Source::Musicbrainz {
            extd.push(format!("MusicBrainz release {}", c.provider_id));
        }
        out += &Self::field("EXTD", &extd.join(" · "));
        for i in 0..m.tracks.len() {
            out += &format!("EXTT{i}=\n");
        }
        out + "PLAYORDER=\n.\n"
    }

    /// The medium of a (possibly multi-disc or hybrid) release that matches
    /// the queried disc: same track count, durations closest.
    pub fn best_medium(c: &Candidate, durations: &[f64]) -> Option<CandidateMedium> {
        let media =
            if c.media.is_empty() { vec![CandidateMedium::with_tracks(1, c.tracks.clone())] } else { c.media.clone() };
        let error = |m: &CandidateMedium| -> f64 {
            m.tracks.iter().zip(durations).map(|(t, d)| (t.duration_ms.unwrap_or(0) as f64 / 1000.0 - d).abs()).sum()
        };
        let mut best: Option<CandidateMedium> = None;
        for m in media.into_iter().filter(|m| m.tracks.len() == durations.len()) {
            if best.as_ref().is_none_or(|b| error(&m) < error(b)) {
                best = Some(m);
            }
        }
        best
    }

    fn edition(e: &Entry) -> String {
        let c = &e.candidate;
        let parts: Vec<String> = [c.year.clone(), c.country.clone(), e.medium.format.clone(), c.catalog_number.clone()]
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty())
            .collect();
        if parts.is_empty() { String::new() } else { format!(" [{}]", parts.join(" · ")) }
    }

    fn dtitle(e: &Entry) -> String {
        format!("{} / {}", e.candidate.artist, e.candidate.title).replace('\n', " ")
    }

    /// xmcd lines are limited to 256 bytes; longer values repeat the key.
    pub fn field(key: &str, value: &str) -> String {
        let clean = value.replace('\n', " ");
        let mut out = String::new();
        let mut chunk = String::new();
        for ch in clean.graphemes(true) {
            if key.len() + 1 + chunk.len() + ch.len() > 250 {
                out += &format!("{key}={chunk}\n");
                chunk.clear();
            }
            chunk.push_str(ch);
        }
        out + &format!("{key}={chunk}\n")
    }

    /// application/x-www-form-urlencoded: "+" is a space, then %-decoding.
    pub fn parse_form(query: &str) -> HashMap<String, String> {
        let mut fields = HashMap::new();
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let parts: Vec<&str> = pair.splitn(2, '=').filter(|p| !p.is_empty()).collect();
            let Some(key) = parts.first() else { continue };
            let raw = parts.get(1).copied().unwrap_or("");
            let spaced = raw.replace('+', " ");
            fields.insert(key.to_lowercase(), percent_decode(&spaced).unwrap_or_else(|| raw.to_string()));
        }
        fields
    }
}

/// `removingPercentEncoding`: None on a bad escape or invalid UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use providerkit::{CandidateTrack, Source};

    fn candidate(id: &str, title: &str, durations: &[i64], year: &str) -> Candidate {
        let tracks: Vec<CandidateTrack> = durations
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let mut t = CandidateTrack::new(i as i64 + 1, &format!("Track {} 曲", i + 1));
                t.duration_ms = Some(d * 1000);
                t
            })
            .collect();
        let mut c = Candidate::new(Source::Musicbrainz, id, title, "坂本龍一");
        c.year = Some(year.into());
        c.country = Some("JP".into());
        c.label = Some("Midi".into());
        c.catalog_number = Some("MDCL-1234".into());
        c.media_format = Some("CD".into());
        c.track_count = Some(tracks.len() as i64);
        c.barcode = Some("4988000000000".into());
        c.media = vec![CandidateMedium::new(1, Some("CD".into()), None, tracks.clone(), vec![], None)];
        c.tracks = tracks;
        c
    }

    #[test]
    fn query_lists_ranked_matches_and_read_returns_xmcd() {
        // 200, 180 and 240 s: offsets 150, 15150, 28650; length 622 s.
        let good = candidate("good", "音楽図鑑", &[200, 180, 240], "1984");
        let poor = candidate("poor", "音楽図鑑 (remaster)", &[205, 170, 250], "2015");
        let other = candidate("other", "Other", &[100, 100], "2000");
        let responder = CDDBResponder::new(Box::new(move |toc| {
            assert_eq!(toc.track_offsets, [150, 15150, 28650]);
            assert_eq!(toc.lead_out, 622 * 75);
            vec![poor.clone(), other.clone(), good.clone()]
        }));
        let q = responder.respond("cmd=cddb+query+2a024903+3+150+15150+28650+622&hello=user+host+XLD+20250302&proto=6");
        let lines: Vec<&str> = q.lines().collect();
        assert!(lines[0].starts_with("211 "), "{q}");
        assert_eq!(lines[1], "rock 2a024903 坂本龍一 / 音楽図鑑 [1984 · JP · CD · MDCL-1234]");
        assert!(lines[2].starts_with("misc 2a024903 ") && *lines.last().unwrap() == ".");
        assert!(!q.contains("Other"));

        let r = responder.respond("cmd=cddb+read+rock+2a024903&hello=user+host+XLD+20250302&proto=6");
        assert!(r.starts_with("210 rock 2a024903 "));
        assert!(
            r.contains("DTITLE=坂本龍一 / 音楽図鑑\n")
                && r.contains("DYEAR=1984\n")
                && r.contains("TTITLE2=Track 3 曲\n")
        );
        assert!(r.contains("#\t15150\n") && r.contains("# Disc length: 622 seconds"));
        assert!(r.contains("MDCL-1234") && r.contains("MusicBrainz release good"));
        assert!(r.ends_with("PLAYORDER=\n.\n"));

        assert!(responder.respond("cmd=cddb+read+jazz+2a024903").starts_with("401 "));
        assert!(responder.respond("cmd=proto+6").starts_with("201 "));
        let none = CDDBResponder::new(Box::new(|_| vec![]));
        assert!(none.respond("cmd=cddb+query+00000001+1+150+60").starts_with("202 "));
        assert!(none.respond("cmd=cddb+query+bad").starts_with("500 "));
    }

    #[test]
    fn single_match_answers_200_and_long_fields_split() {
        let one = candidate("one", "A", &[200, 180, 240], "1984");
        let responder = CDDBResponder::new(Box::new(move |_| vec![one.clone()]));
        assert_eq!(
            responder.respond("cmd=cddb+query+2a024903+3+150+15150+28650+622"),
            "200 rock 2a024903 坂本龍一 / A\n"
        );
        let long = CDDBResponder::field("TTITLE0", &"あ".repeat(120));
        let lines: Vec<&str> = long.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.len() <= 256));
    }
}
