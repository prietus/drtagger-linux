//! Pieces the sections share: a live progress line, the error of an album,
//! status labels with an icon.

use crate::state::{Event, State};
use gtk::prelude::*;
use std::path::PathBuf;

/// The first running activity among `keys`, kept up to date.
pub fn activity_line(state: &State, keys: Vec<PathBuf>, prefix_members: bool) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let bar = gtk::ProgressBar::builder().width_request(220).valign(gtk::Align::Center).build();
    let spinner = adw::Spinner::new();
    let label = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).hexpand(true).build();
    label.add_css_class("dim-label");
    row.append(&bar);
    row.append(&spinner);
    row.append(&label);
    let update = {
        let state = state.clone();
        move |row: &gtk::Box| {
            let found = keys.iter().enumerate().find_map(|(i, k)| state.activity(k).map(|a| (i, a)));
            row.set_visible(found.is_some());
            let Some((i, a)) = found else { return };
            let message = if prefix_members && keys.len() > 1 && i > 0 {
                drtagger_core::i18n::trf("Disc {}: ", &[&i]) + &a.message
            } else {
                a.message
            };
            label.set_label(&message);
            match a.fraction {
                Some(f) => {
                    bar.set_fraction(f.clamp(0.0, 1.0));
                    bar.set_visible(true);
                    spinner.set_visible(false);
                }
                None => {
                    bar.set_visible(false);
                    spinner.set_visible(true);
                }
            }
        }
    };
    update(&row);
    state.listen_for(&row, move |row, e| {
        if matches!(e, Event::Activity(_)) {
            update(row);
        }
    });
    row.upcast()
}

/// A label with an icon, coloured by a style class.
pub fn status(icon: &str, text: &str, class: Option<&str>) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&gtk::Image::from_icon_name(icon));
    row.append(&gtk::Label::builder().label(text).xalign(0.0).wrap(true).build());
    if let Some(c) = class {
        row.add_css_class(c);
    }
    row.upcast()
}

pub fn caption(text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
    l.add_css_class("caption");
    l.add_css_class("dim-label");
    l
}

pub fn button(label: &str, suggested: bool) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    if suggested {
        b.add_css_class("suggested-action");
    }
    b.set_valign(gtk::Align::Center);
    b
}

pub fn separator() -> gtk::Widget {
    gtk::Separator::new(gtk::Orientation::Horizontal).upcast()
}
