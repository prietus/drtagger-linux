//! Left column: every album in the queue, newest first (the Mac app's
//! QueueSidebar), with a filter, a context menu, an empty state and the
//! scan status line at the bottom.

use crate::album_object::AlbumObject;
use crate::state::{Event, State};
use drtagger_core::i18n::{tr, trf};
use drtagger_core::store::{AlbumRecord, AlbumState};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use librarykit::AlbumKind;
use std::path::PathBuf;

pub fn kind_icon(kind: AlbumKind) -> &'static str {
    match kind {
        AlbumKind::SacdIso => "drive-optical-symbolic",
        AlbumKind::CueImage => "media-optical-symbolic",
        AlbumKind::CueMultiFile => "view-list-symbolic",
        AlbumKind::TrackFolder => "folder-symbolic",
    }
}

pub fn state_icon(state: AlbumState) -> &'static str {
    match state {
        AlbumState::Pending => "document-open-recent-symbolic",
        AlbumState::Scanning => "system-search-symbolic",
        AlbumState::Scanned => "edit-find-symbolic",
        AlbumState::Identifying => "content-loading-symbolic",
        AlbumState::Confident => "object-select-symbolic",
        AlbumState::NeedsReview => "dialog-question-symbolic",
        AlbumState::Applying => "document-edit-symbolic",
        AlbumState::Done => "checkbox-checked-symbolic",
        AlbumState::Error => "dialog-warning-symbolic",
    }
}

/// The style class that colours a state's icon.
pub fn state_class(state: AlbumState) -> Option<&'static str> {
    match state {
        AlbumState::Confident | AlbumState::Done => Some("success"),
        AlbumState::NeedsReview => Some("warning"),
        AlbumState::Error => Some("error"),
        AlbumState::Scanning | AlbumState::Identifying | AlbumState::Applying => Some("accent"),
        _ => None,
    }
}

/// "CD image · 9 tracks · FLAC".
pub fn subtitle(r: &AlbumRecord) -> String {
    let mut parts = vec![tr(r.kind.display_name())];
    if r.track_count > 0 {
        parts.push(if r.track_count == 1 { tr("1 track") } else { trf("{} tracks", &[&r.track_count]) });
    }
    if r.disc_count > 1 {
        parts.push(trf("{} discs", &[&r.disc_count]));
    }
    if r.kind == AlbumKind::SacdIso {
        if r.has_dst {
            parts.push("DST".into());
        }
        if r.has_multichannel {
            parts.push(tr("multichannel"));
        }
    } else {
        let formats: Vec<&str> = r
            .formats
            .split(',')
            .filter_map(|f| {
                librarykit::AudioFormat::ALL.into_iter().find(|a| a.raw_value() == f).map(|f| f.display_name())
            })
            .collect();
        if !formats.is_empty() {
            parts.push(formats.join("/"));
        }
    }
    parts.join(" · ")
}

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct QueueRow {
        pub kind: gtk::Image,
        pub title: gtk::Label,
        pub artist: gtk::Label,
        pub subtitle: gtk::Label,
        pub note: gtk::Label,
        pub badge: gtk::Label,
        pub state: gtk::Image,
        pub bound: RefCell<Option<(AlbumObject, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QueueRow {
        const NAME: &'static str = "DrtaggerQueueRow";
        type Type = super::QueueRow;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for QueueRow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_spacing(10);
            obj.set_margin_top(4);
            obj.set_margin_bottom(4);
            self.kind.add_css_class("dim-label");
            self.kind.set_valign(gtk::Align::Center);
            obj.append(&self.kind);
            let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
            texts.set_hexpand(true);
            for (label, class) in [
                (&self.title, None),
                (&self.artist, Some("caption")),
                (&self.subtitle, Some("caption")),
                (&self.note, Some("caption")),
            ] {
                label.set_xalign(0.0);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                if let Some(c) = class {
                    label.add_css_class(c);
                }
                texts.append(label);
            }
            self.artist.add_css_class("dim-label");
            self.subtitle.add_css_class("dim-label");
            self.subtitle.add_css_class("subtitle-tertiary");
            obj.append(&texts);
            self.badge.add_css_class("caption");
            self.badge.add_css_class("disc-badge");
            self.badge.set_valign(gtk::Align::Center);
            obj.append(&self.badge);
            self.state.set_valign(gtk::Align::Center);
            obj.append(&self.state);
        }
    }

    impl WidgetImpl for QueueRow {}
    impl BoxImpl for QueueRow {}
}

