//! What Apply will write, field by field, with locks, the cover to embed,
//! and Restore for files already written (the Mac app's TagPreviewView).

use super::common::{activity_line, button, caption, status};
use crate::actions::spawn;
use crate::image_viewer::{self, Source as ImageSource};
use crate::state::{Job, State, job};
use crate::thumbnails;
use crate::ui;
use drtagger_core::i18n::{tr, trf};
use drtagger_core::store::AlbumRecord;
use drtagger_core::tagging::{TagPlan, chosen_candidate};
use gtk::glib;
use gtk::prelude::*;
use identifykit::ArtworkLocation;
use std::collections::BTreeSet;
use tagkit::{ChangeKind, TagChange};

pub fn section(state: &State, r: &AlbumRecord) -> Option<gtk::Widget> {
    let chosen_id = chosen_candidate(r).map(|c| c.id());
    // A plan built for another release is stale: hidden until rebuilt.
    let plan = state.plan_for(r).filter(|p| Some(&p.candidate_id) == chosen_id.as_ref());
    let key = job(&state.leader_path(r), Job::Tags);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.append(&header(state, r, plan.as_ref(), chosen_id.is_some()));
    body.append(&activity_line(state, vec![key.clone()], false));
    if let Some(e) = state.error(&key) {
        body.append(&status("dialog-warning-symbolic", &e, Some("error")));
    }
    match &plan {
        Some(plan) => {
            for w in &plan.warnings {
                let s = status("dialog-warning-symbolic", w, Some("warning"));
                s.add_css_class("caption");
                body.append(&s);
            }
            body.append(&cover_row(state, r, plan));
            if let Some(effects) = apply_effects(state) {
                body.append(&effects);
            }
            let locks = r.tag_locks();
            body.append(&album_fields(state, r, plan, &locks));
            body.append(&tracks(state, r, plan, &locks));
            if !plan.log.is_empty() {
                body.append(&log(state, r, &plan.log));
            }
        }
        None => {
            let reports = r.tag_reports();
            if !reports.is_empty() {
                let with_cover = reports.iter().filter(|t| t.picture_count > 0).count();
                let mut text = trf("{} files written", &[&reports.len()]);
                if with_cover > 0 {
                    text += &trf(", cover embedded in {}", &[&with_cover]);
                }
                text += &tr(". Use Refresh to preview again.");
                body.append(&caption(&text));
            }
        }
    }
    Some(ui::group(&tr("Tags"), &body))
}

fn header(state: &State, r: &AlbumRecord, plan: Option<&TagPlan>, has_candidate: bool) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let summary: gtk::Widget = if let Some(plan) = plan {
        if plan.has_work() {
            status(
                "document-edit-symbolic",
                &trf(
                    "{} of {} tracks change, {} fields",
                    &[&plan.changed_track_count(), &plan.tracks.len(), &plan.changed_field_count()],
                ),
                Some("warning"),
            )
        } else {
            status("object-select-symbolic", &tr("Tags already match the release"), Some("success"))
        }
    } else if let Some(at) = r.tagged_at {
        let when = glib::DateTime::from_unix_local(at / 1000)
            .ok()
            .and_then(|d| d.format("%x %H:%M").ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        status(
            "checkbox-checked-symbolic",
            &format!("{} · {}", trf("Tags written {}", &[&when]), trf("{} files", &[&r.tag_reports().len()])),
            Some("success"),
        )
    } else if !has_candidate {
        ui::dim(&tr("Choose a release above to preview its tags")).upcast()
    } else {
        ui::dim(&tr("Not previewed yet")).upcast()
    };
    summary.set_hexpand(true);
    row.append(&summary);
    let busy = state.is_busy(&job(&state.leader_path(r), Job::Tags));
    if state.members(r).iter().any(|m| !m.tag_backups().is_empty()) {
        let b = button(&tr("Restore Originals"), false);
        b.set_sensitive(!busy);
        b.set_tooltip_text(Some(&tr("Put back the metadata every file had before the first Apply")));
        let (state, r) = (state.clone(), r.clone());
        b.connect_clicked(move |_| {
            let (state, r) = (state.clone(), r.clone());
            spawn(async move { state.restore(&r).await });
        });
        row.append(&b);
    }
    let preview = button(&if plan.is_none() { tr("Preview Tags") } else { tr("Refresh") }, false);
    preview.set_sensitive(!busy && has_candidate);
    {
        let (state, r) = (state.clone(), r.clone());
        preview.connect_clicked(move |_| {
            let (state, r) = (state.clone(), r.clone());
            spawn(async move { state.build_plan(&r).await });
        });
    }
    row.append(&preview);
    if let Some(plan) = plan {
        let apply = button(&tr("Apply"), true);
        apply.set_sensitive(!busy && plan.has_work());
        apply.set_tooltip_text(Some(&tr(
            "Write the tags and the cover; originals are backed up and the audio is verified untouched",
        )));
        let (state, r) = (state.clone(), r.clone());
        apply.connect_clicked(move |_| {
            let (state, r) = (state.clone(), r.clone());
            spawn(async move { state.apply(&r, None).await });
        });
        row.append(&apply);
    }
    row.upcast()
}

