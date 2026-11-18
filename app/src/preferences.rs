use adw::prelude::*;
use adw::subclass::prelude::*;
use drtagger_core::i18n::{tr, trf};
use drtagger_core::keys::{self, Provider};
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod imp {
    use super::*;
    use std::cell::OnceCell;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/us/priet/drtagger/ui/preferences.ui")]
    pub struct PreferencesDialog {
        #[template_child]
        pub library_root_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub inbox_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub inbox_folder_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub choose_inbox_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub inbox_quiet_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub login_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub choose_root_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub album_template_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub track_template_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub template_preview_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub acoustid_test_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub acoustid_test: TemplateChild<gtk::Button>,
        #[template_child]
        pub discogs_test_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub discogs_test: TemplateChild<gtk::Button>,
        #[template_child]
        pub fanart_test_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub fanart_test: TemplateChild<gtk::Button>,
        #[template_child]
        pub ffmpeg_status_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub ffmpeg_check: TemplateChild<gtk::Button>,
        #[template_child]
        pub ffmpeg_choose: TemplateChild<gtk::Button>,
        #[template_child]
        pub cddb_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub cddb_status_row: TemplateChild<adw::ActionRow>,
        pub state: OnceCell<Option<crate::state::State>>,
        #[template_child]
        pub ascii_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub organize_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub trash_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub keep_queue_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub multichannel_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub pause_policy_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub embed_cover_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub cover_size_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub cover_file_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub genres_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub replaygain_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub acoustid_row: TemplateChild<adw::PasswordEntryRow>,
        #[template_child]
        pub discogs_row: TemplateChild<adw::PasswordEntryRow>,
        #[template_child]
        pub fanart_row: TemplateChild<adw::PasswordEntryRow>,
        #[template_child]
        pub ffmpeg_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub cddb_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub cddb_port_row: TemplateChild<adw::SpinRow>,
        pub settings: OnceCell<gio::Settings>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesDialog {
        const NAME: &'static str = "DrtaggerPreferencesDialog";
        type Type = super::PreferencesDialog;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PreferencesDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let settings = gio::Settings::new(drtagger_core::APP_ID);
            self.settings.set(settings).unwrap();
            let obj = self.obj();
            obj.bind_settings();
            obj.setup_library_root();
            obj.setup_inbox();
            obj.setup_pause_policy();
            obj.setup_cover_size();
            obj.setup_template_preview();
            obj.setup_keys();
            obj.setup_key_tests();
            obj.setup_ffmpeg();
        }
    }

    impl WidgetImpl for PreferencesDialog {}
    impl AdwDialogImpl for PreferencesDialog {}
    impl PreferencesDialogImpl for PreferencesDialog {}
}

