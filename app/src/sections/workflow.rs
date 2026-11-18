//! Top of the inspector: the five steps of an album, what is happening
//! now, and one button for the next thing to do (the Mac app's
//! WorkflowView).

use super::common::{activity_line, button};
use crate::actions::spawn;
use crate::state::{Job, State, job};
use drtagger_core::i18n::tr;
use drtagger_core::store::AlbumRecord;
use drtagger_core::tagging::{self, chosen_candidate};
use gtk::prelude::*;
use librarykit::AlbumKind;

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Done,
    Current,
    Pending,
}

struct Facts {
    identified: bool,
    chosen: bool,
    confident: bool,
    files_needed: bool,
    has_plan: bool,
    already_tagged: bool,
    applied: bool,
    busy: bool,
    extract_first: bool,
    has_fingerprints: bool,
}

fn facts(state: &State, r: &AlbumRecord) -> Facts {
    let members = state.members(r);
    let identification = r.identification();
    let plan = state.plan_for(r);
    Facts {
        identified: identification.is_some(),
        chosen: chosen_candidate(r).is_some(),
        confident: identification.as_ref().is_some_and(|i| {
            i.is_confident() && r.selected_candidate_id.as_deref() == i.best().map(|b| b.id()).as_deref()
        }),
        files_needed: members.iter().any(tagging::needs_files),
        has_plan: plan.is_some(),
        already_tagged: plan.as_ref().is_some_and(|p| p.already_tagged),
        applied: r.tagged_at.is_some() && plan.is_none(),
        busy: members.iter().any(|m| state.is_busy_any(&m.path)) || state.is_busy_any(&state.leader_path(r)),
        extract_first: r.kind == AlbumKind::SacdIso,
        has_fingerprints: identification
            .as_ref()
            .and_then(|i| i.fingerprints.as_ref())
            .is_some_and(|f| f.fingerprinted_tracks > 0),
    }
}

fn files_label(kind: AlbumKind) -> String {
    match kind {
        AlbumKind::SacdIso => tr("Extract"),
        AlbumKind::CueImage => tr("Split"),
        _ => tr("Files"),
    }
}

fn steps(r: &AlbumRecord, f: &Facts) -> Vec<(String, Step)> {
    use Step::*;
    let pick = |done: bool, current: bool| {
        if done {
            Done
        } else if current {
            Current
        } else {
            Pending
        }
    };
    if f.extract_first {
        return vec![
            (files_label(r.kind), pick(!f.files_needed, true)),
            (tr("Identify"), pick(f.identified, !f.files_needed)),
            (tr("Release"), pick(f.chosen, f.identified)),
            (tr("Tags"), pick(f.has_plan || f.applied, f.chosen && !f.files_needed)),
            (tr("Apply"), pick(f.applied, f.has_plan)),
        ];
    }
    vec![
        (tr("Identify"), pick(f.identified, true)),
        (tr("Release"), pick(f.chosen, f.identified)),
        (files_label(r.kind), pick(!f.files_needed, f.chosen)),
        (tr("Tags"), pick(f.has_plan || f.applied, f.chosen && !f.files_needed)),
        (tr("Apply"), pick(f.applied, f.has_plan)),
    ]
}

fn step_bar(steps: &[(String, Step)]) -> gtk::Widget {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for (i, (name, step)) in steps.iter().enumerate() {
        if i > 0 {
            let line = gtk::Separator::new(gtk::Orientation::Horizontal);
            line.set_width_request(28);
            line.set_valign(gtk::Align::Center);
            bar.append(&line);
        }
        let item = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let (icon, class) = match step {
            Step::Done => ("object-select-symbolic", "success"),
            Step::Current => ("media-record-symbolic", "accent"),
            Step::Pending => ("radio-symbolic", "dim-label"),
        };
        let image = gtk::Image::from_icon_name(icon);
        image.add_css_class(class);
        item.append(&image);
        let label = gtk::Label::new(Some(name));
        match step {
            Step::Current => label.add_css_class("heading"),
            Step::Pending => label.add_css_class("dim-label"),
            Step::Done => {}
        }
        item.append(&label);
        bar.append(&item);
    }
    bar.upcast()
}

