//! Identification signals, the ranked candidates and the user's pick (the
//! Mac app's IdentificationView and CandidateRow).

use super::common::{activity_line, button, caption, status};
use crate::actions::spawn;
use crate::release_images;
use crate::state::{Job, State, job};
use crate::thumbnails;
use crate::ui;
use drtagger_core::i18n::{tr, trf};
use drtagger_core::store::AlbumRecord;
use gtk::prelude::*;
use identifykit::{FormatCompatibility, IdentificationResult, MatchConfidence, ScoredCandidate};
use librarykit::AlbumKind;
use std::rc::Rc;

const SHOWN: usize = 12;

pub fn section(state: &State, r: &AlbumRecord) -> Option<gtk::Widget> {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let result = r.identification();
    body.append(&header(state, r, result.as_ref()));
    let key = job(&state.leader_path(r), Job::Identify);
    body.append(&activity_line(state, vec![key.clone()], false));
    if let Some(e) = state.error(&key) {
        body.append(&status("dialog-warning-symbolic", &e, Some("error")));
    }
    if let Some(result) = &result {
        body.append(&signals(state, result));
        if result.candidates.is_empty() {
            body.append(&ui::dim(&tr(
                "No release matched. Add API keys in Preferences or check the folder's artwork and tags.",
            )));
        } else {
            body.append(&candidates(state, r, result));
        }
        body.append(&log(state, r, result));
    }
    Some(ui::group(&tr("Identification"), &body))
}

fn fold_formats(state: &State, r: &AlbumRecord) -> bool {
    // Fold away impossible media by default only when the source is
    // certain: a Scarletbook image can only come from a SACD.
    state.flag(&r.path, "plausible-only").unwrap_or(r.kind == AlbumKind::SacdIso)
}

fn header(state: &State, r: &AlbumRecord, result: Option<&IdentificationResult>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let verdict: gtk::Widget = match result {
        Some(res) => match res.best() {
            Some(best) => {
                let w = if res.is_confident() {
                    status("object-select-symbolic", &tr("Confident match"), Some("success"))
                } else if best.confidence >= MatchConfidence::Likely {
                    status("dialog-question-symbolic", &tr("Likely match, please review"), Some("warning"))
                } else {
                    status(
                        "dialog-question-symbolic",
                        &trf("{} candidates, none certain", &[&res.candidates.len()]),
                        Some("warning"),
                    )
                };
                let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                b.append(&w);
                let t = gtk::Label::new(Some(&format!("· {:.0} s", res.elapsed_seconds)));
                t.add_css_class("dim-label");
                b.append(&t);
                b.upcast()
            }
            None => status("window-close-symbolic", &tr("No candidates"), Some("dim-label")),
        },
        None => ui::dim(&tr("Not identified yet")).upcast(),
    };
    verdict.set_hexpand(true);
    row.append(&verdict);
    let settings = state.core_settings();
    if settings.keys.acoust_id.as_deref().is_none_or(|k| k.trim().is_empty())
        && result.is_none_or(|r| r.fingerprints.is_none())
    {
        row.append(&caption(&tr("No AcoustID key: fingerprints off")));
    }
    if let Some(res) = result
        && res.candidates.iter().any(|c| c.format_compatibility == Some(FormatCompatibility::Unlikely))
    {
        let check = gtk::CheckButton::with_label(&tr("Plausible formats only"));
        check.set_active(fold_formats(state, r));
        check.set_tooltip_text(Some(&tr("Fold away releases whose medium cannot be the source of these files")));
        let (state, path) = (state.clone(), r.path.clone());
        check.connect_toggled(move |c| {
            state.set_flag(&path, "plausible-only", c.is_active());
            state.poke(&path);
        });
        row.append(&check);
    }
    let members = state.members(r);
    if members.len() > 1 {
        row.append(&caption(&trf("Release set · {} discs", &[&members.len()])));
    }
    if let Some(res) = result
        && res.candidates.len() > 1
    {
        let b = button(&tr("Compare Covers"), false);
        b.set_tooltip_text(Some(&tr("See the candidates' covers and obis side by side and pick yours")));
        let (state, r, list) = (state.clone(), r.clone(), res.candidates.iter().take(12).cloned().collect::<Vec<_>>());
        b.connect_clicked(move |b| {
            let (st, rec) = (state.clone(), r.clone());
            release_images::compare(
                b.upcast_ref(),
                &state,
                list.clone(),
                r.selected_candidate_id.clone(),
                Rc::new(move |s: &ScoredCandidate| st.choose_candidate(&rec, Some(s))),
            );
        });
        row.append(&b);
    }
    // The first identification starts from the step bar above.
    if result.is_some() {
        let b = button(&tr("Identify Again"), false);
        b.set_sensitive(!state.is_busy(&job(&state.leader_path(r), Job::Identify)));
        let (state, r) = (state.clone(), r.clone());
        b.connect_clicked(move |_| {
            let (state, r) = (state.clone(), r.clone());
            spawn(async move { state.identify(&r).await });
        });
        row.append(&b);
    }
    row.upcast()
}

