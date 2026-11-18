//! The inspector's step sections, in the Mac app's order. Each returns
//! None when it has nothing to show for the album.

mod common;
mod disc;
mod identification;
mod release_set;
mod tags;
mod workflow;

use crate::inspector::Section;

pub fn all() -> Vec<Section> {
    vec![
        crate::batch::review_banner,
        workflow::section,
        release_set::section,
        disc::sacd_section,
        identification::section,
        disc::identity_section,
        tags::section,
    ]
}