/// What Apply does beyond the diff shown.
fn apply_effects(state: &State) -> Option<gtk::Widget> {
    let s = state.core_settings();
    let mut parts = vec![];
    if s.compute_replay_gain {
        parts.push(tr("loudness is measured and ReplayGain / R128 tags are written"));
    }
    if s.organize_after_apply
        && let Some(root) = &s.library_root
    {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        parts.push(trf("files move to {}/{}/{}", &[&name, &s.album_folder_template, &s.track_file_template]));
    }
    if parts.is_empty() {
        return None;
    }
    Some(caption(&format!("{} {}.", tr("On Apply,"), parts.join("; "))).upcast())
}

fn cover_row(state: &State, r: &AlbumRecord, plan: &TagPlan) -> gtk::Widget {
    let settings = state.core_settings();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    match &plan.cover {
        Some(cover) => {
            let picture = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Contain)
                .width_request(64)
                .height_request(64)
                .build();
            if let Some(image) = visionkit::imaging::load_data(&cover.data, 128) {
                picture.set_paintable(Some(&thumbnails::texture(&image)));
            }
            let b = gtk::Button::builder().child(&picture).tooltip_text(tr("Click to view larger")).build();
            b.add_css_class("flat");
            b.add_css_class("thumbnail-button");
            let (label, data, host) = (
                cover.option.label.clone(),
                cover.data.clone(),
                match &cover.option.location {
                    ArtworkLocation::Remote(u) => u.split('/').nth(2).map(str::to_string),
                    ArtworkLocation::Local(_) => None,
                },
            );
            b.connect_clicked(move |b| image_viewer::show(b, &label, host.as_deref(), ImageSource::Data(data.clone())));
            row.append(&b);
        }
        None => {
            let placeholder = gtk::Image::from_icon_name("image-x-generic-symbolic");
            placeholder.set_size_request(64, 64);
            placeholder.add_css_class("cover-placeholder");
            row.append(&placeholder);
        }
    }
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    texts.set_valign(gtk::Align::Center);
    if !settings.embed_front_cover {
        texts.append(&ui::dim(&tr("Cover embedding is off in Preferences")));
    } else if let Some(cover) = &plan.cover {
        texts.append(
            &gtk::Label::builder()
                .label(format!("{} · {}×{}", cover.option.label, cover.width, cover.height))
                .xalign(0.0)
                .build(),
        );
        let mut line = trf("Embedded at {} px max", &[&settings.embed_cover_max_pixels]);
        if settings.save_cover_file {
            line += &tr(", full size saved as cover file");
        }
        texts.append(&caption(&line));
    } else if plan.artwork_options.is_empty() {
        texts.append(&ui::dim(&tr("No artwork source found; existing pictures are kept")));
    } else {
        texts.append(&ui::dim(&tr("Existing pictures are kept")));
    }
    if !plan.artwork_options.is_empty() {
        let mut labels = vec![tr("Keep existing pictures")];
        labels.extend(plan.artwork_options.iter().map(|o| o.label.clone()));
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let dropdown = gtk::DropDown::from_strings(&refs);
        let selected = plan
            .cover_option_id
            .as_ref()
            .and_then(|id| plan.artwork_options.iter().position(|o| &o.id() == id))
            .map_or(0, |i| i + 1);
        dropdown.set_selected(selected as u32);
        dropdown.set_halign(gtk::Align::Start);
        let ids: Vec<Option<String>> =
            std::iter::once(None).chain(plan.artwork_options.iter().map(|o| Some(o.id()))).collect();
        let (state, r) = (state.clone(), r.clone());
        dropdown.connect_selected_notify(move |d| {
            let id = ids.get(d.selected() as usize).cloned().flatten();
            let (state, r) = (state.clone(), r.clone());
            spawn(async move { state.choose_cover(&r, id).await });
        });
        texts.append(&dropdown);
    }
    row.append(&texts);
    row.upcast()
}

