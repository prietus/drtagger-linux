//! The Picard tag schema as a flat `TagSet`, readers and writers for every
//! container (FLAC, DSF, WAV, AIFF, DSDIFF, APEv2, MP4), byte-exact backups
//! and restores, the merge policy and the library index. Port of the Mac
//! app's TagKit; the bytes written are the same.
//!
//! Not yet ported (later phases): PicardMapper (needs providerkit) and the
//! artwork resize/re-encode (needs libvips).

mod artwork;
pub mod containers;
pub mod id3_mapper;
mod library_index;
mod library_organizer;
mod picard_mapper;
mod picture;
mod tag_merge;
mod tag_set;
mod tagged_file;

pub use artwork::{ArtworkError, ArtworkProcessor};
pub use containers::ContainerContents;
pub use library_index::{LibraryIndexEntry, LibraryIndexer};
pub use library_organizer::{LibraryOrganizer, Move, OrganizerOptions, SIDECAR_EXTENSIONS, move_item};
pub use picard_mapper::{PicardMapper, TagContext};
pub use picture::{Picture, PictureInfo, PictureKind, dimensions, mime_type};
pub use tag_merge::{ChangeKind, TagChange, TagMerge};
pub use tag_set::{TagSet, field};
pub use tagged_file::{TagBackup, TagContainer, TagFileError, TagWriteReport, TaggedFile};

use std::sync::RwLock;

static VENDOR: RwLock<String> = RwLock::new(String::new());

/// The Vorbis vendor string written into FLAC files that have none.
pub fn vendor() -> String {
    let v = VENDOR.read().unwrap();
    if v.is_empty() { "drtagger for Linux".to_string() } else { v.clone() }
}

/// Overrides the vendor string (the parity harness uses the Mac app's).
pub fn set_vendor(vendor: &str) {
    *VENDOR.write().unwrap() = vendor.to_string();
}
