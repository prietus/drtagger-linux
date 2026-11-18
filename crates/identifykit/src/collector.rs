//! Reads an album's signals off its folder name, CUE sheets, TOC, tags,
//! SACD text and artwork.

use crate::catalog::CatalogNumberParser;
use crate::signals::*;
use librarykit::DiscTOC;
use librarykit::FileRules;
use librarykit::{AlbumKind, DetectedAlbum};
use librarykit::{FolderNameParser, SourceHint};
use providerkit::CtdbMetadata;
use splitkit::FFmpegTool;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Reads barcodes and catalog numbers off an image (the vision phase).
pub trait ArtworkScanning: Send + Sync {
    fn scan(&self, image: &Path) -> Option<ArtworkScanResult>;
}

#[derive(Clone)]
pub struct CollectorOptions {
    pub scan_artwork: bool,
    pub max_artwork_files: usize,
    pub scanner: Option<Arc<dyn ArtworkScanning>>,
}

impl Default for CollectorOptions {
    fn default() -> Self {
        CollectorOptions { scan_artwork: true, max_artwork_files: 12, scanner: None }
    }
}

/// One album of a set: several albums that are one release.
#[derive(Clone)]
pub struct SetMember {
    pub album: DetectedAlbum,
    /// 1-based disc position in the release.
    pub position: i64,
    pub toc: Option<DiscTOC>,
    /// DSFs extracted from a SACD image, in track order.
    pub extracted_tracks: Vec<PathBuf>,
}

pub struct SignalCollector<'a> {
    pub tool: &'a FFmpegTool,
}

fn source_raw(s: SourceHint) -> &'static str {
    match s {
        SourceHint::Cd => "cd",
        SourceHint::Sacd => "sacd",
        SourceHint::Vinyl => "vinyl",
        SourceHint::Digital => "digital",
        SourceHint::Dvd => "dvd",
    }
}

fn file_name(p: &Path) -> Option<String> {
    p.file_name().map(|n| n.to_string_lossy().into_owned())
}

