//! The batch over the queue in the app (the Mac app's BatchViews): the
//! glue between the core runner and the views, the start dialog, the
//! sidebar's filter and status line, the review banner and the table.

use crate::album_object::AlbumObject;
use crate::state::{AppState, Event, State};
use adw::prelude::*;
use drtagger_core::batch::{self as core, BatchCounts, BatchEvent, BatchStatus, Mode, describe};
use drtagger_core::i18n::{tr, trf};
use drtagger_core::store::AlbumRecord;
use drtagger_core::tagging::chosen_candidate;
use gtk::glib;
use gtk::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// Which albums the sidebar (or the table) shows while a batch exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BatchFilter {
    #[default]
    All,
    Review,
    Failed,
    Done,
}

impl BatchFilter {
    pub fn matches(self, r: &AlbumRecord) -> bool {
        let s = BatchStatus::of(r);
        match self {
            BatchFilter::All => true,
            BatchFilter::Review => s == Some(BatchStatus::Review),
            BatchFilter::Failed => s == Some(BatchStatus::Failed),
            BatchFilter::Done => s == Some(BatchStatus::Done),
        }
    }

    fn name(self) -> &'static str {
        match self {
            BatchFilter::All => "all",
            BatchFilter::Review => "review",
            BatchFilter::Failed => "failed",
            BatchFilter::Done => "done",
        }
    }

    fn from_name(name: &str) -> BatchFilter {
        match name {
            "review" => BatchFilter::Review,
            "failed" => BatchFilter::Failed,
            "done" => BatchFilter::Done,
            _ => BatchFilter::All,
        }
    }
}

fn tool(settings: &drtagger_core::settings::Settings) -> Option<splitkit::FFmpegTool> {
    drtagger_core::ffmpeg::locate(&settings.ffmpeg_override_path)
        .filter(|l| l.ffprobe.is_some())
        .map(|l| drtagger_core::ffmpeg::tool(&l))
}

