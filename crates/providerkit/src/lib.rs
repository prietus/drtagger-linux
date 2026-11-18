//! Metadata providers (port of ProviderKit): MusicBrainz, Discogs and
//! AcoustID for identification; Cover Art Archive, fanart.tv, iTunes and
//! Deezer for artwork; CUETools DB for rip checksums. The clients are
//! blocking and thread-safe; each keeps the rate limit of its service.

pub mod acoustid;
pub mod art;
pub mod candidate;
pub mod ctdb;
pub mod discogs;
pub mod error;
pub mod http;
pub mod keycheck;
pub mod musicbrainz;

pub use acoustid::{AcoustIDClient, AcoustIDMatch};
pub use art::{
    CAAImage, CoverArtArchiveClient, DeezerAlbum, DeezerClient, FanartClient, FanartCover, ITunesAlbum,
    ITunesSearchClient,
};
pub use candidate::{ArtistCredit, Candidate, CandidateMedium, CandidateTrack, Source, TrackCredit, WorkCredit};
pub use ctdb::{
    CUEToolsDBClient, CtdbEntry, CtdbMetadata, CtdbMetadataTrack, CtdbResponse, CtdbTrackVerdict, CtdbVerification,
};
pub use discogs::{DiscogsClient, DiscogsReleaseImage};
pub use error::{ProviderError, Result};
pub use keycheck::KeyCheck;
pub use musicbrainz::MusicBrainzClient;

/// Foundation's `CharacterSet.whitespaces`: spaces and tabs, not newlines.
pub fn is_whitespace(c: char) -> bool {
    c == '\t'
        || (c.is_whitespace() && !matches!(c, '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'))
}

/// `trimmingCharacters(in: .whitespaces)`.
pub fn trim_whitespaces(s: &str) -> &str {
    s.trim_matches(is_whitespace)
}

/// `Character.isNumber`, close enough for the digits providers send.
pub fn is_number(c: char) -> bool {
    c.is_numeric()
}

/// Digits only: `filter(\.isNumber)`.
pub fn digits(s: &str) -> String {
    s.chars().filter(|c| is_number(*c)).collect()
}

/// `String(s.prefix(n))` over characters.
pub fn prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