glib::wrapper! {
    pub struct QueueRow(ObjectSubclass<imp::QueueRow>) @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl QueueRow {
    fn new() -> Self {
        glib::Object::new()
    }

    fn bind(&self, album: &AlbumObject) {
        self.unbind();
        self.update(album);
        let handler = album.connect_notify_local(
            Some("revision"),
            glib::clone!(
                #[weak(rename_to = row)]
                self,
                move |album, _| row.update(album)
            ),
        );
        self.imp().bound.replace(Some((album.clone(), handler)));
    }

    fn unbind(&self) {
        if let Some((album, handler)) = self.imp().bound.take() {
            album.disconnect(handler);
        }
    }

    fn update(&self, album: &AlbumObject) {
        let imp = self.imp();
        album.with(|r| {
            imp.kind.set_icon_name(Some(kind_icon(r.kind)));
            imp.title.set_label(&r.display_title);
            let artist = r.artist_hint.clone().unwrap_or_default();
            imp.artist.set_label(&artist);
            imp.artist.set_visible(!artist.is_empty());
            imp.subtitle.set_label(&subtitle(r));
            let note = match r.batch_status.as_deref() {
                Some("working") => Some((r.batch_note.clone().unwrap_or_else(|| tr("Working…")), "accent")),
                Some("review") => Some((r.batch_note.clone().unwrap_or_default(), "warning")),
                Some("failed") => Some((r.batch_note.clone().unwrap_or_default(), "error")),
                _ => None,
            };
            for c in ["accent", "warning", "error"] {
                imp.note.remove_css_class(c);
            }
            match note {
                Some((text, class)) if !text.is_empty() => {
                    imp.note.set_label(&text);
                    imp.note.add_css_class(class);
                    imp.note.set_visible(true);
                }
                _ => imp.note.set_visible(false),
            }
            match r.set_position {
                Some(p) => {
                    imp.badge.set_label(&trf("Disc {}/{}", &[&p, &r.set_total.unwrap_or(p)]));
                    imp.badge.set_visible(true);
                }
                None => imp.badge.set_visible(false),
            }
            imp.state.set_icon_name(Some(state_icon(r.state)));
            imp.state.set_tooltip_text(Some(&tr(r.state.label())));
            for c in ["success", "warning", "error", "accent"] {
                imp.state.remove_css_class(c);
            }
            if let Some(c) = state_class(r.state) {
                imp.state.add_css_class(c);
            }
        });
    }
}

pub struct Sidebar {
    pub widget: gtk::Widget,
    pub selection: gtk::MultiSelection,
    pub search: gtk::SearchEntry,
}

/// The paths highlighted in the list.
pub fn selected_paths(selection: &gtk::MultiSelection) -> Vec<PathBuf> {
    let set = selection.selection();
    (0..set.size())
        .filter_map(|i| selection.item(set.nth(i as u32)).and_downcast::<AlbumObject>())
        .map(|o| o.path())
        .collect()
}

pub fn build(state: &State) -> Sidebar {
    let search = gtk::SearchEntry::builder().placeholder_text(tr("Filter albums")).build();
    search.set_margin_start(8);
    search.set_margin_end(8);
    search.set_margin_bottom(6);

    let filter = gtk::CustomFilter::new(glib::clone!(
        #[weak]
        search,
        #[weak]
        state,
        #[upgrade_or]
        true,
        move |item| {
            let Some(album) = item.downcast_ref::<AlbumObject>() else { return false };
            if !album.with(|r| state.batch_filter.get().matches(r)) {
                return false;
            }
            let q = search.text().trim().to_lowercase();
            if q.is_empty() {
                return true;
            }
            album.with(|r| {
                r.display_title.to_lowercase().contains(&q)
                    || r.artist_hint.as_deref().is_some_and(|a| a.to_lowercase().contains(&q))
                    || r.path.to_string_lossy().to_lowercase().contains(&q)
            })
        }
    ));
    search.connect_search_changed(glib::clone!(
        #[weak]
        filter,
        move |_| filter.changed(gtk::FilterChange::Different)
    ));
    let filtered = gtk::FilterListModel::new(Some(state.albums.clone()), Some(filter.clone()));
    let selection = gtk::MultiSelection::new(Some(filtered));

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(glib::clone!(
        #[weak]
        selection,
        #[weak]
        state,
        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let row = QueueRow::new();
            // Right click: select the row if it is not, then the menu.
            let click = gtk::GestureClick::builder().button(gtk::gdk::BUTTON_SECONDARY).build();
            click.connect_pressed(glib::clone!(
                #[weak]
                row,
                #[weak]
                item,
                #[weak]
                selection,
                #[weak]
                state,
                move |_, _, x, y| {
                    let position = item.position();
                    if !selection.is_selected(position) {
                        selection.select_item(position, true);
                    }
                    show_context_menu(&state, &row, &selection, x, y);
                }
            ));
            row.add_controller(click);
            item.set_child(Some(&row));
        }
    ));
    factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = item.child().and_downcast::<QueueRow>().unwrap();
        if let Some(album) = item.item().and_downcast::<AlbumObject>() {
            row.bind(&album);
        }
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(row) = item.child().and_downcast::<QueueRow>() {
            row.unbind();
        }
    });

    let list = gtk::ListView::new(Some(selection.clone()), Some(factory));
    list.add_css_class("navigation-sidebar");
    let scrolled =
        gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();

    // The inspector follows the highlighted album that was clicked last.
    selection.connect_selection_changed(glib::clone!(
        #[weak]
        state,
        move |selection, _, _| {
            let paths = selected_paths(selection);
            let current = state.selection.borrow().clone();
            if current.as_ref().is_some_and(|c| paths.contains(c)) && paths.len() > 1 {
                return;
            }
            state.select(paths.first().cloned());
        }
    ));
    // Code that selects an album (a scan, an opened folder) highlights it.
    state.listen(glib::clone!(
        #[weak]
        selection,
        #[weak]
        state,
        move |e| {
            if *e != Event::Selection && *e != Event::Queue {
                return;
            }
            let Some(path) = state.selection.borrow().clone() else { return };
            let n = selection.n_items();
            let Some(i) =
                (0..n).find(|&i| selection.item(i).and_downcast::<AlbumObject>().is_some_and(|o| o.path() == path))
            else {
                return;
            };
            if !selection.is_selected(i) {
                selection.select_item(i, true);
            }
        }
    ));

    let delete = gtk::ShortcutController::new();
    delete.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Delete"),
        Some(gtk::NamedAction::new("win.remove-selected")),
    ));
    list.add_controller(delete);

    let empty = adw::StatusPage::builder()
        .icon_name("folder-music-symbolic")
        .title(tr("Queue is Empty"))
        .description(tr("Drop folders here, or use Add Folders… (Ctrl+O)."))
        .build();
    empty.add_css_class("compact");
    let add = gtk::Button::builder()
        .label(tr("Add Folders…"))
        .action_name("win.add-folders")
        .halign(gtk::Align::Center)
        .build();
    add.add_css_class("pill");
    empty.set_child(Some(&add));

    let stack = gtk::Stack::new();
    stack.add_named(&scrolled, Some("list"));
    stack.add_named(&empty, Some("empty"));

    let status = status_bar(state);
    let batch_status = crate::batch::status_line(state);
    batch_status.add_css_class("queue-status");
    let batch_filter = crate::batch::sidebar_filter(
        state,
        glib::clone!(
            #[weak]
            filter,
            move || filter.changed(gtk::FilterChange::Different)
        ),
    );
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.append(&search);
    column.append(&batch_filter);
    column.append(&stack);
    column.append(&status);
    column.append(&batch_status);

    let update_stack = glib::clone!(
        #[weak]
        stack,
        #[weak]
        state,
        move || {
            let empty = state.albums.n_items() == 0 && !state.scan.borrow().is_scanning();
            stack.set_visible_child_name(if empty { "empty" } else { "list" });
        }
    );
    update_stack();
    state.listen(move |e| {
        if matches!(e, Event::Queue | Event::Scan) {
            update_stack();
        }
    });

    Sidebar { widget: column.upcast(), selection, search }
}

