//! CUE-based albums: disc IDs from the TOC, CUETools DB status, and the
//! split with its verified results (the Mac app's DiscIdentityView). SACD
//! images: the extraction (SACDExtractView).

use super::common::{activity_line, button, caption, separator, status};
use crate::actions::spawn;
use crate::state::{Job, State, job};
use crate::ui;
use drtagger_core::i18n::{tr, trf};
use drtagger_core::identify::CtdbReport;
use drtagger_core::store::AlbumRecord;
use gtk::glib;
use gtk::prelude::*;
use librarykit::AlbumKind;
use providerkit::CtdbVerification;
use std::path::PathBuf;

fn grid_row(grid: &gtk::Grid, row: i32, name: &str, value: &gtk::Widget) {
    let n = gtk::Label::builder().label(name).xalign(0.0).build();
    n.add_css_class("dim-label");
    grid.attach(&n, 0, row, 1, 1);
    grid.attach(value, 1, row, 1, 1);
}

fn mono(text: &str) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .selectable(true)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    l.add_css_class("monospace");
    l
}

fn folder_line(state: &State, folder: &std::path::Path) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let l = gtk::Label::builder()
        .label(folder.to_string_lossy())
        .xalign(0.0)
        .selectable(true)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    l.add_css_class("caption");
    l.add_css_class("dim-label");
    row.append(&l);
    let open = gtk::Button::from_icon_name("folder-open-symbolic");
    open.add_css_class("flat");
    open.set_tooltip_text(Some(&tr("Open Folder")));
    let (state, folder) = (state.clone(), folder.to_path_buf());
    open.connect_clicked(move |b| {
        let launcher = gtk::FileLauncher::new(Some(&gtk::gio::File::for_path(&folder)));
        let window = b.root().and_downcast::<gtk::Window>();
        launcher.launch(window.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
        let _ = &state;
    });
    row.append(&open);
    row.upcast()
}

fn track_check(verified: bool) -> gtk::Image {
    let i = gtk::Image::from_icon_name(if verified { "object-select-symbolic" } else { "window-close-symbolic" });
    i.add_css_class(if verified { "success" } else { "error" });
    i
}

// MARK: Disc identity

pub fn identity_section(state: &State, r: &AlbumRecord) -> Option<gtk::Widget> {
    if !r.has_cue_sheet() {
        return None;
    }
    // The layout is read when the album is first shown.
    if r.toc().is_none() && !state.is_busy(&job(&r.path, Job::Disc)) && state.flag(&r.path, "toc-tried").is_none() {
        state.set_flag(&r.path, "toc-tried", true);
        let (state, r) = (state.clone(), r.clone());
        glib::idle_add_local_once(move || spawn(async move { state.compute_toc(&r).await }));
    }
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.append(&identity_rows(r));
    body.append(&separator());
    ctdb_rows(state, r, &body);
    if r.is_splittable() {
        body.append(&separator());
        split_rows(state, r, &body);
    }
    body.append(&activity_line(state, vec![job(&r.path, Job::Disc)], false));
    if let Some(e) = state.error(&job(&r.path, Job::Disc)) {
        body.append(&status("dialog-warning-symbolic", &e, Some("error")));
    }
    Some(ui::group(&tr("Disc"), &body))
}

fn identity_rows(r: &AlbumRecord) -> gtk::Widget {
    let Some(toc) = r.toc() else { return ui::dim(&tr("Disc layout not read yet.")).upcast() };
    let grid = gtk::Grid::builder().column_spacing(16).row_spacing(4).build();
    grid_row(&grid, 0, &tr("MusicBrainz Disc ID"), mono(&toc.musicbrainz_disc_id()).upcast_ref());
    let freedb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let id = toc.freedb_disc_id();
    freedb.append(&mono(&id));
    if let Some(rem) = r
        .detected()
        .and_then(|d| d.discs.first().and_then(|d| d.cue.as_ref().and_then(|c| c.disc_id().map(str::to_uppercase))))
    {
        let same = rem == id;
        let icon = gtk::Image::from_icon_name(if same { "object-select-symbolic" } else { "dialog-warning-symbolic" });
        icon.add_css_class(if same { "success" } else { "warning" });
        icon.set_tooltip_text(Some(&if same {
            tr("Matches the CUE's REM DISCID")
        } else {
            trf("CUE says {}", &[&rem])
        }));
        freedb.append(&icon);
    }
    grid_row(&grid, 1, &tr("FreeDB ID"), freedb.upcast_ref());
    let t = mono(&format!("{} · {}", trf("{} tracks", &[&toc.track_count()]), toc.ctdb_toc_string()));
    t.add_css_class("caption");
    grid_row(&grid, 2, "TOC", t.upcast_ref());
    grid.upcast()
}