impl<'a> SignalCollector<'a> {
    pub fn new(tool: &'a FFmpegTool) -> SignalCollector<'a> {
        SignalCollector { tool }
    }

    pub fn collect(
        &self,
        album: &DetectedAlbum,
        toc: Option<&DiscTOC>,
        ctdb: &[CtdbMetadata],
        extracted_tracks: &[PathBuf],
        options: &CollectorOptions,
        log: &(dyn Fn(String) + Sync),
    ) -> AlbumSignals {
        let mut s = AlbumSignals {
            disc_count: if album.kind == AlbumKind::SacdIso { 1 } else { album.discs.len().max(1) as i64 },
            ..Default::default()
        };
        let folder = FolderNameParser::parse(&album.folder_name());
        s.artist_hint = folder.artist.clone();
        s.album_hint = folder.title.clone();
        s.folder_album_hint = folder.title.clone();
        s.year_hint = folder.year.clone();
        s.edition_hint = folder.edition.clone();
        s.edition_year_hint = folder.edition_year.clone();
        s.country_hint = folder.country.clone();
        s.source_hint = folder.source;
        // Codes people write in the folder name: "(Fantasy FCD 8387-2,
        // Germany)", "[SICP-1700]", "[0602547288042]".
        let barcode_re = regex::Regex::new(r"\b(\d{12,14})\b").unwrap();
        for note in &folder.notes {
            for cat in CatalogNumberParser::extract(std::slice::from_ref(note)) {
                s.catalog_numbers.push(SignalValue::with_detail(
                    cat.formatted(),
                    SignalOrigin::FolderName,
                    Some(note.clone()),
                ));
            }
            for m in barcode_re.captures_iter(note) {
                s.barcodes.push(SignalValue::with_detail(&m[1], SignalOrigin::FolderName, Some(note.clone())));
            }
        }
        if !folder.notes.is_empty() || folder.edition.is_some() || folder.source.is_some() || folder.country.is_some() {
            let mut parts = vec![];
            if let Some(e) = &folder.edition {
                parts.push(format!("edition {e}"));
            }
            if let Some(c) = &folder.country {
                parts.push(format!("country {c}"));
            }
            if let Some(src) = folder.source {
                parts.push(format!("source {}", source_raw(src)));
            }
            if !folder.notes.is_empty() {
                parts.push(format!("notes {}", folder.notes.join(" | ")));
            }
            log(format!("Folder name: {}.", parts.join(", ")));
        }

        // CUE texts and hints.
        let first_disc_number = album.discs.first().map(|d| d.number);
        for disc in &album.discs {
            let Some(cue) = &disc.cue else { continue };
            if let Some(b) = cue.barcode_hint() {
                s.barcodes.push(SignalValue::with_detail(
                    b,
                    SignalOrigin::Cue,
                    disc.cue_url.as_deref().and_then(file_name),
                ));
            }
            for c in cue.catalog_number_hints() {
                s.catalog_numbers.push(SignalValue::new(c, SignalOrigin::Cue));
            }
            if Some(disc.number) == first_disc_number {
                if let Some(p) = cue.performer.as_ref().filter(|p| !p.is_empty()) {
                    s.artist_hint = Some(p.clone());
                }
                if let Some(t) = cue.title.as_ref().filter(|t| !t.is_empty()) {
                    s.album_hint = Some(t.clone());
                }
                if let Some(d) = cue.date().filter(|d| d.chars().count() >= 4) {
                    s.year_hint = Some(providerkit::prefix(d, 4));
                }
            }
        }

        // Disc positions: a CD1/CD2 folder covers 1…n; a lone disc that says
        // "disc 2 of 3" covers position 2 of a declared total of 3.
        if album.discs.len() > 1 {
            let mut p: Vec<i64> = album.discs.iter().map(|d| d.number as i64).collect();
            p.sort();
            s.present_discs = p;
        } else if let Some((number, Some(total))) = album.disc_position()
            && total > 1
        {
            s.present_discs = vec![number as i64];
            s.declared_disc_total = Some(total as i64);
            s.disc_count = total as i64;
        }

        if let Some(toc) = toc {
            s.disc_id = Some(toc.musicbrainz_disc_id());
            s.toc_string = Some(toc.musicbrainz_toc_string());
            if let Some(first) = s.present_discs.first().copied() {
                s.disc_ids.insert(first, toc.musicbrainz_disc_id());
                s.toc_strings.insert(first, toc.musicbrainz_toc_string());
            }
        }
        for m in ctdb.iter().filter(|m| m.source == "musicbrainz") {
            if let Some(id) = &m.id {
                s.mb_release_ids.push(SignalValue::with_detail(
                    id.clone(),
                    SignalOrigin::Ctdb,
                    Some(format!("{} – {}", m.artist, m.album)),
                ));
            }
        }

        s.tracks = self.local_tracks(album, toc, extracted_tracks);

        // Existing tags (first track file) and SACD texts.
        if album.kind == AlbumKind::SacdIso {
            s.is_dsd = true;
            s.is_sacd_image = true;
            if let Ok(disc) = sacdkit::read(&album.url) {
                if let Some(a) = disc.artist() {
                    s.artist_hint = Some(a);
                }
                if let Some(t) = disc.title() {
                    s.album_hint = Some(FileRules::stripping_disc_token(&t));
                }
                if let Some(y) = disc.year() {
                    s.year_hint = Some(y);
                }
                let cat = disc.info.disc_catalog_number.clone();
                if !cat.is_empty() {
                    s.catalog_numbers.push(SignalValue::new(cat.clone(), SignalOrigin::SacdText));
                }
                let box_cat = disc.info.album_catalog_number.clone();
                if !box_cat.is_empty() && box_cat != cat {
                    s.catalog_numbers.push(SignalValue::with_detail(
                        box_cat,
                        SignalOrigin::SacdText,
                        Some("album set".into()),
                    ));
                }
            }
        } else if let Some(first) = album.discs.first().and_then(|d| d.track_files.first().or(d.image_file.as_ref())) {
            s.is_dsd = first.format.is_dsd();
            if let Ok(info) = self.tool.probe(&first.url) {
                s.sample_rate = Some(info.sample_rate);
                s.bits_per_sample = Some(info.bits_per_sample);
            }
            if let Ok(tags) = self.tool.probe_tags(&first.url) {
                Self::read_tags(&mut s, &tags);
                s.existing_tags = tags;
            }
        }

        if options.scan_artwork {
            let files = Self::prioritised_artwork(&album.artwork_files, options.max_artwork_files);
            let results = match &options.scanner {
                Some(scanner) => Self::scan_all(scanner.as_ref(), &files),
                None => vec![],
            };
            for r in &results {
                let name = file_name(&r.url);
                for b in &r.barcodes {
                    s.barcodes.push(SignalValue::with_detail(b.clone(), SignalOrigin::Artwork, name.clone()));
                }
                for c in &r.catalog_numbers {
                    s.catalog_numbers.push(SignalValue::with_detail(c.clone(), SignalOrigin::Artwork, name.clone()));
                }
            }
            let hits = results.iter().filter(|r| r.has_hit()).count();
            log(format!("Artwork: scanned {} image(s), {hits} with a barcode or catalog number.", results.len()));
            s.artwork_scans = results;
        }
        s
    }

    fn read_tags(s: &mut AlbumSignals, tags: &std::collections::BTreeMap<String, String>) {
        if let Some(b) = tags.get("BARCODE").or(tags.get("UPC")).or(tags.get("EAN")) {
            let digits = providerkit::digits(b);
            if digits.chars().count() >= 8 {
                s.barcodes.push(SignalValue::new(digits, SignalOrigin::Tags));
            }
        }
        for key in ["CATALOGNUMBER", "CATALOG#", "CATNO", "LABELNO"] {
            if let Some(c) = tags.get(key).filter(|c| !c.is_empty()) {
                s.catalog_numbers.push(SignalValue::with_detail(c.clone(), SignalOrigin::Tags, Some(key.into())));
            }
        }
        if let Some(id) = tags.get("MUSICBRAINZ_ALBUMID").or(tags.get("MUSICBRAINZ ALBUM ID"))
            && id.chars().count() == 36
        {
            s.mb_release_ids.push(SignalValue::new(id.clone(), SignalOrigin::Tags));
        }
        if let Some(a) = tags.get("ALBUMARTIST").or(tags.get("ALBUM_ARTIST")).or(tags.get("ARTIST"))
            && !a.is_empty()
        {
            s.artist_hint = Some(a.clone());
        }
        if let Some(t) = tags.get("ALBUM").filter(|t| !t.is_empty()) {
            // "Meteora (20th Anniversary Edition)" → title + edition.
            let parsed = FolderNameParser::parse_album_title(t);
            s.album_hint = Some(parsed.title.clone().unwrap_or_else(|| t.clone()));
            if let Some(e) = parsed.edition {
                s.edition_hint = Some(e);
            }
            if let Some(y) = parsed.edition_year {
                s.edition_year_hint = Some(y);
            }
        }
        if let Some(d) = tags.get("DATE").or(tags.get("YEAR"))
            && d.chars().count() >= 4
        {
            s.year_hint = Some(providerkit::prefix(d, 4));
        }
        if let Some(y) = tags.get("ORIGINALDATE").or(tags.get("ORIGINALYEAR"))
            && y.chars().count() >= 4
            && s.year_hint.as_deref() != Some(providerkit::prefix(y, 4).as_str())
        {
            // Picard: DATE is the pressing, ORIGINALDATE the first release.
            s.edition_year_hint = s.year_hint.clone();
            s.year_hint = Some(providerkit::prefix(y, 4));
        }
        if let Some(c) = tags.get("RELEASECOUNTRY")
            && c.chars().count() == 2
        {
            s.country_hint = Some(c.to_uppercase());
        }
        if let Some(m) = tags.get("MEDIA").map(|m| m.to_lowercase()) {
            s.source_hint = if m.contains("sacd") {
                Some(SourceHint::Sacd)
            } else if m.contains("vinyl") {
                Some(SourceHint::Vinyl)
            } else if m.contains("digital") {
                Some(SourceHint::Digital)
            } else if m.contains("dvd") || m.contains("blu-ray") {
                Some(SourceHint::Dvd)
            } else if m.contains("cd") {
                Some(SourceHint::Cd)
            } else {
                s.source_hint
            };
        }
    }

    /// Three images at a time; results by file name.
    fn scan_all(scanner: &dyn ArtworkScanning, files: &[PathBuf]) -> Vec<ArtworkScanResult> {
        let next = Mutex::new(0usize);
        let results = Mutex::new(vec![]);
        std::thread::scope(|sc| {
            for _ in 0..3 {
                sc.spawn(|| {
                    loop {
                        let file = {
                            let mut n = next.lock().unwrap();
                            let Some(f) = files.get(*n) else { break };
                            *n += 1;
                            f
                        };
                        if let Some(r) = scanner.scan(file) {
                            results.lock().unwrap().push(r);
                        }
                    }
                });
            }
        });
        let mut results = results.into_inner().unwrap();
        results.sort_by_key(|r| file_name(&r.url));
        results
    }

    /// Several albums that are one release: every member's signals, tracks
    /// in disc order, one disc ID per position.
    pub fn collect_set(
        &self,
        members: &[SetMember],
        declared_total: Option<i64>,
        ctdb: &[CtdbMetadata],
        options: &CollectorOptions,
        log: &(dyn Fn(String) + Sync),
    ) -> AlbumSignals {
        let mut ordered: Vec<&SetMember> = members.iter().collect();
        ordered.sort_by_key(|m| m.position);
        let mut merged: Option<AlbumSignals> = None;
        let mut seen_barcodes = std::collections::HashSet::new();
        let mut seen_catalogs = std::collections::HashSet::new();
        let mut seen_mbids = std::collections::HashSet::new();
        for member in &ordered {
            let mut part =
                self.collect(&member.album, member.toc.as_ref(), ctdb, &member.extracted_tracks, options, log);
            for t in &mut part.tracks {
                t.disc_number = member.position;
            }
            match merged.as_mut() {
                Some(m) => {
                    let base = m.tracks.len();
                    for mut t in part.tracks {
                        t.index += base;
                        m.tracks.push(t);
                    }
                    for b in part.barcodes {
                        if seen_barcodes.insert(b.value.clone()) {
                            m.barcodes.push(b);
                        }
                    }
                    for c in part.catalog_numbers {
                        if seen_catalogs.insert(c.value.clone()) {
                            m.catalog_numbers.push(c);
                        }
                    }
                    for id in part.mb_release_ids {
                        if seen_mbids.insert(id.value.clone()) {
                            m.mb_release_ids.push(id);
                        }
                    }
                    m.artwork_scans.extend(part.artwork_scans);
                    if let Some(id) = part.disc_id {
                        m.disc_ids.insert(member.position, id);
                    }
                    if let Some(t) = part.toc_string {
                        m.toc_strings.insert(member.position, t);
                    }
                }
                None => {
                    seen_barcodes.extend(part.barcodes.iter().map(|b| b.value.clone()));
                    seen_catalogs.extend(part.catalog_numbers.iter().map(|c| c.value.clone()));
                    seen_mbids.extend(part.mb_release_ids.iter().map(|m| m.value.clone()));
                    part.disc_ids.clear();
                    part.toc_strings.clear();
                    if let Some(id) = &part.disc_id {
                        part.disc_ids.insert(member.position, id.clone());
                    }
                    if let Some(t) = &part.toc_string {
                        part.toc_strings.insert(member.position, t.clone());
                    }
                    merged = Some(part);
                }
            }
        }
        let Some(mut s) = merged else { return AlbumSignals::default() };
        s.present_discs = ordered.iter().map(|m| m.position).collect();
        s.declared_disc_total = declared_total.or_else(|| {
            ordered.iter().filter_map(|m| m.album.disc_position().and_then(|p| p.1)).max().map(|t| t as i64)
        });
        s.disc_count = s.declared_disc_total.unwrap_or(0).max(ordered.iter().map(|m| m.position).max().unwrap_or(1));
        // The box title beats a disc title ("Piano Sonatas" over "Sonatas 1–7").
        if let Some(first) = ordered.first().map(|m| &m.album) {
            if let Some(box_title) = first.sacd.as_ref().and_then(|x| x.album_title.clone()).filter(|t| !t.is_empty()) {
                s.album_hint = Some(FileRules::stripping_disc_token(&box_title));
                if let Some(artist) = first.sacd.as_ref().and_then(|x| x.album_artist.clone()).filter(|a| !a.is_empty())
                {
                    s.artist_hint = Some(artist);
                }
            } else {
                let parsed = FolderNameParser::parse(&FileRules::stripping_disc_token(&first.folder_name()));
                if let Some(t) = parsed.title
                    && (s.album_hint.is_none() || s.album_hint == FolderNameParser::parse(&first.folder_name()).title)
                {
                    s.album_hint = Some(t);
                }
            }
        }
        log(format!(
            "Release set: discs {} of {}, {} track(s).",
            s.present_discs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", "),
            s.disc_count,
            s.track_count()
        ));
        s
    }

