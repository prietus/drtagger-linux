//! Tags written to freshly split tracks straight from the CUE sheet, so the
//! files are playable and sortable before any online identification.

use flackit::{FLACFile, FileSource, FlacError, VorbisComment};
use librarykit::{CueSheet, DiscTOC, SplitTrack};
use std::path::Path;
use std::sync::RwLock;

static VENDOR: RwLock<String> = RwLock::new(String::new());

/// The Vorbis vendor of provisional tags ("drtagger for Linux").
pub fn vendor() -> String {
    let v = VENDOR.read().unwrap();
    if v.is_empty() { "drtagger for Linux".to_string() } else { v.clone() }
}

/// Overrides the vendor string (the parity harness uses the Mac app's).
pub fn set_vendor(vendor: &str) {
    *VENDOR.write().unwrap() = vendor.to_string();
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlbumContext {
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub date: Option<String>,
    pub genre: Option<String>,
    pub disc_number: Option<u32>,
    pub disc_total: Option<u32>,
    /// FreeDB id (REM DISCID).
    pub disc_id: Option<String>,
    pub musicbrainz_disc_id: Option<String>,
    pub barcode: Option<String>,
}

impl AlbumContext {
    pub fn from_cue(cue: &CueSheet, disc_number: Option<u32>, disc_total: Option<u32>, toc: Option<&DiscTOC>) -> Self {
        AlbumContext {
            album: cue.title.clone(),
            album_artist: cue.performer.clone(),
            date: cue.date().map(str::to_string),
            genre: cue.genre().map(str::to_string),
            disc_number,
            disc_total,
            disc_id: cue.disc_id().map(str::to_string).or_else(|| toc.map(DiscTOC::freedb_disc_id)),
            musicbrainz_disc_id: toc.map(DiscTOC::musicbrainz_disc_id),
            barcode: cue.barcode_hint(),
        }
    }
}

pub fn comment(track: &SplitTrack, track_total: usize, album: &AlbumContext) -> VorbisComment {
    let mut fields = vec![];
    let mut add = |name: &str, value: Option<&str>| {
        if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
            fields.push((name.to_string(), v.to_string()));
        }
    };
    let title = if track.is_hidden_track() {
        "Hidden Track".to_string()
    } else {
        track.title.clone().unwrap_or_else(|| format!("Track {}", track.number))
    };
    add("TITLE", Some(&title));
    add("ARTIST", track.performer.as_deref().or(album.album_artist.as_deref()));
    add("ALBUM", album.album.as_deref());
    add("ALBUMARTIST", album.album_artist.as_deref());
    add("TRACKNUMBER", Some(&track.number.to_string()));
    add("TRACKTOTAL", Some(&track_total.to_string()));
    if let Some(n) = album.disc_number {
        add("DISCNUMBER", Some(&n.to_string()));
    }
    if let Some(t) = album.disc_total {
        add("DISCTOTAL", Some(&t.to_string()));
    }
    add("DATE", album.date.as_deref());
    add("GENRE", album.genre.as_deref());
    add("ISRC", track.isrc.as_deref());
    add("BARCODE", album.barcode.as_deref());
    add("MUSICBRAINZ_DISCID", album.musicbrainz_disc_id.as_deref());
    add("DISCID", album.disc_id.as_deref());
    VorbisComment::new(vendor(), fields)
}

/// Rewrites the file's metadata section with `comment`, audio untouched,
/// through a temporary file that replaces the original.
pub fn write(comment: &VorbisComment, path: &Path) -> Result<(), FlacError> {
    let data = {
        let source = FileSource::open(path)?;
        FLACFile::parse(&source, None)?.rewritten(comment, &source)?
    };
    let name = path.file_name().ok_or(FlacError::Truncated)?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}