pub fn section(state: &State, r: &AlbumRecord) -> Option<gtk::Widget> {
    let f = facts(state, r);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    card.add_css_class("card");
    card.add_css_class("workflow-card");
    card.append(&step_bar(&steps(r, &f)));

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let status = status(r, &f);
    status.set_hexpand(true);
    row.append(&status);

    // Extracted after identifying: fingerprints can now help.
    if !f.busy
        && f.identified
        && !f.confident
        && !f.applied
        && r.kind == AlbumKind::SacdIso
        && !f.files_needed
        && !f.has_fingerprints
    {
        let b = button(&tr("Identify Again with Fingerprints"), false);
        b.set_tooltip_text(Some(&tr("The disc is extracted now, so acoustic fingerprints can confirm the edition")));
        let (state, r) = (state.clone(), r.clone());
        b.connect_clicked(move |_| {
            let (state, r) = (state.clone(), r.clone());
            spawn(async move { state.identify(&r).await });
        });
        row.append(&b);
    }
    if !f.busy && f.chosen && f.confident && !f.applied && !f.has_plan {
        let b = button(&tr("Process All"), false);
        b.set_tooltip_text(Some(&tr(
            "Extract or split, write tags, cover and ReplayGain, and file the album in the library",
        )));
        let (state, r) = (state.clone(), r.clone());
        b.connect_clicked(move |b| {
            let (state, r, b) = (state.clone(), r.clone(), b.clone().upcast::<gtk::Widget>());
            spawn(async move { process_all(&state, &r, &b).await });
        });
        row.append(&b);
    }
    if let Some(w) = primary(state, r, &f) {
        row.append(&w);
    }
    card.append(&row);
    // Identification, then each disc's split or extraction, then tagging.
    let leader = state.leader_path(r);
    let mut keys = vec![job(&leader, Job::Identify)];
    keys.extend(state.members(r).into_iter().map(|m| job(&m.path, Job::Disc)));
    keys.push(job(&leader, Job::Tags));
    card.append(&activity_line(state, keys, true));
    auto_preview(state, r, &f);
    Some(card.upcast())
}

fn status(r: &AlbumRecord, f: &Facts) -> gtk::Widget {
    let dim = |t: &str| {
        let l = gtk::Label::builder().label(t).xalign(0.0).wrap(true).build();
        l.add_css_class("dim-label");
        l.upcast::<gtk::Widget>()
    };
    if f.extract_first && f.files_needed && !f.chosen {
        return dim(&tr("Extract the disc first: its fingerprints identify the exact edition."));
    }
    if !f.identified {
        return dim(&tr("Find the exact release from tags, disc data, scans and fingerprints."));
    }
    let Some(c) = chosen_candidate(r) else {
        return super::common::status(
            "pan-down-symbolic",
            &tr("Choose a release in Identification below."),
            Some("warning"),
        );
    };
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = gtk::Label::builder()
        .label(format!("{} – {}", c.artist, c.title))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("heading");
    b.append(&title);
    let details: Vec<String> = [c.year.clone(), c.country.clone(), c.media_format.clone(), c.catalog_number.clone()]
        .into_iter()
        .flatten()
        .collect();
    let d =
        gtk::Label::builder().label(details.join(" · ")).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
    d.add_css_class("caption");
    d.add_css_class("dim-label");
    b.append(&d);
    if f.already_tagged {
        let l =
            super::common::status("object-select-symbolic", &tr("Already tagged with this release"), Some("success"));
        l.add_css_class("caption");
        b.append(&l);
    }
    b.upcast()
}

/// What a step button starts: the album and the button (a dialog parent).
type ButtonAction = fn(State, AlbumRecord, gtk::Widget) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;