impl AppState {
    /// Worker reports come back to the main loop: records are read again,
    /// the batch views redraw.
    pub(crate) fn connect_batch(self: &Rc<Self>) {
        let (tx, rx) = async_channel::unbounded::<BatchEvent>();
        self.batch.set_listener(Arc::new(move |e| {
            let _ = tx.send_blocking(e);
        }));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(event) = rx.recv().await {
                let Some(state) = weak.upgrade() else { break };
                match event {
                    BatchEvent::Records(paths) => {
                        for p in paths {
                            state.refresh(&p);
                        }
                    }
                    BatchEvent::Activity => {}
                    BatchEvent::Finished => {
                        state.reload();
                        state.report_inbox();
                    }
                }
                state.emit(Event::Batch);
            }
        });
        // The time left moves on its own while a batch runs.
        let weak = Rc::downgrade(self);
        glib::timeout_add_seconds_local(10, move || {
            let Some(state) = weak.upgrade() else { return glib::ControlFlow::Break };
            if state.batch.is_running() {
                state.emit(Event::Batch);
            }
            glib::ControlFlow::Continue
        });
    }

    /// From the list in memory, not the store: the views ask often.
    pub fn batch_counts(&self) -> BatchCounts {
        let mut c = BatchCounts::default();
        for o in (0..self.albums.n_items()).filter_map(|i| self.albums.item(i).and_downcast::<AlbumObject>()) {
            let Some(s) = o.with(BatchStatus::of) else { continue };
            c.total += 1;
            match s {
                BatchStatus::Done => c.done += 1,
                BatchStatus::Review => c.review += 1,
                BatchStatus::Failed => c.failed += 1,
                _ => c.open += 1,
            }
        }
        c
    }

    pub fn batch_mode(&self) -> Mode {
        Mode::from_raw(&self.settings.string("batch-mode"))
    }

    pub fn start_batch(self: &Rc<Self>, mode: Mode, dry_run: bool) {
        let _ = self.settings.set_string("batch-mode", mode.raw());
        let _ = self.settings.set_boolean("batch-dry-run", dry_run);
        let settings = self.core_settings();
        let tool = tool(&settings);
        let started = self.batch.start(&self.store.borrow(), mode, dry_run, settings, tool);
        if let Err(e) = started {
            tracing::warn!("batch: {e:#}");
        }
        self.reload();
        self.emit(Event::Batch);
    }

    pub fn resume_batch(self: &Rc<Self>) {
        let settings = self.core_settings();
        let tool = tool(&settings);
        let dry_run = self.settings.boolean("batch-dry-run");
        self.batch.resume(&self.store.borrow(), self.batch_mode(), dry_run, settings, tool);
        self.emit(Event::Batch);
    }

    pub fn stop_batch(&self) {
        self.batch.stop();
        self.emit(Event::Batch);
    }

    /// Forgets the batch statuses; the albums stay in the queue.
    pub fn close_batch(&self) {
        if self.batch.is_running() {
            return;
        }
        let _ = core::clear(&self.store.borrow());
        self.batch_filter.set(BatchFilter::All);
        self.show_batch_table.set(false);
        self.reload();
        self.emit(Event::Batch);
    }

    /// Marks an album the batch set aside done, writing nothing.
    pub fn leave_as_is(&self, record: &AlbumRecord) {
        let mut members = self.members(record);
        let _ = core::leave_as_is(&self.store.borrow(), &mut members);
        for m in &members {
            self.refresh(&m.path);
        }
        self.emit(Event::Batch);
    }

    /// The next album set aside after `record`, in queue order.
    pub fn next_to_review(&self, record: &AlbumRecord) -> Option<PathBuf> {
        let open: Vec<PathBuf> = (0..self.albums.n_items())
            .filter_map(|i| self.albums.item(i).and_downcast::<AlbumObject>())
            .filter(|o| o.with(|r| matches!(BatchStatus::of(r), Some(BatchStatus::Review | BatchStatus::Failed))))
            .map(|o| o.path())
            .collect();
        if let Some(i) = open.iter().position(|p| *p == record.path)
            && i + 1 < open.len()
        {
            return Some(open[i + 1].clone());
        }
        open.into_iter().find(|p| *p != record.path)
    }

    // MARK: Release sets

    fn after_regroup(&self, paths: &[PathBuf]) {
        for p in paths {
            self.set_plan(p, None);
        }
        self.reload();
    }

    pub fn group(&self, paths: &[PathBuf]) {
        if let Err(e) = drtagger_core::sets::group(&self.store.borrow(), paths) {
            tracing::warn!("group: {e:#}");
        }
        self.after_regroup(paths);
    }

    pub fn ungroup(&self, record: &AlbumRecord) {
        let paths: Vec<PathBuf> = self.members(record).into_iter().map(|m| m.path).collect();
        if let Err(e) = drtagger_core::sets::ungroup(&self.store.borrow(), &record.path) {
            tracing::warn!("ungroup: {e:#}");
        }
        self.after_regroup(&paths);
    }

    pub fn set_disc_position(&self, record: &AlbumRecord, position: i64) {
        let paths: Vec<PathBuf> = self.members(record).into_iter().map(|m| m.path).collect();
        let _ = drtagger_core::sets::set_position(&self.store.borrow(), &record.path, position);
        for p in &paths {
            self.refresh(p);
        }
    }

    /// Ungrouped albums in the same folder with a disc number: what "Group
    /// with folder siblings" would join.
    pub fn siblings(&self, record: &AlbumRecord) -> Vec<AlbumRecord> {
        drtagger_core::sets::sibling_candidates(&self.records(), record)
    }
}

// MARK: Filter

fn filter_labels(counts: &BatchCounts) -> [(BatchFilter, String); 4] {
    [
        (BatchFilter::All, tr("All")),
        (BatchFilter::Review, trf("Review {}", &[&counts.review])),
        (BatchFilter::Failed, trf("Errors {}", &[&counts.failed])),
        (BatchFilter::Done, trf("Done {}", &[&counts.done])),
    ]
}

/// All / Review n / Errors n / Done n.
fn filter_group(get: impl Fn() -> BatchFilter + 'static, set: impl Fn(BatchFilter) + 'static) -> adw::ToggleGroup {
    let group = adw::ToggleGroup::new();
    for (f, label) in filter_labels(&BatchCounts::default()) {
        group.add(adw::Toggle::builder().name(f.name()).label(label).build());
    }
    group.set_active_name(Some(get().name()));
    group.connect_active_name_notify(move |g| {
        let f = BatchFilter::from_name(g.active_name().as_deref().unwrap_or("all"));
        if f != get() {
            set(f);
        }
    });
    group
}

