//! Thumbnails of artwork files and remote images, decoded two at a time on
//! worker threads (a slow network volume or server must not hold the
//! interface) and cached.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::rc::Rc;

type Waiter = Box<dyn FnOnce(Option<gdk::Texture>)>;

#[derive(Default)]
struct Loader {
    cache: HashMap<(String, u32), Option<gdk::Texture>>,
    order: VecDeque<(String, u32)>,
    waiting: HashMap<(String, u32), Vec<Waiter>>,
    queue: VecDeque<(String, u32)>,
    running: usize,
}

const MAX_CONCURRENT: usize = 2;
const CACHE_SIZE: usize = 400;

thread_local! {
    static LOADER: Rc<RefCell<Loader>> = Rc::default();
}

/// RGBA pixels as a texture.
pub fn texture(image: &image::RgbImage) -> gdk::Texture {
    let bytes = glib::Bytes::from(image.as_raw());
    gdk::MemoryTexture::new(
        image.width() as i32,
        image.height() as i32,
        gdk::MemoryFormat::R8g8b8,
        &bytes,
        image.width() as usize * 3,
    )
    .upcast()
}

/// Calls `done` with the image at most `max_pixel` on its longest side.
pub fn load(path: &Path, max_pixel: u32, done: impl FnOnce(Option<gdk::Texture>) + 'static) {
    load_source(path.to_string_lossy().into_owned(), max_pixel, done);
}

/// A local path or an http(s) URL.
pub fn load_source(source: String, max_pixel: u32, done: impl FnOnce(Option<gdk::Texture>) + 'static) {
    let key = (source, max_pixel);
    LOADER.with(|loader| {
        let mut l = loader.borrow_mut();
        if let Some(t) = l.cache.get(&key) {
            let t = t.clone();
            drop(l);
            done(t);
            return;
        }
        let first = !l.waiting.contains_key(&key);
        l.waiting.entry(key.clone()).or_default().push(Box::new(done));
        if first {
            l.queue.push_back(key);
        }
        drop(l);
        pump(loader.clone());
    });
}

fn pump(loader: Rc<RefCell<Loader>>) {
    loop {
        let key = {
            let mut l = loader.borrow_mut();
            if l.running >= MAX_CONCURRENT {
                return;
            }
            let Some(k) = l.queue.pop_front() else { return };
            l.running += 1;
            k
        };
        let loader = loader.clone();
        glib::spawn_future_local(async move {
            let (path, size) = key.clone();
            let image = gio::spawn_blocking(move || decode(&path, size)).await.ok().flatten();
            let tex = image.as_ref().map(texture);
            let waiters = {
                let mut l = loader.borrow_mut();
                l.running -= 1;
                l.cache.insert(key.clone(), tex.clone());
                l.order.push_back(key.clone());
                while l.order.len() > CACHE_SIZE {
                    if let Some(old) = l.order.pop_front() {
                        l.cache.remove(&old);
                    }
                }
                l.waiting.remove(&key).unwrap_or_default()
            };
            for w in waiters {
                w(tex.clone());
            }
            pump(loader);
        });
    }
}

fn decode(source: &str, size: u32) -> Option<image::RgbImage> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let request = providerkit::http::Request::get(source)
            .header("User-Agent", drtagger_core::identify::USER_AGENT)
            .timeout(30);
        let response = providerkit::http::default_transport().send(&request).ok()?;
        if response.status != 200 {
            return None;
        }
        visionkit::imaging::load_data(&response.body, size)
    } else {
        visionkit::imaging::load(Path::new(source), size)
    }
}

/// A remote image in a square with a neutral placeholder.
pub fn remote(url: Option<&str>, side: i32) -> gtk::Widget {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .width_request(side)
        .height_request(side)
        .can_shrink(true)
        .build();
    let frame = gtk::Frame::builder().child(&picture).width_request(side).height_request(side).build();
    frame.add_css_class("thumbnail-frame");
    frame.add_css_class("remote-thumbnail");
    frame.set_halign(gtk::Align::Start);
    frame.set_valign(gtk::Align::Start);
    if let Some(url) = url {
        let weak = picture.downgrade();
        load_source(url.to_string(), side as u32 * 2, move |t| {
            if let (Some(p), Some(t)) = (weak.upgrade(), t) {
                p.set_paintable(Some(&t));
            }
        });
    }
    frame.upcast()
}

/// A square thumbnail that fills in when decoded.
pub fn thumbnail(path: &Path, side: i32) -> gtk::Widget {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Contain)
        .width_request(side)
        .height_request(side)
        .can_shrink(true)
        .build();
    picture.add_css_class("thumbnail");
    let frame = gtk::Frame::builder().child(&picture).width_request(side).height_request(side).build();
    frame.add_css_class("thumbnail-frame");
    frame.set_halign(gtk::Align::Start);
    frame.set_valign(gtk::Align::Start);
    let weak = picture.downgrade();
    load(path, side as u32 * 2, move |t| {
        if let (Some(p), Some(t)) = (weak.upgrade(), t) {
            p.set_paintable(Some(&t));
        }
    });
    frame.upcast()
}