fn signals(state: &State, result: &IdentificationResult) -> gtk::Widget {
    let s = &result.signals;
    let mut parts: Vec<String> = vec![];
    let barcodes = s.unique_barcodes();
    if !barcodes.is_empty() {
        parts.push(format!("{} {}", tr("barcode"), barcodes.join(", ")));
    }
    let catalogs = s.unique_catalog_numbers();
    if !catalogs.is_empty() {
        parts.push(format!("{} {}", tr("catalog"), catalogs.iter().take(3).cloned().collect::<Vec<_>>().join(", ")));
    }
    if s.disc_id.is_some() {
        parts.push(tr("disc ID"));
    }
    let ids = s.unique_release_ids();
    if !ids.is_empty() {
        parts.push(format!("{} MBID", ids.len()));
    }
    if let Some(fp) = &result.fingerprints
        && fp.fingerprinted_tracks > 0
    {
        parts.push(format!("{} {}/{}", tr("fingerprints"), fp.tracks_with_hits, fp.fingerprinted_tracks));
    }
    if let Some(e) = &s.edition_hint {
        parts.push(format!("{} {e}", tr("edition")));
    }
    if let Some(c) = &s.country_hint {
        parts.push(format!("{} {c}", tr("country")));
    }
    if let Some(src) = &s.source_hint {
        let raw = serde_json::to_value(src).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        parts.push(format!("{} {raw}", tr("source")));
    }
    if !s.artwork_scans.is_empty() {
        parts.push(trf("{} scans read", &[&s.artwork_scans.len()]));
    }
    let mut providers = vec!["MusicBrainz".to_string()];
    providers.push(if result.fingerprints.is_some() { "AcoustID".into() } else { tr("AcoustID off") });
    if state.core_settings().discogs_token().is_some() {
        providers.push(if result.discogs_candidates.is_empty() {
            tr("Discogs (no hits)")
        } else {
            format!("Discogs {}", result.discogs_candidates.len())
        });
    } else {
        providers.push(tr("Discogs off"));
    }
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    b.append(&caption(&if parts.is_empty() {
        tr("No identifying signals found")
    } else {
        format!("{} {}", tr("Signals:"), parts.join(" · "))
    }));
    b.append(&caption(&format!("{} {}", tr("Providers:"), providers.join(" · "))));
    // Japanese reissues and SACD editions are often on Discogs only.
    if state.core_settings().discogs_token().is_none() && !result.is_confident() {
        let w = status(
            "dialog-warning-symbolic",
            &tr(
                "Discogs is not set up: many editions (Japanese reissues, SACDs) are only there. Add a token in Settings → Providers.",
            ),
            Some("warning"),
        );
        w.set_margin_top(2);
        b.append(&w);
    }
    b.upcast()
}

fn candidates(state: &State, r: &AlbumRecord, result: &IdentificationResult) -> gtk::Widget {
    let fold = fold_formats(state, r);
    let unlikely = |c: &&ScoredCandidate| c.format_compatibility == Some(FormatCompatibility::Unlikely);
    let folded: Vec<&ScoredCandidate> = if fold { result.candidates.iter().filter(unlikely).collect() } else { vec![] };
    let mut shown: Vec<&ScoredCandidate> = if fold {
        result.candidates.iter().filter(|c| !unlikely(c)).collect()
    } else {
        result.candidates.iter().collect()
    };
    // Never leave the list empty because of the filter.
    let filter_emptied = shown.is_empty() && !folded.is_empty();
    if filter_emptied {
        shown = result.candidates.iter().collect();
    }
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    if filter_emptied {
        b.append(&caption(&tr("No release in a matching format; showing every format.")));
    }
    for c in shown.iter().take(SHOWN) {
        b.append(&candidate_row(state, r, c));
    }
    if shown.len() > SHOWN {
        b.append(&caption(&trf("{} more candidates not shown", &[&(shown.len() - SHOWN)])));
    }
    if !filter_emptied && !folded.is_empty() {
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 4);
        inner.set_margin_top(4);
        for c in folded.iter().take(SHOWN) {
            inner.append(&candidate_row(state, r, c));
        }
        let expander = gtk::Expander::builder()
            .label(trf("{} in other formats", &[&folded.len()]))
            .child(&inner)
            .expanded(state.flag(&r.path, "other-formats").unwrap_or(false))
            .build();
        let (st, path) = (state.clone(), r.path.clone());
        expander.connect_expanded_notify(move |e| st.set_flag(&path, "other-formats", e.is_expanded()));
        b.append(&expander);
    }
    b.upcast()
}

fn confidence_badge(c: MatchConfidence) -> gtk::Widget {
    let (text, class) = match c {
        MatchConfidence::Confident => (tr("confident"), "success"),
        MatchConfidence::Likely => (tr("likely"), "warning"),
        MatchConfidence::Possible => (tr("possible"), "dim-label"),
        MatchConfidence::Unlikely => (tr("unlikely"), "dim-label"),
    };
    let l = gtk::Label::new(Some(&text));
    l.add_css_class("caption");
    l.add_css_class("confidence-badge");
    l.add_css_class(class);
    l.set_valign(gtk::Align::Center);
    l.upcast()
}