fn show_context_menu(state: &State, row: &QueueRow, selection: &gtk::MultiSelection, x: f64, y: f64) {
    let paths = selected_paths(selection);
    let menu = gio::Menu::new();
    if paths.len() > 1 {
        let group = gio::Menu::new();
        group.append(Some(&trf("Group as One Release ({} Discs)", &[&paths.len()])), Some("win.group-selected"));
        menu.append_section(None, &group);
        let remove = gio::Menu::new();
        remove.append(Some(&trf("Remove {} Albums from Queue", &[&paths.len()])), Some("win.remove-selected"));
        menu.append_section(None, &remove);
    } else if let Some(path) = paths.first() {
        let target = path.to_string_lossy().to_variant();
        let item = |label: &str, action: &str| {
            let i = gio::MenuItem::new(Some(label), None);
            i.set_action_and_target_value(Some(action), Some(&target));
            i
        };
        let first = gio::Menu::new();
        first.append_item(&item(&tr("Rescan"), "win.rescan-album"));
        first.append_item(&item(&tr("Open Containing Folder"), "win.open-album-folder"));
        // Siblings are looked up when chosen, not for every row.
        if let Some(record) = state.record(path) {
            if record.set_id.is_none() && record.detected().and_then(|d| d.disc_position()).is_some() {
                first.append_item(&item(&tr("Group with Folder Siblings"), "win.group-siblings"));
            }
            if record.set_id.is_some() {
                first.append_item(&item(&tr("Ungroup Release Set"), "win.ungroup"));
            }
        }
        menu.append_section(None, &first);
        let last = gio::Menu::new();
        last.append(Some(&tr("Remove from Queue")), Some("win.remove-selected"));
        menu.append_section(None, &last);
    }
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(row);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}

