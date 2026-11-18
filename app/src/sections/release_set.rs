//! Multi-disc releases (the Mac app's ReleaseSetView): the discs of a set
//! (several records) or of a folder album (CD1/CD2…), with grouping
//! controls and one-click split/extract of every disc.

use super::common;
use crate::sidebar::state_icon;
use crate::state::{Job, State, job};
use crate::ui;
use drtagger_core::i18n::{tr, trf};
use drtagger_core::store::AlbumRecord;
use gtk::glib;
use gtk::prelude::*;
use librarykit::AlbumKind;

pub fn section(state: &State, record: &AlbumRecord) -> Option<gtk::Widget> {
    let detected = record.detected()?;
    let members = state.members(record);
    let siblings = if record.set_id.is_none() { state.siblings(record) } else { vec![] };
    let mut folder_discs = detected.discs.clone();
    folder_discs.sort_by_key(|d| d.number);
    let position = detected.disc_position();
    let visible = record.set_id.is_some()
        || folder_discs.len() > 1
        || !siblings.is_empty()
        || position.and_then(|p| p.1).unwrap_or(0) > 1;
    if !visible {
        return None;
    }
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let busy = members.iter().any(|m| state.is_busy(&job(&m.path, Job::Disc)));

    // Header: what this is, and the grouping buttons.
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let summary = if record.set_id.is_some() {
        let total = record.set_total.unwrap_or(members.len() as i64) as usize;
        if total > members.len() {
            common::status(
                "dialog-warning-symbolic",
                &trf("{} of {} discs in the queue", &[&members.len(), &total]),
                Some("warning"),
            )
        } else {
            dim_status("view-dual-symbolic", &trf("{} discs, one release", &[&members.len()]))
        }
    } else if folder_discs.len() > 1 {
        dim_status("view-dual-symbolic", &trf("{} discs in this folder", &[&folder_discs.len()]))
    } else if let Some((number, Some(total))) = position {
        common::status(
            "dialog-warning-symbolic",
            &trf("This is disc {} of {}; the other discs are not in the queue", &[&number, &total]),
            Some("warning"),
        )
    } else {
        gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast()
    };
    summary.set_hexpand(true);
    header.append(&summary);
    if !siblings.is_empty() {
        let b = common::button(&trf("Group with {} sibling(s)", &[&siblings.len()]), false);
        b.set_tooltip_text(Some(&tr("Treat the discs found in the same folder as one release")));
        let mut paths = vec![record.path.clone()];
        paths.extend(siblings.iter().map(|s| s.path.clone()));
        b.connect_clicked(glib::clone!(
            #[weak]
            state,
            move |_| state.group(&paths)
        ));
        header.append(&b);
    }
    if record.set_id.is_some() {
        if members.iter().any(|m| m.is_splittable() && m.split_outcomes().is_empty()) {
            let b = common::button(&tr("Split All Discs…"), false);
            b.set_sensitive(!busy);
            connect_run_all(state, record, &b, true);
            header.append(&b);
        }
        if members.iter().any(|m| m.kind == AlbumKind::SacdIso && m.sacd_outcomes().is_empty()) {
            let b = common::button(&tr("Extract All Discs…"), false);
            b.set_sensitive(!busy);
            connect_run_all(state, record, &b, false);
            header.append(&b);
        }
        let b = common::button(&tr("Ungroup"), false);
        b.set_sensitive(!busy);
        let r = record.clone();
        b.connect_clicked(glib::clone!(
            #[weak]
            state,
            move |_| state.ungroup(&r)
        ));
        header.append(&b);
    }
    body.append(&header);

    if record.set_id.is_some() {
        for m in &members {
            body.append(&member_row(state, record, m));
        }
    } else if folder_discs.len() > 1 {
        for d in &folder_discs {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let n = gtk::Label::builder().label(trf("Disc {}", &[&d.number])).xalign(0.0).width_chars(8).build();
            let name = d
                .cue_url
                .as_ref()
                .or(Some(&d.folder))
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let label = gtk::Label::builder()
                .label(name)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            let tracks = tag(&trf("{} tracks", &[&d.track_count()]));
            row.append(&n);
            row.append(&label);
            row.append(&tracks);
            body.append(&row);
        }
    }
    Some(ui::group(&tr("Release set"), &body))
}

/// A caption on one line (rows).
fn tag(text: &str) -> gtk::Label {
    let l = common::caption(text);
    l.set_wrap(false);
    l
}

fn dim_status(icon: &str, text: &str) -> gtk::Widget {
    let w = common::status(icon, text, None);
    w.add_css_class("dim-label");
    w
}

fn member_row(state: &State, shown: &AlbumRecord, m: &AlbumRecord) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let disc = gtk::Label::new(Some(&tr("Disc")));
    let spin = gtk::SpinButton::with_range(1.0, 99.0, 1.0);
    spin.set_value(m.set_position.unwrap_or(1) as f64);
    spin.set_valign(gtk::Align::Center);
    let record = m.clone();
    spin.connect_value_changed(glib::clone!(
        #[weak]
        state,
        move |s| {
            let value = s.value() as i64;
            if Some(value) != record.set_position {
                // After the spin button has finished its own signal.
                let record = record.clone();
                glib::idle_add_local_once(move || state.set_disc_position(&record, value));
            }
        }
    ));
    row.append(&disc);
    row.append(&spin);
    let title = gtk::Label::builder()
        .label(&m.display_title)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    row.append(&title);
    if m.path == shown.path {
        row.append(&tag(&tr("(this)")));
    }
    row.append(&tag(&trf("{} tracks", &[&m.track_count])));
    let icon = gtk::Image::from_icon_name(state_icon(m.state));
    icon.add_css_class("dim-label");
    icon.set_tooltip_text(Some(&tr(m.state.label())));
    row.append(&icon);
    let show = gtk::Button::from_icon_name("go-next-symbolic");
    show.add_css_class("flat");
    show.set_tooltip_text(Some(&tr("Show this disc")));
    let path = m.path.clone();
    show.connect_clicked(glib::clone!(
        #[weak]
        state,
        move |_| state.select(Some(path.clone()))
    ));
    row.append(&show);
    row.upcast()
}

/// Split or extract every member that still needs it, into the library.
fn connect_run_all(state: &State, record: &AlbumRecord, button: &gtk::Button, split: bool) {
    let record = record.clone();
    button.connect_clicked(glib::clone!(
        #[weak]
        state,
        move |b| {
            let state = state.clone();
            let record = record.clone();
            let parent = b.clone().upcast::<gtk::Widget>();
            glib::spawn_future_local(async move {
                let Some(root) =
                    state.destination(&parent, &tr("Choose the library folder where the discs are written.")).await
                else {
                    return;
                };
                for m in state.members(&record) {
                    if split && m.is_splittable() && m.split_outcomes().is_empty() {
                        state.split(&m, root.clone(), false).await;
                    } else if !split && m.kind == AlbumKind::SacdIso && m.sacd_outcomes().is_empty() {
                        state.extract(&m, root.clone(), false).await;
                    }
                }
            });
        }
    ));
}