fn candidate_details(s: &ScoredCandidate) -> String {
    let c = &s.candidate;
    let mut parts: Vec<String> = [&c.year, &c.country, &c.media_format, &c.label, &c.catalog_number, &c.barcode]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    if let Some(n) = c.track_count {
        parts.push(trf("{} tracks", &[&n]));
    }
    if let Some(d) = c.disambiguation.as_ref().filter(|d| !d.is_empty()) {
        parts.push(d.clone());
    }
    if s.format_compatibility == Some(FormatCompatibility::Unlikely) {
        parts.push(tr("format unlikely for these files"));
    }
    parts.join(" · ")
}

fn candidate_row(state: &State, r: &AlbumRecord, s: &ScoredCandidate) -> gtk::Widget {
    let c = &s.candidate;
    let selected = r.selected_candidate_id.as_deref() == Some(s.id().as_str());
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("candidate-row");
    if selected {
        row.add_css_class("selected-candidate");
    }
    let radio = gtk::Button::from_icon_name(if selected { "radio-checked-symbolic" } else { "radio-symbolic" });
    radio.add_css_class("flat");
    radio.add_css_class("circular");
    radio.set_valign(gtk::Align::Start);
    if selected {
        radio.add_css_class("accent");
    }
    radio.set_tooltip_text(Some(&if selected { tr("Clear the choice") } else { tr("Choose this release") }));
    {
        let (state, r, s) = (state.clone(), r.clone(), s.clone());
        radio.connect_clicked(move |_| state.choose_candidate(&r, if selected { None } else { Some(&s) }));
    }
    row.append(&radio);
    let photos = gtk::Button::builder().child(&thumbnails::remote(c.cover_art_url.as_deref(), 40)).build();
    photos.add_css_class("flat");
    photos.add_css_class("thumbnail-button");
    photos.set_valign(gtk::Align::Start);
    photos.set_tooltip_text(Some(&tr("See every photo of this release")));
    {
        let (state, r, s) = (state.clone(), r.clone(), s.clone());
        photos.connect_clicked(move |b| {
            let (st, rec, sc) = (state.clone(), r.clone(), s.clone());
            release_images::gallery(
                b.upcast_ref(),
                &state,
                &s,
                selected,
                Rc::new(move || st.choose_candidate(&rec, Some(&sc))),
            );
        });
    }
    row.append(&photos);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
    texts.set_hexpand(true);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let title = gtk::Label::builder()
        .label(format!("{} – {}", c.artist, c.title))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("heading");
    line.append(&title);
    line.append(&confidence_badge(s.confidence));
    let score = gtk::Label::new(Some(&format!("{:.0}", s.score)));
    score.add_css_class("caption");
    score.add_css_class("dim-label");
    score.add_css_class("numeric");
    line.append(&score);
    texts.append(&line);
    let d = gtk::Label::builder().label(candidate_details(s)).xalign(0.0).wrap(true).lines(2).build();
    d.add_css_class("caption");
    d.add_css_class("dim-label");
    texts.append(&d);
    let key = format!("expanded:{}", s.id());
    let expanded = state.flag(&r.path, &key).unwrap_or(false);
    if expanded {
        for reason in &s.reasons {
            texts.append(&caption(&format!("• {reason}")));
        }
        let id = gtk::Label::builder().label(format!("MBID {}", c.provider_id)).xalign(0.0).selectable(true).build();
        id.add_css_class("caption");
        id.add_css_class("monospace");
        id.add_css_class("dim-label");
        texts.append(&id);
    }
    row.append(&texts);
    let more = gtk::Button::from_icon_name(if expanded { "pan-up-symbolic" } else { "pan-down-symbolic" });
    more.add_css_class("flat");
    more.set_valign(gtk::Align::Start);
    {
        let (state, path) = (state.clone(), r.path.clone());
        more.connect_clicked(move |_| {
            state.set_flag(&path, &key, !expanded);
            state.poke(&path);
        });
    }
    row.append(&more);
    row.upcast()
}

fn log(state: &State, r: &AlbumRecord, result: &IdentificationResult) -> gtk::Widget {
    let lines = gtk::Box::new(gtk::Orientation::Vertical, 2);
    lines.set_margin_top(4);
    for l in &result.log {
        let label = gtk::Label::builder().label(l).xalign(0.0).wrap(true).selectable(true).build();
        label.add_css_class("caption");
        label.add_css_class("monospace");
        lines.append(&label);
    }
    let expander = gtk::Expander::builder()
        .label(trf("Log ({} lines)", &[&result.log.len()]))
        .child(&lines)
        .expanded(state.flag(&r.path, "log").unwrap_or(false))
        .build();
    let (st, path) = (state.clone(), r.path.clone());
    expander.connect_expanded_notify(move |e| st.set_flag(&path, "log", e.is_expanded()));
    expander.upcast()
}