fn change_row(state: &State, r: &AlbumRecord, change: &TagChange, locked: bool) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let lock = gtk::Button::from_icon_name(if locked { "changes-prevent-symbolic" } else { "changes-allow-symbolic" });
    lock.add_css_class("flat");
    lock.add_css_class(if locked { "accent" } else { "dim-label" });
    lock.set_valign(gtk::Align::Start);
    lock.set_tooltip_text(Some(&if locked {
        tr("Locked: the file keeps its value")
    } else {
        tr("Lock this field to keep the file's value")
    }));
    {
        let (state, r, name) = (state.clone(), r.clone(), change.name.clone());
        lock.connect_clicked(move |_| state.toggle_lock(&r, &name));
    }
    row.append(&lock);
    let name = gtk::Label::builder()
        .label(&change.name)
        .xalign(0.0)
        .width_chars(26)
        .max_width_chars(26)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .valign(gtk::Align::Start)
        .build();
    name.add_css_class("caption");
    name.add_css_class("monospace");
    row.append(&name);
    let values = gtk::Box::new(gtk::Orientation::Vertical, 1);
    values.set_hexpand(true);
    let value = |text: String, classes: &[&str]| {
        let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).selectable(true).build();
        l.add_css_class("caption");
        for c in classes {
            l.add_css_class(c);
        }
        l
    };
    match change.kind() {
        ChangeKind::Unchanged => values.append(&value(change.new.join("; "), &["dim-label"])),
        ChangeKind::Added => values.append(&value(change.new.join("; "), &["success"])),
        ChangeKind::Removed => values.append(&value(change.old.join("; "), &["error", "strikethrough"])),
        ChangeKind::Changed => {
            values.append(&value(change.old.join("; "), &["dim-label", "strikethrough"]));
            values.append(&value(change.new.join("; "), &["warning"]));
        }
    }
    row.append(&values);
    if locked && change.kind() != ChangeKind::Unchanged {
        row.set_opacity(0.6);
    }
    row.upcast()
}

fn album_fields(state: &State, r: &AlbumRecord, plan: &TagPlan, locks: &BTreeSet<String>) -> gtk::Widget {
    let show_unchanged = state.flag(&r.path, "show-unchanged").unwrap_or(false);
    let changes: Vec<TagChange> =
        plan.album_changes().into_iter().filter(|c| show_unchanged || c.kind() != ChangeKind::Unchanged).collect();
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let title = gtk::Label::builder().label(tr("Album fields")).xalign(0.0).hexpand(true).build();
    title.add_css_class("caption-heading");
    title.add_css_class("dim-label");
    head.append(&title);
    let toggle = gtk::CheckButton::with_label(&tr("Show unchanged"));
    toggle.set_active(show_unchanged);
    toggle.add_css_class("caption");
    {
        let (state, path) = (state.clone(), r.path.clone());
        toggle.connect_toggled(move |t| {
            state.set_flag(&path, "show-unchanged", t.is_active());
            state.poke(&path);
        });
    }
    head.append(&toggle);
    b.append(&head);
    if changes.is_empty() {
        b.append(&caption(&if show_unchanged { tr("No album-level fields") } else { tr("No album-level changes") }));
    }
    for c in &changes {
        b.append(&change_row(state, r, c, locks.contains(&c.name)));
    }
    b.upcast()
}

fn tracks(state: &State, r: &AlbumRecord, plan: &TagPlan, locks: &BTreeSet<String>) -> gtk::Widget {
    let show_unchanged = state.flag(&r.path, "show-unchanged").unwrap_or(false);
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
    inner.set_margin_top(4);
    for t in &plan.tracks {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let name = gtk::Label::builder()
            .label(t.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        name.add_css_class("caption-heading");
        head.append(&name);
        if !t.existing_pictures.is_empty() {
            head.append(&caption(&format!("· {}", trf("{} picture(s)", &[&t.existing_pictures.len()]))));
        }
        b.append(&head);
        let changes: Vec<TagChange> = plan
            .track_only_changes(t)
            .into_iter()
            .filter(|c| show_unchanged || c.kind() != ChangeKind::Unchanged)
            .collect();
        if changes.is_empty() {
            b.append(&caption(&tr("No track-specific changes")));
        }
        for c in &changes {
            b.append(&change_row(state, r, c, locks.contains(&c.name)));
        }
        inner.append(&b);
    }
    let label = gtk::Label::new(Some(&trf("Tracks ({})", &[&plan.tracks.len()])));
    label.add_css_class("caption-heading");
    label.add_css_class("dim-label");
    let expander =
        gtk::Expander::builder().child(&inner).expanded(state.flag(&r.path, "tag-tracks").unwrap_or(false)).build();
    expander.set_label_widget(Some(&label));
    let (st, path) = (state.clone(), r.path.clone());
    expander.connect_expanded_notify(move |e| st.set_flag(&path, "tag-tracks", e.is_expanded()));
    expander.upcast()
}

fn log(state: &State, r: &AlbumRecord, lines: &[String]) -> gtk::Widget {
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 2);
    inner.set_margin_top(4);
    for l in lines {
        let label = gtk::Label::builder().label(l).xalign(0.0).wrap(true).selectable(true).build();
        label.add_css_class("caption");
        label.add_css_class("monospace");
        inner.append(&label);
    }
    let expander = gtk::Expander::builder()
        .label(trf("Log ({} lines)", &[&lines.len()]))
        .child(&inner)
        .expanded(state.flag(&r.path, "tag-log").unwrap_or(false))
        .build();
    let (st, path) = (state.clone(), r.path.clone());
    expander.connect_expanded_notify(move |e| st.set_flag(&path, "tag-log", e.is_expanded()));
    expander.upcast()
}