fn relabel(group: &adw::ToggleGroup, counts: &BatchCounts, current: BatchFilter) {
    for (f, label) in filter_labels(counts) {
        if let Some(t) = group.toggle_by_name(f.name()) {
            t.set_label(Some(&label));
        }
    }
    if group.active_name().as_deref() != Some(current.name()) {
        group.set_active_name(Some(current.name()));
    }
}

/// The sidebar's filter while a batch exists; `changed` re-filters the list.
pub fn sidebar_filter(state: &State, changed: impl Fn() + 'static) -> gtk::Widget {
    let changed = Rc::new(changed);
    let group = filter_group(
        glib::clone!(
            #[weak]
            state,
            #[upgrade_or_default]
            move || state.batch_filter.get()
        ),
        glib::clone!(
            #[weak]
            state,
            #[strong]
            changed,
            move |f| {
                state.batch_filter.set(f);
                changed();
            }
        ),
    );
    group.set_margin_start(8);
    group.set_margin_end(8);
    group.set_margin_bottom(6);
    group.add_css_class("round");
    let update = move |group: &adw::ToggleGroup, state: &State| {
        let counts = state.batch_counts();
        group.set_visible(counts.total > 0);
        if counts.total == 0 && state.batch_filter.get() != BatchFilter::All {
            state.batch_filter.set(BatchFilter::All);
        }
        relabel(group, &counts, state.batch_filter.get());
    };
    update(&group, state);
    let weak = Rc::downgrade(state);
    state.listen_for(&group, move |group, e| {
        if let Some(state) = weak.upgrade()
            && matches!(e, Event::Batch | Event::Queue | Event::Record(_))
        {
            update(group, &state);
            if state.batch_filter.get() != BatchFilter::All {
                changed();
            }
        }
    });
    group.upcast()
}

// MARK: Status line

fn eta_text(d: std::time::Duration) -> String {
    let minutes = d.as_secs().div_ceil(60);
    let text = if minutes >= 60 {
        trf("{} h {} min", &[&(minutes / 60), &(minutes % 60)])
    } else {
        trf("{} min", &[&minutes.max(1)])
    };
    trf("About {} left", &[&text])
}