fn verification_row(v: &CtdbVerification) -> gtk::Widget {
    if v.all_tracks_match() {
        status(
            "object-select-symbolic",
            &trf("All {} tracks match a known rip (confidence {})", &[&v.tracks.len(), &v.total_confidence]),
            Some("success"),
        )
    } else if v.matched_track_count() > 0 {
        status(
            "dialog-warning-symbolic",
            &trf(
                "{} of {} tracks match; the others differ (offset or damage)",
                &[&v.matched_track_count(), &v.tracks.len()],
            ),
            Some("warning"),
        )
    } else {
        status("window-close-symbolic", &tr("No track matches a known rip"), Some("error"))
    }
}

fn ctdb_rows(state: &State, r: &AlbumRecord, body: &gtk::Box) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let name = gtk::Label::new(Some("CUETools DB"));
    name.add_css_class("dim-label");
    row.append(&name);
    let report: Option<CtdbReport> = r.ctdb();
    let verdict: gtk::Widget = match &report {
        Some(rep) if rep.is_known_disc() => status(
            "object-select-symbolic",
            &trf("{} rips known, confidence {}", &[&rep.entry_count, &rep.total_confidence]),
            None,
        ),
        Some(_) => status("dialog-question-symbolic", &tr("Disc not in the database"), Some("dim-label")),
        None => gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast(),
    };
    verdict.set_hexpand(true);
    row.append(&verdict);
    let check = button(&if report.is_none() { tr("Check") } else { tr("Re-check") }, false);
    check.set_sensitive(!state.is_busy(&job(&r.path, Job::Disc)));
    {
        let (state, r) = (state.clone(), r.clone());
        check.connect_clicked(move |_| {
            let (state, r) = (state.clone(), r.clone());
            spawn(async move { state.check_ctdb(&r).await });
        });
    }
    row.append(&check);
    body.append(&row);
    let Some(report) = report else { return };
    if let Some(v) = &report.verification {
        body.append(&verification_row(v));
    }
    if !report.metadata.is_empty() {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 3);
        list.append(&caption(&tr("Releases with this TOC")));
        for m in report.metadata.iter().take(6) {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let icon = gtk::Image::from_icon_name(if m.source == "musicbrainz" {
                "audio-x-generic-symbolic"
            } else {
                "accessories-dictionary-symbolic"
            });
            icon.add_css_class("dim-label");
            line.append(&icon);
            let t = gtk::Label::builder()
                .label(format!("{} – {}", m.artist, m.album))
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            line.append(&t);
            for (value, mono_) in
                [(&m.year, false), (&m.country, false), (&m.catalog_number, false), (&m.barcode, true)]
            {
                if let Some(v) = value {
                    let l = gtk::Label::new(Some(v));
                    l.add_css_class("dim-label");
                    if mono_ {
                        l.add_css_class("monospace");
                    }
                    line.append(&l);
                }
            }
            line.add_css_class("caption");
            list.append(&line);
        }
        body.append(&list);
    }
}

fn split_rows(state: &State, r: &AlbumRecord, body: &gtk::Box) {
    let outcomes = r.split_outcomes();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let name = gtk::Label::new(Some(&tr("Split")));
    name.add_css_class("dim-label");
    row.append(&name);
    let verdict: gtk::Widget = if outcomes.is_empty() {
        ui::dim(&tr("Not split yet")).upcast()
    } else {
        let ok = outcomes.iter().all(|o| o.verified);
        let count: usize = outcomes.iter().map(|o| o.tracks.len()).sum();
        let text = if !ok {
            tr("Split finished with problems")
        } else if outcomes.len() > 1 {
            trf("{} tracks on {} discs written and verified", &[&count, &outcomes.len()])
        } else {
            trf("{} tracks written and verified", &[&count])
        };
        status(
            if ok { "object-select-symbolic" } else { "dialog-warning-symbolic" },
            &text,
            Some(if ok { "success" } else { "warning" }),
        )
    };
    verdict.set_hexpand(true);
    row.append(&verdict);
    let again = !outcomes.is_empty();
    let split = button(&if again { tr("Split Again…") } else { tr("Split to Library…") }, false);
    split.set_sensitive(!state.is_busy(&job(&r.path, Job::Disc)));
    {
        let (state, r) = (state.clone(), r.clone());
        split.connect_clicked(move |b| {
            let (state, r, b) = (state.clone(), r.clone(), b.clone().upcast::<gtk::Widget>());
            spawn(async move {
                if let Some(dest) =
                    state.destination(&b, &tr("Choose the library folder where split tracks are written.")).await
                {
                    state.split(&r, dest, again).await;
                }
            });
        });
    }
    row.append(&split);
    body.append(&row);
    for outcome in &outcomes {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 3);
        b.append(&folder_line(state, &outcome.output_folder));
        b.append(&caption(&format!("{} · FLAC · {:.1} s", outcome.format, outcome.elapsed_seconds)));
        for t in &outcome.tracks {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            line.add_css_class("caption");
            line.append(&track_check(t.verified));
            let n = gtk::Label::new(Some(&format!("{:02}", t.number)));
            n.add_css_class("dim-label");
            n.add_css_class("numeric");
            line.append(&n);
            let f = gtk::Label::builder()
                .label(t.url.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            line.append(&f);
            if let Some(crc) = t.ctdb_crc32 {
                let c = gtk::Label::new(Some(&format!("{crc:08X}")));
                c.add_css_class("monospace");
                c.add_css_class("dim-label");
                line.append(&c);
            }
            b.append(&line);
        }
        for w in &outcome.warnings {
            let s = status("dialog-information-symbolic", w, Some("warning"));
            s.add_css_class("caption");
            b.append(&s);
        }
        body.append(&b);
    }
}