fn primary(state: &State, r: &AlbumRecord, f: &Facts) -> Option<gtk::Widget> {
    let st = state.clone();
    let rec = r.clone();
    let on = move |b: &gtk::Button, action: ButtonAction| {
        let (state, r) = (st.clone(), rec.clone());
        b.connect_clicked(move |b| spawn(action(state.clone(), r.clone(), b.clone().upcast())));
    };
    if f.extract_first && f.files_needed && !(f.chosen && f.has_fingerprints) && !(f.chosen && f.confident) {
        let b = button(&tr("Extract & Identify"), true);
        b.set_sensitive(!f.busy);
        b.set_tooltip_text(Some(&tr(
            "Extract the DSF tracks, then identify the release with their acoustic fingerprints",
        )));
        on(&b, |s, r, w| Box::pin(async move { extract_and_identify(&s, &r, &w).await }));
        return Some(b.upcast());
    }
    if !f.identified {
        let identifying = state.is_busy(&job(&state.leader_path(r), Job::Identify));
        let b = button(&if identifying { tr("Identifying…") } else { tr("Identify") }, true);
        b.set_sensitive(!f.busy);
        on(&b, |s, r, _| Box::pin(async move { s.identify(&r).await }));
        return Some(b.upcast());
    }
    if !f.chosen {
        return None;
    }
    if f.files_needed {
        let label =
            if r.kind == AlbumKind::SacdIso { tr("Extract & Preview Tags") } else { tr("Split & Preview Tags") };
        let b = button(&label, true);
        b.set_sensitive(!f.busy);
        on(&b, |s, r, w| {
            Box::pin(async move {
                if s.produce_files(&r, &w).await
                    && let Some(r) = s.record(&r.path)
                {
                    s.build_plan(&r).await;
                }
            })
        });
        return Some(b.upcast());
    }
    if let Some(plan) = state.plan_for(r) {
        let b = button(&tr("Apply"), true);
        b.set_sensitive(!f.busy && plan.has_work());
        b.set_tooltip_text(Some(&tr(
            "Write the tags and the cover; originals are backed up and the audio is verified untouched",
        )));
        on(&b, |s, r, _| Box::pin(async move { s.apply(&r, None).await }));
        return Some(b.upcast());
    }
    if f.applied {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_valign(gtk::Align::Center);
        row.append(&super::common::status("object-select-symbolic", &tr("Done"), Some("success")));
        if let Some(first) = r.tag_reports().first().map(|t| t.path.clone()) {
            let b = button(&tr("Open Containing Folder"), false);
            let state = state.clone();
            b.connect_clicked(move |b| state.reveal(b.upcast_ref(), &first));
            row.append(&b);
        }
        return Some(row.upcast());
    }
    let b = button(&tr("Preview Tags"), true);
    b.set_sensitive(!f.busy);
    on(&b, |s, r, _| Box::pin(async move { s.build_plan(&r).await }));
    Some(b.upcast())
}

/// SACD: extract, identify with fingerprints, and preview the tags when
/// the match is sure.
async fn extract_and_identify(state: &State, r: &AlbumRecord, parent: &gtk::Widget) {
    if !state.produce_files(r, parent).await {
        return;
    }
    let Some(r) = state.record(&r.path) else { return };
    state.identify(&r).await;
    if let Some(r) = state.record(&r.path)
        && chosen_candidate(&r).is_some()
    {
        state.build_plan(&r).await;
    }
}

/// Confident match: files, tags, cover, ReplayGain and library layout in one go.
async fn process_all(state: &State, r: &AlbumRecord, parent: &gtk::Widget) {
    if !state.produce_files(r, parent).await {
        return;
    }
    let Some(r) = state.record(&r.path) else { return };
    state.build_plan(&r).await;
    if state.plan_for(&r).is_some_and(|p| p.has_work()) {
        state.apply(&r, None).await;
    }
}

/// Files just appeared or the release changed: show what will be written
/// without another click, once per change.
fn auto_preview(state: &State, r: &AlbumRecord, f: &Facts) {
    let signature = format!(
        "{:?}|{}|{}",
        r.selected_candidate_id,
        r.split_outcomes_json.as_ref().map_or(0, String::len),
        r.sacd_outcomes_json.as_ref().map_or(0, String::len)
    );
    if !state.changed(&r.path, signature) {
        return;
    }
    if r.batch_status.as_deref().is_some_and(|s| s != "done" && s != "failed") {
        return;
    }
    if !f.chosen || f.files_needed || f.has_plan || f.applied || f.busy {
        return;
    }
    {
        let (state, r) = (state.clone(), r.clone());
        spawn(async move { state.build_plan(&r).await });
    }
}