/// The sidebar's status bar while a batch exists.
pub fn status_line(state: &State) -> gtk::Widget {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let bar = gtk::ProgressBar::builder().width_request(90).valign(gtk::Align::Center).build();
    let count = gtk::Label::new(None);
    count.add_css_class("numeric");
    let review_icon = gtk::Image::from_icon_name("dialog-question-symbolic");
    let review = gtk::Label::new(None);
    let review_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    review_box.append(&review_icon);
    review_box.append(&review);
    review_box.add_css_class("warning");
    review_box.set_tooltip_text(Some(&tr("To review")));
    let failed_icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    let failed = gtk::Label::new(None);
    let failed_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    failed_box.append(&failed_icon);
    failed_box.append(&failed);
    failed_box.add_css_class("error");
    failed_box.set_tooltip_text(Some(&tr("Errors")));
    let spacer = gtk::Box::builder().hexpand(true).build();
    let button = gtk::Button::new();
    button.add_css_class("flat");
    button.add_css_class("caption");
    for w in [bar.upcast_ref::<gtk::Widget>(), count.upcast_ref(), review_box.upcast_ref(), failed_box.upcast_ref()] {
        row.append(w);
    }
    row.append(&spacer);
    row.append(&button);
    let second = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let preview = gtk::Label::new(Some(&tr("Preview")));
    preview.add_css_class("warning");
    preview.add_css_class("heading");
    let eta = gtk::Label::new(None);
    eta.add_css_class("dim-label");
    second.append(&preview);
    second.append(&eta);
    let activity = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
    activity.add_css_class("dim-label");
    column.append(&row);
    column.append(&second);
    column.append(&activity);
    for l in [&count, &review, &failed, &preview, &eta, &activity] {
        l.add_css_class("caption");
    }

    let action: Rc<Cell<&'static str>> = Rc::new(Cell::new(""));
    button.connect_clicked(glib::clone!(
        #[weak]
        state,
        #[strong]
        action,
        move |_| match action.get() {
            "stop" => state.stop_batch(),
            "resume" => state.resume_batch(),
            "close" => state.close_batch(),
            _ => {}
        }
    ));
    let update = move |column: &gtk::Box, state: &State| {
        let counts = state.batch_counts();
        column.set_visible(counts.total > 0);
        bar.set_fraction(counts.finished() as f64 / counts.total.max(1) as f64);
        count.set_label(&format!("{}/{}", counts.finished(), counts.total));
        review.set_label(&counts.review.to_string());
        review_box.set_visible(counts.review > 0);
        failed.set_label(&counts.failed.to_string());
        failed_box.set_visible(counts.failed > 0);
        let running = state.batch.is_running();
        button.set_sensitive(true);
        button.set_tooltip_text(None);
        if running {
            let stopping = state.batch.is_stopping();
            button.set_label(&if stopping { tr("Stopping…") } else { tr("Stop") });
            button.set_sensitive(!stopping);
            action.set("stop");
        } else if counts.open > 0 {
            button.set_label(&tr("Resume"));
            action.set("resume");
        } else {
            button.set_label(&tr("Close Batch"));
            button.set_tooltip_text(Some(&tr("Forget the batch statuses; the albums stay in the queue")));
            action.set("close");
        }
        preview.set_visible(state.settings.boolean("batch-dry-run"));
        let left = state.batch.estimated_remaining(&counts);
        eta.set_visible(left.is_some());
        if let Some(d) = left {
            eta.set_label(&eta_text(d));
        }
        let line = state.batch.activity().line();
        activity.set_visible(line.is_some());
        if let Some(text) = line {
            activity.set_label(&text);
            activity.set_tooltip_text(Some(&text));
        }
    };
    update(&column, state);
    let weak = Rc::downgrade(state);
    state.listen_for(&column, move |column, e| {
        if let Some(state) = weak.upgrade()
            && matches!(e, Event::Batch | Event::Queue | Event::Record(_))
        {
            update(column, &state);
        }
    });
    column.upcast()
}

// MARK: Start

fn caption(text: &str) -> gtk::Label {
    let l = crate::ui::dim(text);
    l.add_css_class("caption");
    l
}