glib::wrapper! {
    pub struct PreferencesDialog(ObjectSubclass<imp::PreferencesDialog>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl PreferencesDialog {
    pub fn new(state: Option<crate::state::State>) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.imp().state.set(state).ok();
        dialog.setup_cddb();
        dialog
    }

    fn state(&self) -> Option<crate::state::State> {
        self.imp().state.get().cloned().flatten()
    }

    fn settings(&self) -> &gio::Settings {
        self.imp().settings.get().unwrap()
    }

    fn bind_settings(&self) {
        let imp = self.imp();
        let s = self.settings();
        let bind = |key: &str, widget: &gtk::Widget, prop: &str| s.bind(key, widget, prop).build();
        bind("album-folder-template", imp.album_template_row.upcast_ref(), "text");
        bind("track-file-template", imp.track_template_row.upcast_ref(), "text");
        bind("ascii-file-names", imp.ascii_row.upcast_ref(), "active");
        bind("organize-after-apply", imp.organize_row.upcast_ref(), "active");
        bind("move-originals-to-trash", imp.trash_row.upcast_ref(), "active");
        bind("keep-queue-between-launches", imp.keep_queue_row.upcast_ref(), "active");
        bind("extract-multichannel", imp.multichannel_row.upcast_ref(), "active");
        bind("embed-front-cover", imp.embed_cover_row.upcast_ref(), "active");
        bind("save-cover-file", imp.cover_file_row.upcast_ref(), "active");
        bind("genres-from-discogs", imp.genres_row.upcast_ref(), "active");
        bind("compute-replaygain", imp.replaygain_row.upcast_ref(), "active");
        bind("ffmpeg-override-path", imp.ffmpeg_row.upcast_ref(), "text");
        bind("cddb-server-enabled", imp.cddb_row.upcast_ref(), "active");
        bind("cddb-port", imp.cddb_port_row.upcast_ref(), "value");
        imp.embed_cover_row.bind_property("active", &*imp.cover_size_row, "sensitive").sync_create().build();
        imp.cddb_row.bind_property("active", &*imp.cddb_port_row, "sensitive").sync_create().build();
    }

    fn setup_library_root(&self) {
        let imp = self.imp();
        let show = glib::clone!(
            #[weak(rename_to = row)]
            imp.library_root_row,
            move |settings: &gio::Settings, _: &str| {
                let root = settings.string("library-root");
                row.set_subtitle(&if root.is_empty() { tr("Not set") } else { root.to_string() });
            }
        );
        show(self.settings(), "library-root");
        self.settings().connect_changed(Some("library-root"), show);

        imp.choose_root_button.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                glib::spawn_future_local(async move {
                    let chooser = gtk::FileDialog::builder()
                        .title(tr("Choose the library folder where split tracks are written"))
                        .modal(true)
                        .build();
                    let current = dialog.settings().string("library-root");
                    if !current.is_empty() {
                        chooser.set_initial_folder(Some(&gio::File::for_path(current.as_str())));
                    }
                    let parent = dialog.root().and_downcast::<gtk::Window>();
                    if let Ok(folder) = chooser.select_folder_future(parent.as_ref()).await
                        && let Some(path) = folder.path()
                    {
                        let _ = dialog.settings().set_string("library-root", &path.to_string_lossy());
                    }
                });
            }
        ));
    }

    fn setup_inbox(&self) {
        let imp = self.imp();
        let settings = self.settings();
        settings.bind("inbox-enabled", &*imp.inbox_row, "active").build();
        settings.bind("inbox-quiet-minutes", &*imp.inbox_quiet_row, "value").build();
        settings.bind("start-at-login", &*imp.login_row, "active").build();
        for row in [
            imp.inbox_folder_row.upcast_ref::<gtk::Widget>(),
            imp.inbox_quiet_row.upcast_ref(),
            imp.login_row.upcast_ref(),
        ] {
            imp.inbox_row.bind_property("active", row, "sensitive").sync_create().build();
        }
        let show = glib::clone!(
            #[weak(rename_to = row)]
            imp.inbox_folder_row,
            move |settings: &gio::Settings, _: &str| {
                let folder = settings.string("inbox-folder");
                row.set_subtitle(&if folder.is_empty() { tr("Not set") } else { folder.to_string() });
            }
        );
        show(settings, "inbox-folder");
        settings.connect_changed(Some("inbox-folder"), show);
        imp.choose_inbox_button.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                glib::spawn_future_local(async move {
                    let chooser = gtk::FileDialog::builder()
                        .title(tr("Choose the local folder your ripper writes into"))
                        .modal(true)
                        .build();
                    let parent = dialog.root().and_downcast::<gtk::Window>();
                    if let Ok(folder) = chooser.select_folder_future(parent.as_ref()).await
                        && let Some(path) = folder.path()
                    {
                        let _ = dialog.settings().set_string("inbox-folder", &path.to_string_lossy());
                    }
                });
            }
        ));
    }

    /// The Mac app's four sizes.
    fn setup_cover_size(&self) {
        const SIZES: [i32; 4] = [1000, 1500, 2000, 3000];
        let row = &self.imp().cover_size_row;
        let current = self.settings().int("embed-cover-max-pixels");
        let index = SIZES.iter().position(|s| *s == current).unwrap_or(1);
        row.set_selected(index as u32);
        row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                let _ = dialog.settings().set_int("embed-cover-max-pixels", SIZES[row.selected() as usize % 4]);
            }
        ));
    }

    /// Where a sample track would go with the current templates.
    fn setup_template_preview(&self) {
        let row = self.imp().template_preview_row.get();
        let placeholders: Vec<String> = splitkit::PLACEHOLDERS.iter().map(|p| format!("{{{p}}}")).collect();
        row.set_tooltip_text(Some(&format!(
            "{} {}. {}",
            tr("Placeholders:"),
            placeholders.join(" "),
            tr("Multi-disc releases get a “Disc N” folder automatically.")
        )));
        let update = move |s: &gio::Settings| {
            let sample: std::collections::HashMap<String, String> = [
                ("albumartist", "Miles Davis"),
                ("album", "Kind of Blue"),
                ("year", "1997"),
                ("originalyear", "1959"),
                ("artist", "Miles Davis"),
                ("title", "So What"),
                ("track", "01"),
                ("tracktotal", "5"),
                ("disc", "1"),
                ("disctotal", "1"),
                ("label", "Columbia"),
                ("catalognumber", "CK 64935"),
                ("media", "CD"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
            let ascii = s.boolean("ascii-file-names");
            let folder = splitkit::PathTemplate::render(&s.string("album-folder-template"), &sample, ascii);
            let file = splitkit::PathTemplate::render(&s.string("track-file-template"), &sample, ascii);
            let or_q = |v: String| if v.is_empty() { "?".to_string() } else { v };
            row.set_subtitle(&format!("{}/{}.flac", or_q(folder), or_q(file)));
        };
        update(self.settings());
        for key in ["album-folder-template", "track-file-template", "ascii-file-names"] {
            let update = update.clone();
            self.settings().connect_changed(Some(key), move |s, _| update(s));
        }
    }

    /// "Test" asks each service whether it accepts the key in the field.
    fn setup_key_tests(&self) {
        use providerkit::KeyCheck;
        let imp = self.imp();
        for (provider, button, row) in [
            (Provider::AcoustId, imp.acoustid_test.get(), imp.acoustid_test_row.get()),
            (Provider::Discogs, imp.discogs_test.get(), imp.discogs_test_row.get()),
            (Provider::Fanart, imp.fanart_test.get(), imp.fanart_test_row.get()),
        ] {
            let entry = self.key_row(provider);
            let sync = glib::clone!(
                #[weak]
                button,
                #[weak]
                row,
                move |entry: &adw::PasswordEntryRow| {
                    let has = !entry.text().trim().is_empty();
                    button.set_sensitive(has);
                    if !has {
                        row.set_subtitle(&tr("Enter a key to test it"));
                    }
                }
            );
            sync(&entry);
            entry.connect_changed(sync);
            button.connect_clicked(glib::clone!(
                #[weak]
                entry,
                #[weak]
                row,
                move |button| {
                    let key = entry.text().trim().to_string();
                    button.set_sensitive(false);
                    row.set_subtitle(&tr("Testing…"));
                    for c in ["success", "error", "warning"] {
                        row.remove_css_class(c);
                    }
                    let button = button.clone();
                    glib::spawn_future_local(async move {
                        let ua = drtagger_core::identify::USER_AGENT;
                        let result = gio::spawn_blocking(move || match provider {
                            Provider::AcoustId => providerkit::AcoustIDClient::new(&key, ua).validate_key(),
                            Provider::Discogs => providerkit::DiscogsClient::new(ua, &key).validate_token(),
                            Provider::Fanart => providerkit::FanartClient::new(&key, ua).validate_key(),
                        })
                        .await
                        .unwrap_or_else(|_| KeyCheck::Unreachable(String::new()));
                        let class = match &result {
                            KeyCheck::Valid(_) => "success",
                            KeyCheck::Invalid(_) => "error",
                            KeyCheck::Unreachable(_) => "warning",
                        };
                        row.set_subtitle(&tr(result.message()));
                        row.add_css_class(class);
                        button.set_sensitive(true);
                    });
                }
            ));
        }
    }

    /// Which ffmpeg runs, its version and licence; checked again when the
    /// override changes.
    fn setup_ffmpeg(&self) {
        let imp = self.imp();
        let check = glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move || {
                let row = dialog.imp().ffmpeg_status_row.get();
                let button = dialog.imp().ffmpeg_check.get();
                let path = dialog.settings().string("ffmpeg-override-path").to_string();
                button.set_sensitive(false);
                row.set_subtitle(&tr("Checking…"));
                glib::spawn_future_local(async move {
                    let info = gio::spawn_blocking(move || {
                        let location =
                            drtagger_core::ffmpeg::locate(&path).ok_or_else(|| tr(drtagger_core::ffmpeg::NOT_FOUND))?;
                        drtagger_core::ffmpeg::inspect(&location)
                    })
                    .await
                    .unwrap_or_else(|_| Err(String::new()));
                    row.remove_css_class("warning");
                    row.remove_css_class("error");
                    match info {
                        Ok(info) => {
                            let source = match info.location.source {
                                drtagger_core::ffmpeg::Source::Bundled => tr("bundled"),
                                drtagger_core::ffmpeg::Source::Override => tr("override"),
                                drtagger_core::ffmpeg::Source::System => tr("system"),
                            };
                            let mut parts = vec![source, info.location.ffmpeg.to_string_lossy().into_owned()];
                            if info.is_gpl {
                                parts.push(tr("GPL build, development only"));
                                row.add_css_class("warning");
                            }
                            if !info.has_dst_decoder {
                                parts.push(tr("no DST decoder"));
                            }
                            row.set_subtitle(&format!("{}\n{}", info.version_line, parts.join(" · ")));
                        }
                        Err(e) => {
                            row.add_css_class("error");
                            row.set_subtitle(&e);
                        }
                    }
                    button.set_sensitive(true);
                });
            }
        );
        check();
        let c = check.clone();
        imp.ffmpeg_check.connect_clicked(move |_| c());
        let c = check.clone();
        self.settings().connect_changed(Some("ffmpeg-override-path"), move |_, _| c());
        imp.ffmpeg_choose.connect_clicked(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                glib::spawn_future_local(async move {
                    let chooser =
                        gtk::FileDialog::builder().title(tr("Choose an ffmpeg executable.")).modal(true).build();
                    let parent = dialog.root().and_downcast::<gtk::Window>();
                    if let Ok(file) = chooser.open_future(parent.as_ref()).await
                        && let Some(path) = file.path()
                    {
                        let _ = dialog.settings().set_string("ffmpeg-override-path", &path.to_string_lossy());
                    }
                });
            }
        ));
    }

    /// Instructions for rippers and the live server status.
    fn setup_cddb(&self) {
        let imp = self.imp();
        let group = imp.cddb_group.get();
        let row = imp.cddb_status_row.get();
        let state = self.state();
        let update = move |s: &gio::Settings| {
            let port = s.int("cddb-port");
            group.set_description(Some(&trf(
                "In your ripper's freedb/CDDB settings: server localhost, port {}, path /~cddb/cddb.cgi, HTTP. drtagger must be open while you rip. Answers come from MusicBrainz by disc TOC, best-fitting pressing first; CDDB carries artist, album, year and track titles only, so tag the rip in drtagger afterwards for full metadata.",
                &[&port],
            )));
            row.remove_css_class("success");
            row.remove_css_class("error");
            match state.as_ref().and_then(|st| st.cddb_status()) {
                Some((text, ok)) => {
                    row.set_subtitle(&text);
                    row.add_css_class(if ok { "success" } else { "error" });
                }
                None => row.set_subtitle(&tr("Off")),
            }
        };
        update(self.settings());
        for key in ["cddb-server-enabled", "cddb-port"] {
            let update = update.clone();
            // After the app's own handler has (re)started the server.
            self.settings().connect_changed(Some(key), move |s, _| {
                let (s, update) = (s.clone(), update.clone());
                glib::idle_add_local_once(move || update(&s));
            });
        }
    }

    fn setup_pause_policy(&self) {
        let row = &self.imp().pause_policy_row;
        row.set_selected(self.settings().enum_("sacd-pause-policy") as u32);
        row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            move |row| {
                let _ = dialog.settings().set_enum("sacd-pause-policy", row.selected() as i32);
            }
        ));
    }

    fn key_row(&self, provider: Provider) -> adw::PasswordEntryRow {
        let imp = self.imp();
        match provider {
            Provider::AcoustId => imp.acoustid_row.get(),
            Provider::Discogs => imp.discogs_row.get(),
            Provider::Fanart => imp.fanart_row.get(),
        }
    }

    /// Loads each key from the keyring and stores it as it is typed (as on
    /// Mac, where the field writes the Keychain on every change), a moment
    /// after the last keystroke; the apply button and Enter store at once.
    /// An empty value deletes it.
    fn setup_keys(&self) {
        for provider in Provider::ALL {
            let row = self.key_row(provider);
            row.set_sensitive(false);
            // No saving while the stored value is being shown.
            let loading = Rc::new(Cell::new(true));
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = dialog)]
                self,
                #[weak]
                row,
                #[strong]
                loading,
                async move {
                    match drtagger_core::runtime().spawn(keys::get(provider)).await {
                        Ok(Ok(value)) => row.set_text(value.as_deref().unwrap_or("")),
                        Ok(Err(e)) => dialog.toast(&format!("Keyring: {e:#}")),
                        Err(e) => dialog.toast(&format!("Keyring: {e}")),
                    }
                    loading.set(false);
                    row.set_sensitive(true);
                }
            ));
            let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
            // Takes the value, so a save scheduled before the dialog closes
            // still happens; only the toast needs the dialog.
            let weak = self.downgrade();
            let save: Rc<dyn Fn(String)> = Rc::new(move |value: String| {
                let weak = weak.clone();
                glib::spawn_future_local(async move {
                    let deleting = value.trim().is_empty();
                    let result = drtagger_core::runtime().spawn(async move { keys::set(provider, &value).await }).await;
                    let Some(dialog) = weak.upgrade() else { return };
                    match result {
                        Ok(Ok(())) => {
                            if let Some(state) = dialog.state() {
                                state.load_keys();
                            }
                            dialog.toast(&format!(
                                "{} {}",
                                provider.label(),
                                if deleting { "removed" } else { "saved" }
                            ))
                        }
                        Ok(Err(e)) => dialog.toast(&format!("Keyring: {e:#}")),
                        Err(e) => dialog.toast(&format!("Keyring: {e}")),
                    }
                });
            });
            row.connect_apply(glib::clone!(
                #[strong]
                save,
                #[strong]
                pending,
                move |row| {
                    if let Some(id) = pending.borrow_mut().take() {
                        id.remove();
                    }
                    save(row.text().to_string());
                }
            ));
            row.connect_changed(glib::clone!(
                #[strong]
                save,
                #[strong]
                pending,
                #[strong]
                loading,
                move |row| {
                    if loading.get() {
                        return;
                    }
                    if let Some(id) = pending.borrow_mut().take() {
                        id.remove();
                    }
                    let (save, value, after) = (save.clone(), row.text().to_string(), pending.clone());
                    let id = glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
                        after.borrow_mut().take();
                        save(value);
                    });
                    pending.replace(Some(id));
                }
            ));
        }
    }

    fn toast(&self, message: &str) {
        self.add_toast(adw::Toast::new(message));
    }
}
