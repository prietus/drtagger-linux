//! Small widget helpers shared by the inspector sections.

use gtk::prelude::*;

/// A titled card, as the Mac app's GroupBox.
pub fn group(title: &str, body: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let t = gtk::Label::builder().label(title).xalign(0.0).build();
    t.add_css_class("heading");
    b.append(&t);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.add_css_class("group-card");
    card.append(body);
    b.append(&card);
    b.upcast()
}

/// Two columns: dimmed names, values.
pub fn grid(rows: &[(String, String)]) -> gtk::Widget {
    let g = gtk::Grid::builder().column_spacing(16).row_spacing(6).build();
    for (i, (name, value)) in rows.iter().enumerate() {
        let n = gtk::Label::builder().label(name).xalign(0.0).yalign(0.0).build();
        n.add_css_class("dim-label");
        let v = gtk::Label::builder().label(value).xalign(0.0).wrap(true).selectable(true).hexpand(true).build();
        g.attach(&n, 0, i as i32, 1, 1);
        g.attach(&v, 1, i as i32, 1, 1);
    }
    g.upcast()
}

pub fn dim(text: &str) -> gtk::Label {
    let l = gtk::Label::builder().label(text).xalign(0.0).wrap(true).build();
    l.add_css_class("dim-label");
    l
}

pub fn selectable(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).selectable(true).build()
}

pub fn error_line(text: &str) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.add_css_class("error");
    row.append(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
    row.append(&gtk::Label::builder().label(text).xalign(0.0).wrap(true).selectable(true).build());
    row.upcast()
}