/// Start a batch: which albums, where they end up.
pub fn start_dialog(state: &State, parent: &impl IsA<gtk::Widget>) {
    let dialog = adw::Dialog::builder().title(tr("Process the Queue")).content_width(560).build();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
    body.set_margin_top(12);
    body.set_margin_bottom(22);
    body.set_margin_start(22);
    body.set_margin_end(22);
    let intro = crate::ui::dim(&tr(
        "Every album is identified; those with a sure match get their files, tags, cover and ReplayGain without asking. The rest are set aside for you to review, and the batch keeps going.",
    ));
    body.append(&intro);

    let import = gtk::CheckButton::with_label(&tr("Import to the library: tag, then move into the library layout"));
    let update_in_place = gtk::CheckButton::with_label(&tr("Update in place: tag the files where they are"));
    update_in_place.set_group(Some(&import));
    let modes = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let destination = gtk::Label::builder().label(tr("Destination")).xalign(0.0).build();
    destination.add_css_class("heading");
    modes.append(&destination);
    modes.append(&import);
    modes.append(&update_in_place);
    body.append(&modes);
    match state.batch_mode() {
        Mode::ImportToLibrary => import.set_active(true),
        Mode::UpdateInPlace => update_in_place.set_active(true),
    }

    let folder_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    folder_row.append(&gtk::Image::from_icon_name("folder-symbolic"));
    let folder = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
    folder_row.append(&folder);
    let choose = gtk::Button::with_label(&tr("Choose…"));
    folder_row.append(&choose);
    body.append(&folder_row);
    body.append(&caption(&tr(
        "SACD images and CD images always get their tracks written to the library folder. Originals are backed up, and every album can be restored afterwards.",
    )));
    let dry_run =
        gtk::CheckButton::with_label(&tr("Preview run: identify and report what would change, write nothing"));
    // A first batch starts as a preview.
    dry_run.set_active(state.settings.boolean("batch-dry-run") || state.batch_counts().total == 0);
    body.append(&dry_run);
    body.append(&caption(&tr(
        "Albums whose files name a different release than the one found, and releases already elsewhere in the library, are always set aside for you.",
    )));

    let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let count = gtk::Label::builder().xalign(0.0).hexpand(true).build();
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let start = gtk::Button::new();
    start.add_css_class("suggested-action");
    bottom.append(&count);
    bottom.append(&cancel);
    bottom.append(&start);
    body.append(&bottom);

    let candidates = core::candidates(&state.records()).len();
    count.set_label(&trf("{} album(s) to process", &[&candidates]));
    let update = Rc::new(glib::clone!(
        #[weak]
        state,
        #[weak]
        folder,
        #[weak]
        start,
        #[weak]
        dry_run,
        move || {
            let root = state.settings.string("library-root");
            folder.remove_css_class("warning");
            folder.remove_css_class("dim-label");
            if root.is_empty() {
                folder.set_label(&tr("No library folder set"));
                folder.add_css_class("warning");
            } else {
                folder.set_label(&root);
                folder.add_css_class("dim-label");
            }
            start.set_label(&if dry_run.is_active() { tr("Start Preview") } else { tr("Start") });
            start.set_sensitive(candidates > 0 && !root.is_empty());
        }
    ));
    update();
    dry_run.connect_toggled(glib::clone!(
        #[strong]
        update,
        move |_| update()
    ));
    choose.connect_clicked(glib::clone!(
        #[weak]
        state,
        #[weak]
        dialog,
        #[strong]
        update,
        move |_| {
            let file_dialog = gtk::FileDialog::builder().title(tr("Choose the library folder.")).modal(true).build();
            let window = dialog.root().and_downcast::<gtk::Window>();
            let update = update.clone();
            glib::spawn_future_local(async move {
                if let Ok(f) = file_dialog.select_folder_future(window.as_ref()).await
                    && let Some(p) = f.path()
                {
                    let _ = state.settings.set_string("library-root", &p.to_string_lossy());
                    update();
                }
            });
        }
    ));
    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    start.connect_clicked(glib::clone!(
        #[weak]
        state,
        #[weak]
        dialog,
        #[weak]
        import,
        #[weak]
        dry_run,
        move |_| {
            let mode = if import.is_active() { Mode::ImportToLibrary } else { Mode::UpdateInPlace };
            state.start_batch(mode, dry_run.is_active());
            dialog.close();
        }
    ));
    dialog.set_default_widget(Some(&start));
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&body));
    dialog.set_child(Some(&view));
    dialog.present(Some(parent));
}

// MARK: Review banner

/// Inspector banner for an album the batch set aside.
pub fn review_banner(state: &State, record: &AlbumRecord) -> Option<gtk::Widget> {
    let status = BatchStatus::of(record).filter(|s| matches!(s, BatchStatus::Review | BatchStatus::Failed))?;
    let review = status == BatchStatus::Review;
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("batch-banner");
    row.add_css_class(if review { "review" } else { "failed" });
    let icon = gtk::Image::from_icon_name(if review { "dialog-question-symbolic" } else { "dialog-warning-symbolic" });
    icon.add_css_class(if review { "warning" } else { "error" });
    icon.set_valign(gtk::Align::Start);
    row.append(&icon);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
    texts.set_hexpand(true);
    let title = gtk::Label::builder()
        .label(if review { tr("Set aside for review") } else { tr("The batch could not finish this album") })
        .xalign(0.0)
        .wrap(true)
        .build();
    title.add_css_class("heading");
    texts.append(&title);
    if let Some(note) = &record.batch_note {
        let l = crate::ui::selectable(note);
        l.add_css_class("dim-label");
        texts.append(&l);
    }
    row.append(&texts);
    let leave = gtk::Button::with_label(&tr("Leave As Is"));
    leave.set_tooltip_text(Some(&tr("Mark it done without writing anything")));
    leave.set_valign(gtk::Align::Start);
    let next = gtk::Button::with_label(&tr("Next to Review"));
    next.set_valign(gtk::Align::Start);
    let following = state.next_to_review(record);
    next.set_sensitive(following.is_some());
    let r = record.clone();
    leave.connect_clicked(glib::clone!(
        #[weak]
        state,
        move |_| {
            let following = state.next_to_review(&r);
            state.leave_as_is(&r);
            if let Some(p) = following {
                state.select(Some(p));
            }
        }
    ));
    next.connect_clicked(glib::clone!(
        #[weak]
        state,
        move |_| {
            if let Some(p) = following.clone() {
                state.select(Some(p));
            }
        }
    ));
    row.append(&leave);
    row.append(&next);
    Some(row.upcast())
}