fn status_bar(state: &State) -> gtk::Widget {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar.add_css_class("queue-status");
    let spinner = adw::Spinner::new();
    let warning = gtk::Image::from_icon_name("dialog-warning-symbolic");
    warning.add_css_class("warning");
    let label = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::Middle).build();
    label.add_css_class("caption");
    label.add_css_class("dim-label");
    let clear = gtk::Button::builder().label(tr("Clear")).action_name("win.clear-queue").build();
    clear.add_css_class("flat");
    clear.add_css_class("caption");
    clear.set_tooltip_text(Some(&tr("Empty the queue")));
    bar.append(&spinner);
    bar.append(&warning);
    bar.append(&label);
    bar.append(&clear);

    let update = glib::clone!(
        #[weak]
        state,
        #[weak]
        bar,
        #[weak]
        spinner,
        #[weak]
        warning,
        #[weak]
        label,
        #[weak]
        clear,
        move || {
            let scan = state.scan.borrow();
            let count = state.albums.n_items();
            let stalled = !scan.stalled.is_empty() && scan.last_summary.is_some();
            warning.set_visible(stalled);
            spinner.set_visible(scan.is_scanning() && !stalled);
            label.remove_css_class("warning");
            let text = if stalled {
                label.add_css_class("warning");
                scan.last_summary.clone().unwrap_or_default()
            } else if scan.is_scanning() {
                scan.current_folder.as_ref().map_or_else(|| tr("Scanning…"), |f| trf("Scanning {}…", &[f]))
            } else if let Some(s) = &scan.last_summary {
                s.clone()
            } else if count == 1 {
                tr("1 album")
            } else {
                trf("{} albums", &[&count])
            };
            label.set_label(&text);
            label.set_tooltip_text(Some(&text));
            clear.set_visible(count > 0 && !scan.is_scanning());
            // A batch shows its own line instead.
            bar.set_visible(state.batch_counts().total == 0);
        }
    );
    update();
    state.listen(move |e| {
        if matches!(e, Event::Queue | Event::Scan | Event::Batch | Event::Record(_)) {
            update();
        }
    });
    bar.upcast()
}
