use crate::state::{AppState, Event, State};
use crate::{inspector, sidebar};
use adw::prelude::*;
use adw::subclass::prelude::*;
use drtagger_core::i18n::tr;
use gtk::{gdk, gio, glib};
use std::cell::OnceCell;
use std::path::PathBuf;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/us/priet/drtagger/ui/window.ui")]
    pub struct Window {
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub sidebar_view: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub content_view: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub content_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub toasts: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub process_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub stop_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub overview_button: TemplateChild<gtk::ToggleButton>,
        pub settings: OnceCell<gio::Settings>,
        pub state: OnceCell<State>,
        pub selection: OnceCell<gtk::MultiSelection>,
        pub scrolled: OnceCell<gtk::ScrolledWindow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "DrtaggerWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();
            let settings = gio::Settings::new(drtagger_core::APP_ID);
            let obj = self.obj();
            obj.set_default_size(settings.int("window-width"), settings.int("window-height"));
            if settings.boolean("window-maximized") {
                obj.maximize();
            }
            self.settings.set(settings).unwrap();
        }
    }

    impl WidgetImpl for Window {}

    impl WindowImpl for Window {
        fn close_request(&self) -> glib::Propagation {
            let obj = self.obj();
            let settings = self.settings.get().unwrap();
            let (width, height) = obj.default_size();
            let _ = settings.set_int("window-width", width);
            let _ = settings.set_int("window-height", height);
            let _ = settings.set_boolean("window-maximized", obj.is_maximized());
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Window {
    pub fn new(app: &adw::Application) -> anyhow::Result<Self> {
        let window: Self = glib::Object::builder().property("application", app).build();
        let state = AppState::new()?;
        window.imp().state.set(state.clone()).ok();
        window.setup(&state);
        Ok(window)
    }

    pub fn state(&self) -> &State {
        self.imp().state.get().unwrap()
    }

    pub fn toast(&self, message: &str) {
        self.imp().toasts.add_toast(adw::Toast::new(message));
    }

    fn setup(&self, state: &State) {
        let imp = self.imp();
        let side = sidebar::build(state);
        imp.sidebar_view.set_content(Some(&side.widget));
        imp.selection.set(side.selection.clone()).ok();
        let search = side.search.clone();
        let find = gio::ActionEntry::builder("find")
            .activate(move |_: &Window, _, _| {
                search.grab_focus();
            })
            .build();
        self.add_action_entries([find]);
        let inspector = inspector::build(state, crate::sections::all());
        // The batch table takes the inspector's place while it is shown.
        let content = gtk::Stack::new();
        content.add_named(&inspector.widget, Some("inspector"));
        content.add_named(&crate::batch::table(state), Some("batch"));
        imp.content_view.set_content(Some(&content));
        let update_content = glib::clone!(
            #[weak]
            state,
            #[weak]
            content,
            move || {
                let table = state.show_batch_table.get() && state.batch_counts().total > 0;
                content.set_visible_child_name(if table { "batch" } else { "inspector" });
            }
        );
        update_content();
        state.listen(move |e| {
            if matches!(e, Event::Batch | Event::Queue) {
                update_content();
            }
        });
        imp.scrolled.set(inspector.scrolled.clone()).ok();
        self.setup_actions(state);
        self.setup_drop(state);
        crate::inbox::keep_running(state, self);

        // The content page shows the album's title; collapsed, choosing an
        // album opens it.
        state.listen(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[weak]
            state,
            move |e| {
                if matches!(e, Event::Selection | Event::Record(_)) {
                    let title = state.selected_record().map(|r| r.display_title).unwrap_or_else(|| tr("Album"));
                    window.imp().content_page.set_title(&title);
                }
                if *e == Event::Selection && state.selection.borrow().is_some() {
                    window.imp().split_view.set_show_content(true);
                    // Choosing an album in the sidebar shows it, not the table.
                    if state.show_batch_table.get() {
                        state.show_batch_table.set(false);
                        state.emit(Event::Batch);
                    }
                }
                window.update_action_states();
            }
        ));
        self.update_action_states();
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.imp().selection.get().map(sidebar::selected_paths).unwrap_or_default()
    }

    fn update_action_states(&self) {
        let state = self.state();
        let scanning = state.scan.borrow().is_scanning();
        let has_selection = state.selection.borrow().is_some();
        let has_albums = state.albums.n_items() > 0;
        let enable = |name: &str, on: bool| {
            if let Some(a) = self.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                a.set_enabled(on);
            }
        };
        enable("rescan", has_selection);
        enable("identify", has_selection);
        enable("remove-selected", has_selection && !scanning);
        let running = state.batch.is_running();
        enable("clear-queue", has_albums && !scanning && !running);
        enable("process-queue", !scanning && !running);
        enable("stop-batch", running && !state.batch.is_stopping());
        enable("group-selected", !running);
        enable("ungroup", !running);
        enable("group-siblings", !running);
        let imp = self.imp();
        imp.process_button.set_visible(!running);
        imp.stop_button.set_visible(running);
        let has_batch = state.batch_counts().total > 0;
        imp.overview_button.set_visible(has_batch);
        if let Some(a) = self.lookup_action("batch-overview") {
            a.change_state(&(state.show_batch_table.get() && has_batch).to_variant());
        }
    }

    fn add_paths(&self, paths: Vec<PathBuf>) {
        let state = self.state().clone();
        glib::spawn_future_local(async move {
            state.add_roots(paths).await;
        });
    }

    fn setup_actions(&self, state: &State) {
        let add_folders =
            gio::ActionEntry::builder("add-folders").activate(|window: &Window, _, _| window.choose(true)).build();
        let add_files =
            gio::ActionEntry::builder("add-files").activate(|window: &Window, _, _| window.choose(false)).build();
        let rescan = gio::ActionEntry::builder("rescan")
            .activate(|window: &Window, _, _| {
                if let Some(path) = window.state().selection.borrow().clone() {
                    let state = window.state().clone();
                    glib::spawn_future_local(async move { state.rescan(path).await });
                }
            })
            .build();
        let rescan_album = gio::ActionEntry::builder("rescan-album")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|window: &Window, _, param| {
                if let Some(path) = param.and_then(|p| p.get::<String>()) {
                    let state = window.state().clone();
                    glib::spawn_future_local(async move { state.rescan(PathBuf::from(path)).await });
                }
            })
            .build();
        let open_folder = gio::ActionEntry::builder("open-album-folder")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|window: &Window, _, param| {
                if let Some(path) = param.and_then(|p| p.get::<String>()) {
                    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(path)));
                    launcher.open_containing_folder(Some(window), gio::Cancellable::NONE, |_| {});
                }
            })
            .build();
        let remove = gio::ActionEntry::builder("remove-selected")
            .activate(|window: &Window, _, _| {
                let paths = window.selected_paths();
                if !paths.is_empty() {
                    window.state().remove(&paths);
                }
            })
            .build();
        let clear =
            gio::ActionEntry::builder("clear-queue").activate(|window: &Window, _, _| window.confirm_clear()).build();
        let identify = gio::ActionEntry::builder("identify")
            .activate(|window: &Window, _, _| {
                let state = window.state().clone();
                if let Some(r) = state.selected_record() {
                    glib::spawn_future_local(async move { state.identify(&r).await });
                }
            })
            .build();
        let preview = gio::ActionEntry::builder("preview-tags")
            .activate(|window: &Window, _, _| {
                let state = window.state().clone();
                if let Some(r) = state.selected_record() {
                    glib::spawn_future_local(async move { state.build_plan(&r).await });
                }
            })
            .build();
        let apply = gio::ActionEntry::builder("apply-tags")
            .activate(|window: &Window, _, _| {
                let state = window.state().clone();
                if let Some(r) = state.selected_record()
                    && state.plan_for(&r).is_some_and(|p| p.has_work())
                {
                    glib::spawn_future_local(async move { state.apply(&r, None).await });
                }
            })
            .build();
        // Scrolls the inspector to a fraction of its height (screenshots).
        let scroll = gio::ActionEntry::builder("scroll-inspector")
            .parameter_type(Some(glib::VariantTy::DOUBLE))
            .activate(|window: &Window, _, param| {
                if let (Some(f), Some(s)) = (param.and_then(|p| p.get::<f64>()), window.imp().scrolled.get()) {
                    let adj = s.vadjustment();
                    adj.set_value(f * (adj.upper() - adj.page_size()));
                }
            })
            .build();
        // Selects an album by path (scripts and screenshots over D-Bus).
        let select = gio::ActionEntry::builder("select-album")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|window: &Window, _, param| {
                if let Some(path) = param.and_then(|p| p.get::<String>()) {
                    window.state().select(Some(PathBuf::from(path)));
                }
            })
            .build();
        let process = gio::ActionEntry::builder("process-queue")
            .activate(|window: &Window, _, _| {
                let state = window.state();
                state.show_batch_table.set(true);
                state.emit(Event::Batch);
                crate::batch::start_dialog(state, window);
            })
            .build();
        // Starts a batch without the dialog (scripts and screenshots over
        // D-Bus): "import", "update", with ":preview" for a dry run.
        let start = gio::ActionEntry::builder("start-batch")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|window: &Window, _, param| {
                let arg = param.and_then(|p| p.get::<String>()).unwrap_or_default();
                let mode = if arg.starts_with("update") {
                    drtagger_core::batch::Mode::UpdateInPlace
                } else {
                    drtagger_core::batch::Mode::ImportToLibrary
                };
                let state = window.state();
                if state.scan.borrow().is_scanning() {
                    return;
                }
                state.show_batch_table.set(true);
                state.start_batch(mode, arg.ends_with(":preview"));
            })
            .build();
        let stop = gio::ActionEntry::builder("stop-batch")
            .activate(|window: &Window, _, _| window.state().stop_batch())
            .build();
        let overview = gio::ActionEntry::builder("batch-overview")
            .state(false.to_variant())
            .activate(|window: &Window, action, _| {
                let on = !action.state().and_then(|v| v.get::<bool>()).unwrap_or(false);
                let state = window.state();
                state.show_batch_table.set(on);
                action.set_state(&on.to_variant());
                state.emit(Event::Batch);
            })
            .build();
        let group = gio::ActionEntry::builder("group-selected")
            .activate(|window: &Window, _, _| {
                let paths = window.selected_paths();
                window.state().group(&paths);
            })
            .build();
        let group_siblings = gio::ActionEntry::builder("group-siblings")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|window: &Window, _, param| {
                let state = window.state();
                if let Some(r) = param.and_then(|p| p.get::<String>()).and_then(|p| state.record(&PathBuf::from(p))) {
                    let mut paths = vec![r.path.clone()];
                    paths.extend(state.siblings(&r).into_iter().map(|s| s.path));
                    if paths.len() > 1 {
                        state.group(&paths);
                    }
                }
            })
            .build();
        let ungroup = gio::ActionEntry::builder("ungroup")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|window: &Window, _, param| {
                let state = window.state();
                if let Some(r) = param.and_then(|p| p.get::<String>()).and_then(|p| state.record(&PathBuf::from(p))) {
                    state.ungroup(&r);
                }
            })
            .build();
        self.add_action_entries([
            process,
            start,
            stop,
            overview,
            group,
            group_siblings,
            ungroup,
            add_folders,
            add_files,
            rescan,
            rescan_album,
            open_folder,
            remove,
            clear,
            select,
            identify,
            preview,
            apply,
            scroll,
        ]);
        let _ = state;
    }

    fn choose(&self, folders: bool) {
        let window = self.clone();
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder()
                .title(if folders {
                    tr("Choose folders with SACD ISOs, CD images + CUE, or album tracks.")
                } else {
                    tr("Choose SACD ISOs or CD images")
                })
                .accept_label(tr("Add"))
                .modal(true)
                .build();
            let result = if folders {
                dialog.select_multiple_folders_future(Some(&window)).await
            } else {
                dialog.open_multiple_future(Some(&window)).await
            };
            if let Ok(files) = result {
                let paths: Vec<PathBuf> = (0..files.n_items())
                    .filter_map(|i| files.item(i).and_downcast::<gio::File>())
                    .filter_map(|f| f.path())
                    .collect();
                window.add_paths(paths);
            }
        });
    }

    fn confirm_clear(&self) {
        let dialog = adw::AlertDialog::new(
            Some(&tr("Remove every album from the queue?")),
            Some(&tr(
                "Only the queue is cleared. Nothing is deleted from disk; scan results and identifications for these albums are forgotten.",
            )),
        );
        dialog.add_responses(&[("cancel", &tr("Cancel")), ("clear", &tr("Clear Queue"))]);
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let state = self.state().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "clear" {
                state.remove_all();
            }
        });
        dialog.present(Some(self));
    }

    fn setup_drop(&self, _state: &State) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_drop(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                let Ok(list) = value.get::<gdk::FileList>() else { return false };
                let paths: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
                window.add_paths(paths);
                true
            }
        ));
        self.add_controller(target);
    }

    /// Folders opened from another app (or the command line).
    pub fn open_paths(&self, paths: Vec<PathBuf>) {
        let state = self.state().clone();
        glib::spawn_future_local(async move { state.open(paths).await });
    }
}