// MARK: Table

/// One album as the table shows it, computed once per revision.
#[derive(Clone, Debug, Default)]
struct Row {
    status: Option<BatchStatus>,
    album: String,
    artist: String,
    release: String,
    release_id: String,
    confidence: String,
    note: String,
    path: String,
}

impl Row {
    fn new(r: &AlbumRecord) -> Row {
        let chosen = chosen_candidate(r);
        Row {
            status: BatchStatus::of(r),
            album: r.display_title.clone(),
            artist: r.artist_hint.clone().unwrap_or_default(),
            release: chosen.as_ref().map(|c| describe(Some(c))).unwrap_or_default(),
            release_id: chosen.as_ref().map(drtagger_core::library_index::release_key).unwrap_or_default(),
            confidence: r
                .identification()
                .and_then(|i| i.best().map(|b| b.confidence.raw().to_string()))
                .unwrap_or_default(),
            note: r.batch_note.clone().unwrap_or_default(),
            path: r.path.to_string_lossy().into_owned(),
        }
    }

    fn status_text(&self) -> String {
        match self.status {
            Some(BatchStatus::Done) => tr("Done"),
            Some(BatchStatus::Review) => tr("Review"),
            Some(BatchStatus::Failed) => tr("Error"),
            Some(BatchStatus::Working) => tr("Working"),
            _ => tr("Pending"),
        }
    }

    fn icon(&self) -> (&'static str, Option<&'static str>) {
        match self.status {
            Some(BatchStatus::Done) => ("checkbox-checked-symbolic", Some("success")),
            Some(BatchStatus::Review) => ("dialog-question-symbolic", Some("warning")),
            Some(BatchStatus::Failed) => ("dialog-warning-symbolic", Some("error")),
            Some(BatchStatus::Working) => ("emblem-system-symbolic", Some("accent")),
            _ => ("radio-symbolic", None),
        }
    }
}

type RowCache = Rc<RefCell<HashMap<PathBuf, (u64, Row)>>>;

fn row_of(cache: &RowCache, album: &AlbumObject) -> Row {
    let key = album.path();
    if let Some((rev, row)) = cache.borrow().get(&key)
        && *rev == album.revision()
    {
        return row.clone();
    }
    let row = album.with(Row::new);
    cache.borrow_mut().insert(key, (album.revision(), row.clone()));
    row
}

mod cell {
    use super::*;