// MARK: SACD extraction

pub fn sacd_section(state: &State, r: &AlbumRecord) -> Option<gtk::Widget> {
    if r.kind != AlbumKind::SacdIso {
        return None;
    }
    let sacd = r.detected()?.sacd?;
    let outcomes = r.sacd_outcomes();
    let settings = state.core_settings();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let verdict: gtk::Widget = if outcomes.is_empty() {
        ui::dim(&tr("Not extracted yet")).upcast()
    } else {
        let ok = outcomes.iter().all(|o| o.verified());
        let count: usize = outcomes.iter().map(|o| o.tracks.len()).sum();
        let text = if ok {
            trf("{} DSF tracks written and verified", &[&count])
        } else {
            tr("Extraction finished with problems")
        };
        status(
            if ok { "object-select-symbolic" } else { "dialog-warning-symbolic" },
            &text,
            Some(if ok { "success" } else { "warning" }),
        )
    };
    verdict.set_hexpand(true);
    row.append(&verdict);
    let again = !outcomes.is_empty();
    let extract = button(&if again { tr("Extract Again…") } else { tr("Extract to Library…") }, false);
    extract.set_sensitive(!state.is_busy(&job(&r.path, Job::Disc)));
    {
        let (state, r) = (state.clone(), r.clone());
        extract.connect_clicked(move |b| {
            let (state, r, b) = (state.clone(), r.clone(), b.clone().upcast::<gtk::Widget>());
            spawn(async move {
                if let Some(dest) =
                    state.destination(&b, &tr("Choose the library folder where extracted tracks are written.")).await
                {
                    state.extract(&r, dest, again).await;
                }
            });
        });
    }
    row.append(&extract);
    body.append(&row);
    let mut policy = vec![];
    if sacd.has_dst() {
        policy.push(tr("DST-compressed, decoded losslessly"));
    }
    policy.push(if settings.sacd_pause_policy == sacdkit::PausePolicy::Drop {
        tr("Pauses between tracks are dropped")
    } else {
        tr("Pauses between tracks stay with the previous track")
    });
    if sacd.multichannel_area().is_some() {
        policy.push(if settings.extract_multichannel {
            tr("multichannel area included")
        } else {
            tr("stereo area only")
        });
    }
    body.append(&caption(&policy.join(" · ")));
    body.append(&activity_line(state, vec![job(&r.path, Job::Disc)], false));
    if let Some(e) = state.error(&job(&r.path, Job::Disc)) {
        body.append(&status("dialog-warning-symbolic", &e, Some("error")));
    }
    for o in &outcomes {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 3);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let area = gtk::Label::new(Some(&if o.is_multichannel {
            trf("Multichannel ({} ch)", &[&o.channel_count])
        } else {
            tr("Stereo")
        }));
        area.add_css_class("caption-heading");
        head.append(&area);
        head.append(&folder_line(state, &o.output_folder));
        b.append(&head);
        let mut line = trf(
            "DSF · {} s · {} lead-in frames skipped",
            &[&format!("{:.1}", o.elapsed_seconds), &o.dropped_lead_in_frames],
        );
        if o.dropped_pause_frames > 0 {
            line += &trf(" · {} pause frames dropped", &[&o.dropped_pause_frames]);
        }
        b.append(&caption(&line));
        for t in &o.tracks {
            let l = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            l.add_css_class("caption");
            l.append(&track_check(t.verified()));
            let n = gtk::Label::new(Some(&format!("{:02}", t.number)));
            n.add_css_class("dim-label");
            l.append(&n);
            let f = gtk::Label::builder()
                .label(t.url.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            l.append(&f);
            let size = gtk::Label::new(Some(&format!(
                "{} · {}",
                trf("{} frames", &[&t.frames]),
                glib::format_size(t.file_size)
            )));
            size.add_css_class("dim-label");
            size.add_css_class("numeric");
            l.append(&size);
            b.append(&l);
        }
        for w in &o.warnings {
            let s = status("dialog-information-symbolic", w, Some("warning"));
            s.add_css_class("caption");
            b.append(&s);
        }
        body.append(&b);
    }
    let _: Option<PathBuf> = None;
    Some(ui::group(&tr("Extract"), &body))
}
