//! One queued album in the sidebar's list model: the stored record and a
//! revision counter that rows and the inspector watch.

use drtagger_core::store::AlbumRecord;
use gtk::glib;
use gtk::subclass::prelude::*;
use std::path::PathBuf;

mod imp {
    use super::*;
    use glib::prelude::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::AlbumObject)]
    pub struct AlbumObject {
        pub record: RefCell<Option<AlbumRecord>>,
        #[property(get)]
        pub revision: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumObject {
        const NAME: &'static str = "DrtaggerAlbumObject";
        type Type = super::AlbumObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for AlbumObject {}
}

glib::wrapper! {
    pub struct AlbumObject(ObjectSubclass<imp::AlbumObject>);
}

impl AlbumObject {
    pub fn new(record: AlbumRecord) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().record.replace(Some(record));
        obj
    }

    /// A copy of the current record.
    pub fn record(&self) -> AlbumRecord {
        self.imp().record.borrow().clone().expect("album record")
    }

    pub fn path(&self) -> PathBuf {
        self.imp().record.borrow().as_ref().map(|r| r.path.clone()).unwrap_or_default()
    }

    pub fn with<R>(&self, f: impl FnOnce(&AlbumRecord) -> R) -> R {
        f(self.imp().record.borrow().as_ref().expect("album record"))
    }

    /// Replaces the record and tells every watcher.
    pub fn set_record(&self, record: AlbumRecord) {
        self.imp().record.replace(Some(record));
        self.imp().revision.set(self.imp().revision.get() + 1);
        glib::prelude::ObjectExt::notify(self, "revision");
    }
}