    #[derive(Default)]
    pub struct Cell {
        pub icon: gtk::Image,
        pub first: gtk::Label,
        pub second: gtk::Label,
        pub bound: RefCell<Option<(AlbumObject, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Cell {
        const NAME: &'static str = "DrtaggerBatchCell";
        type Type = super::BatchCell;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for Cell {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_spacing(6);
            let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
            texts.set_hexpand(true);
            for l in [&self.first, &self.second] {
                l.set_xalign(0.0);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                texts.append(l);
            }
            self.second.add_css_class("caption");
            self.second.add_css_class("dim-label");
            obj.append(&self.icon);
            obj.append(&texts);
        }
    }

    impl WidgetImpl for Cell {}
    impl BoxImpl for Cell {}
}

glib::wrapper! {
    pub struct BatchCell(ObjectSubclass<cell::Cell>) @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

#[derive(Clone, Copy)]
enum Column {
    Status,
    Album,
    Release,
    Match,
    Note,
}

impl BatchCell {
    fn render(&self, row: &Row, column: Column) {
        let imp = self.imp();
        for c in ["success", "warning", "error", "accent"] {
            self.remove_css_class(c);
        }
        imp.icon.set_visible(false);
        imp.second.set_visible(false);
        imp.first.remove_css_class("dim-label");
        imp.first.set_lines(1);
        imp.first.set_wrap(false);
        self.set_tooltip_text(None);
        match column {
            Column::Status => {
                let (icon, class) = row.icon();
                imp.icon.set_icon_name(Some(icon));
                imp.icon.set_visible(true);
                imp.first.set_label(&row.status_text());
                if let Some(c) = class {
                    self.add_css_class(c);
                }
            }
            Column::Album => {
                imp.first.set_label(&row.album);
                imp.second.set_label(&row.artist);
                imp.second.set_visible(!row.artist.is_empty());
            }
            Column::Release | Column::Note => {
                let text = if matches!(column, Column::Release) { &row.release } else { &row.note };
                imp.first.set_label(text);
                imp.first.add_css_class("dim-label");
                imp.first.set_wrap(true);
                imp.first.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                imp.first.set_lines(2);
                if matches!(column, Column::Note) && !text.is_empty() {
                    self.set_tooltip_text(Some(text));
                }
            }
            Column::Match => imp.first.set_label(&row.confidence),
        }
    }

    fn bind(&self, album: &AlbumObject, cache: &RowCache, column: Column) {
        self.unbind();
        self.render(&row_of(cache, album), column);
        let cache = cache.clone();
        let handler = album.connect_notify_local(
            Some("revision"),
            glib::clone!(
                #[weak(rename_to = cell)]
                self,
                move |album, _| cell.render(&row_of(&cache, album), column)
            ),
        );
        self.imp().bound.replace(Some((album.clone(), handler)));
    }

    fn unbind(&self) {
        if let Some((album, handler)) = self.imp().bound.take() {
            album.disconnect(handler);
        }
    }
}

fn column(
    view: &gtk::ColumnView,
    title: &str,
    kind: Column,
    cache: &RowCache,
    key: impl Fn(&Row) -> String + 'static,
    expand: bool,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        item.set_child(Some(&glib::Object::new::<BatchCell>()));
    });
    let bind_cache = cache.clone();
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let (Some(cell), Some(album)) =
            (item.child().and_downcast::<BatchCell>(), item.item().and_downcast::<AlbumObject>())
        {
            cell.bind(&album, &bind_cache, kind);
        }
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(cell) = item.child().and_downcast::<BatchCell>() {
            cell.unbind();
        }
    });
    let sort_cache = cache.clone();
    let sorter = gtk::CustomSorter::new(move |a, b| {
        let (Some(a), Some(b)) = (a.downcast_ref::<AlbumObject>(), b.downcast_ref::<AlbumObject>()) else {
            return gtk::Ordering::Equal;
        };
        let (ka, kb) = (key(&row_of(&sort_cache, a)), key(&row_of(&sort_cache, b)));
        librarykit::natural_cmp(&ka, &kb).into()
    });
    let c = gtk::ColumnViewColumn::builder().title(title).factory(&factory).sorter(&sorter).resizable(true).build();
    c.set_expand(expand);
    view.append_column(&c);
    c
}