    /// Back covers and OBI strips carry the codes; scan those first.
    pub fn prioritised_artwork(urls: &[PathBuf], limit: usize) -> Vec<PathBuf> {
        let rank = |u: &PathBuf| -> u8 {
            let n = file_name(u).unwrap_or_default().to_lowercase();
            let has = |s: &str| n.contains(s);
            if has("back") || has("rear") || has("trasera") || has("obi") {
                0
            } else if has("inlay") || has("tray") || has("booklet") {
                1
            } else if has("front") || has("cover") || has("folder") || has("frontal") {
                3
            } else {
                2
            }
        };
        let mut sorted = urls.to_vec();
        sorted.sort_by_key(rank);
        sorted.truncate(limit);
        sorted
    }

    fn local_tracks(&self, album: &DetectedAlbum, toc: Option<&DiscTOC>, extracted: &[PathBuf]) -> Vec<LocalTrack> {
        let mut tracks = vec![];
        if album.kind == AlbumKind::SacdIso {
            let Ok(disc) = sacdkit::read(&album.url) else { return vec![] };
            let Some(area) = disc.stereo_area() else { return vec![] };
            // Durations from the TOC; audio from the extracted DSFs (ffmpeg
            // cannot read a Scarletbook image).
            let files = if extracted.len() == area.tracks.len() { extracted } else { &[] };
            for (i, t) in area.tracks.iter().enumerate() {
                tracks.push(LocalTrack {
                    index: tracks.len(),
                    disc_number: 1,
                    title: t.title.clone(),
                    duration_seconds: Some(t.duration.total_seconds()),
                    url: Some(if files.is_empty() { album.url.clone() } else { files[i].clone() }),
                    image_start: None,
                });
            }
            return tracks;
        }
        for disc in &album.discs {
            if let (Some(image), Some(cue)) = (&disc.image_file, &disc.cue) {
                // Durations from the TOC when there is one, else from the CUE
                // and the image length.
                let audio = cue.audio_tracks();
                let starts: Vec<f64> = audio.iter().map(|t| t.start().map_or(0.0, |s| s.total_seconds())).collect();
                let mut ends: Vec<f64> = starts.iter().skip(1).copied().collect();
                if let Some(toc) = toc.filter(|t| t.track_count() == starts.len() && album.discs.len() == 1) {
                    ends.push((toc.lead_out - DiscTOC::LEAD_IN_FRAMES) as f64 / 75.0);
                } else if let Some(d) = self.tool.probe(&image.url).ok().and_then(|i| i.duration_seconds) {
                    ends.push(d);
                } else {
                    ends.push(starts.last().copied().unwrap_or(0.0));
                }
                for (i, t) in audio.iter().enumerate() {
                    let dur = (ends[i] - starts[i]).max(0.0);
                    tracks.push(LocalTrack {
                        index: tracks.len(),
                        disc_number: disc.number as i64,
                        title: t.title.clone(),
                        duration_seconds: if dur > 0.0 { Some(dur) } else { None },
                        url: Some(image.url.clone()),
                        image_start: Some(starts[i]),
                    });
                }
            } else {
                for (i, f) in disc.track_files.iter().enumerate() {
                    let info = self.tool.probe(&f.url).ok();
                    let cue_title = disc
                        .cue
                        .as_ref()
                        .and_then(|c| c.audio_tracks().into_iter().find(|t| t.number == i as i64 + 1).cloned())
                        .and_then(|t| t.title);
                    tracks.push(LocalTrack {
                        index: tracks.len(),
                        disc_number: disc.number as i64,
                        title: cue_title,
                        duration_seconds: info.and_then(|i| i.duration_seconds),
                        url: Some(f.url.clone()),
                        image_start: None,
                    });
                }
            }
        }
        tracks
    }
}