/// The batch at a glance: one row per album, sortable, with the release
/// chosen, the confidence and what happened. Activating a row opens it.
pub fn table(state: &State) -> gtk::Widget {
    let cache: RowCache = Rc::default();
    let filter_value: Rc<Cell<BatchFilter>> = Rc::default();
    let filter = gtk::CustomFilter::new(glib::clone!(
        #[strong]
        filter_value,
        move |item| {
            item.downcast_ref::<AlbumObject>()
                .is_some_and(|a| a.with(|r| r.batch_status.is_some() && filter_value.get().matches(r)))
        }
    ));
    let filtered = gtk::FilterListModel::new(Some(state.albums.clone()), Some(filter.clone()));
    let sorted = gtk::SortListModel::new(Some(filtered), None::<gtk::Sorter>);
    let selection = gtk::SingleSelection::builder().model(&sorted).autoselect(false).can_unselect(true).build();
    let view = gtk::ColumnView::new(Some(selection.clone()));
    view.add_css_class("data-table");
    view.set_vexpand(true);
    sorted.set_sorter(view.sorter().as_ref());
    column(&view, &tr("Status"), Column::Status, &cache, |r| r.status_text(), false);
    column(&view, &tr("Album"), Column::Album, &cache, |r| r.album.clone(), true);
    column(&view, &tr("Release"), Column::Release, &cache, |r| r.release.clone(), true);
    column(&view, &tr("Match"), Column::Match, &cache, |r| r.confidence.clone(), false);
    column(&view, &tr("Note"), Column::Note, &cache, |r| r.note.clone(), true);
    view.connect_activate(glib::clone!(
        #[weak]
        state,
        #[weak]
        sorted,
        move |_, position| {
            if let Some(album) = sorted.item(position).and_downcast::<AlbumObject>() {
                state.show_batch_table.set(false);
                state.select(Some(album.path()));
                state.emit(Event::Batch);
            }
        }
    ));
    let scrolled = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();

    let column_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    column_box.set_margin_top(18);
    column_box.set_margin_bottom(18);
    column_box.set_margin_start(18);
    column_box.set_margin_end(18);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let title = gtk::Label::new(None);
    title.add_css_class("title-3");
    let bar = gtk::ProgressBar::builder().width_request(160).valign(gtk::Align::Center).build();
    let count = gtk::Label::new(None);
    count.add_css_class("dim-label");
    count.add_css_class("numeric");
    let spacer = gtk::Box::builder().hexpand(true).build();
    let group = filter_group(
        glib::clone!(
            #[strong]
            filter_value,
            move || filter_value.get()
        ),
        glib::clone!(
            #[strong]
            filter_value,
            #[weak]
            filter,
            move |f| {
                filter_value.set(f);
                filter.changed(gtk::FilterChange::Different);
            }
        ),
    );
    let export = gtk::Button::with_label(&tr("Export Report…"));
    for w in [title.upcast_ref::<gtk::Widget>(), bar.upcast_ref(), count.upcast_ref(), spacer.upcast_ref()] {
        header.append(w);
    }
    header.append(&group);
    header.append(&export);
    column_box.append(&header);
    let frame = gtk::Frame::new(None);
    frame.set_child(Some(&scrolled));
    column_box.append(&frame);

    export.connect_clicked(glib::clone!(
        #[weak]
        sorted,
        #[strong]
        cache,
        move |button| {
            let rows: Vec<Row> = (0..sorted.n_items())
                .filter_map(|i| sorted.item(i).and_downcast::<AlbumObject>())
                .map(|a| row_of(&cache, &a))
                .collect();
            export_report(button, rows);
        }
    ));

    let update = glib::clone!(
        #[weak]
        title,
        #[weak]
        bar,
        #[weak]
        count,
        #[weak]
        group,
        #[weak]
        filter,
        #[strong]
        filter_value,
        move |state: &State| {
            let counts = state.batch_counts();
            title.set_label(&if state.settings.boolean("batch-dry-run") { tr("Batch preview") } else { tr("Batch") });
            bar.set_fraction(counts.finished() as f64 / counts.total.max(1) as f64);
            count.set_label(&trf("{} of {}", &[&counts.finished(), &counts.total]));
            relabel(&group, &counts, filter_value.get());
            filter.changed(gtk::FilterChange::Different);
        }
    );
    update(state);
    let weak = Rc::downgrade(state);
    state.listen_for(&column_box, move |_, e| {
        if let Some(state) = weak.upgrade()
            && matches!(e, Event::Batch | Event::Queue)
        {
            update(&state);
        }
    });
    column_box.upcast()
}

fn export_report(parent: &gtk::Button, rows: Vec<Row>) {
    let today = glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d").ok()).unwrap_or_default();
    let dialog = gtk::FileDialog::builder()
        .title(tr("Export Report…"))
        .initial_name(format!("drtagger batch {today}.csv"))
        .modal(true)
        .build();
    let window = parent.root().and_downcast::<gtk::Window>();
    glib::spawn_future_local(async move {
        let Ok(file) = dialog.save_future(window.as_ref()).await else { return };
        let Some(path) = file.path() else { return };
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut csv = String::from("status,artist,album,path,release,release_id,match,note\n");
        for r in rows {
            let fields = [r.status_text(), r.artist, r.album, r.path, r.release, r.release_id, r.confidence, r.note];
            csv += &fields.iter().map(|f| q(f)).collect::<Vec<_>>().join(",");
            csv.push('\n');
        }
        let _ = std::fs::write(path, csv);
    });
}
