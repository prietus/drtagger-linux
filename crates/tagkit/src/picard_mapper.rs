//! Candidate → Picard tags. Album fields repeat on every track (that is how
//! Vorbis and ID3 work); track fields come from the medium the local files
//! map to, so a hybrid SACD rip gets the SACD layer's numbering.

use crate::tag_set::{TagSet, field};
use providerkit::{ArtistCredit, Candidate, CandidateMedium, CandidateTrack, Source, TrackCredit};
use std::collections::BTreeMap;

/// What the mapper knows beyond the candidate.
#[derive(Clone, Debug)]
pub struct TagContext {
    /// The Discogs release chosen for credits.
    pub discogs: Option<Candidate>,
    pub musicbrainz_disc_id: Option<String>,
    pub freedb_disc_id: Option<String>,
    /// By 0-based album track index.
    pub acoust_ids: BTreeMap<usize, String>,
    pub fingerprints: BTreeMap<usize, String>,
    pub write_fingerprints: bool,
    pub write_genres: bool,
    pub include_discogs_credits: bool,
    /// The album is known but not the edition the files come from: nothing
    /// that belongs to one pressing is written, and MEDIA says what the
    /// files are.
    pub album_only: bool,
    pub media_override: Option<String>,
}

impl Default for TagContext {
    fn default() -> Self {
        TagContext {
            discogs: None,
            musicbrainz_disc_id: None,
            freedb_disc_id: None,
            acoust_ids: BTreeMap::new(),
            fingerprints: BTreeMap::new(),
            write_fingerprints: false,
            write_genres: true,
            include_discogs_credits: true,
            album_only: false,
            media_override: None,
        }
    }
}

pub struct PicardMapper;

impl PicardMapper {
    pub const VARIOUS_ARTISTS_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

    pub fn album_tags(c: &Candidate, media: &[CandidateMedium], context: &TagContext) -> TagSet {
        let mut t = TagSet::new();
        t.set(field::ALBUM, Some(&c.title));
        t.set(field::ALBUM_ARTIST, Some(&c.artist));
        t.set(field::ALBUM_ARTIST_SORT, c.artist_sort().as_deref());
        for id in c.artist_ids() {
            t.add(field::MB_ALBUM_ARTIST_ID, Some(&id));
        }
        if let Some(original) = &c.first_release_date {
            t.set(field::ORIGINAL_DATE, Some(original));
            t.set(field::ORIGINAL_YEAR, Some(&providerkit::prefix(original, 4)));
        }
        if context.album_only {
            t.set(field::DATE, c.first_release_date.as_deref().or(c.year.as_deref()));
            t.set(
                field::MB_RELEASE_GROUP_ID,
                if c.source == Source::Musicbrainz { c.release_group_id.as_deref() } else { None },
            );
        } else {
            Self::edition_tags(&mut t, c, context);
        }
        // The release group's type, shared by every edition.
        if let Some(primary) = &c.primary_type {
            t.add(field::RELEASE_TYPE, Some(&primary.to_lowercase()));
        }
        for secondary in &c.secondary_types {
            t.add(field::RELEASE_TYPE, Some(&secondary.to_lowercase()));
        }
        let disc_count = media.len().max(1).to_string();
        t.set(field::DISC_TOTAL, Some(&disc_count));
        t.set(field::TOTAL_DISCS, Some(&disc_count));
        if c.artist_ids().iter().any(|id| id == Self::VARIOUS_ARTISTS_ID)
            || c.artist.to_lowercase() == "various artists"
        {
            t.set(field::COMPILATION, Some("1"));
        }
        if context.write_genres {
            let source = if c.genres.is_empty() { context.discogs.as_ref() } else { Some(c) };
            for g in source.map(|s| s.genres.as_slice()).unwrap_or_default() {
                t.add(field::GENRE, Some(g));
            }
            for s in source.map(|s| s.styles.as_slice()).unwrap_or_default() {
                t.add(field::STYLE, Some(s));
            }
        }
        t
    }

    /// What belongs to one pressing: its date, label, codes and IDs.
    fn edition_tags(t: &mut TagSet, c: &Candidate, context: &TagContext) {
        t.set(field::DATE, c.released_date.as_deref().or(c.year.as_deref()));
        for label in split(c.label.as_deref()) {
            t.add(field::LABEL, Some(&label));
        }
        for cat in c.catalog_numbers() {
            t.add(field::CATALOG_NUMBER, Some(&cat));
        }
        t.set(field::BARCODE, c.barcode.as_deref());
        t.set(field::RELEASE_COUNTRY, c.country.as_deref());
        t.set(field::RELEASE_STATUS, c.status.as_ref().map(|s| s.to_lowercase()).as_deref());
        t.set(field::SCRIPT, c.script.as_deref());
        match c.source {
            Source::Musicbrainz => {
                t.set(field::MB_ALBUM_ID, Some(&c.provider_id));
                t.set(field::MB_RELEASE_GROUP_ID, c.release_group_id.as_deref());
            }
            Source::Discogs => t.set(field::DISCOGS_RELEASE_ID, Some(&c.provider_id)),
            Source::Acoustid => {}
        }
        t.set(field::MB_DISC_ID, context.musicbrainz_disc_id.as_deref());
        t.set(field::DISC_ID, context.freedb_disc_id.as_deref());
    }

    fn media_or_tracks(c: &Candidate, media: &[CandidateMedium]) -> Vec<CandidateMedium> {
        if media.is_empty() { vec![CandidateMedium::with_tracks(1, c.tracks.clone())] } else { media.to_vec() }
    }

    /// Tags for the local track at `index` (0-based across the album).
    pub fn track_tags(c: &Candidate, media: &[CandidateMedium], index: usize, context: &TagContext) -> Option<TagSet> {
        let list = Self::media_or_tracks(c, media);
        let mut cursor = 0;
        for (disc_index, medium) in list.iter().enumerate() {
            if index < cursor + medium.tracks.len() {
                let mut t = Self::album_tags(c, media, context);
                Self::fill(&mut t, &medium.tracks[index - cursor], medium, disc_index + 1, index, c, context);
                return Some(t);
            }
            cursor += medium.tracks.len();
        }
        None
    }

    /// Tags for track `track` (0-based) of the medium at 1-based `disc`:
    /// what a set member's files map to.
    pub fn track_tags_by_disc(
        c: &Candidate,
        media: &[CandidateMedium],
        disc: usize,
        track: usize,
        context: &TagContext,
    ) -> Option<TagSet> {
        let list = Self::media_or_tracks(c, media);
        if disc < 1 || disc > list.len() || track >= list[disc - 1].tracks.len() {
            return None;
        }
        let index = list[..disc - 1].iter().map(|m| m.tracks.len()).sum::<usize>() + track;
        let mut t = Self::album_tags(c, media, context);
        Self::fill(&mut t, &list[disc - 1].tracks[track], &list[disc - 1], disc, index, c, context);
        Some(t)
    }

    fn fill(
        t: &mut TagSet,
        track: &CandidateTrack,
        medium: &CandidateMedium,
        disc_number: usize,
        index: usize,
        c: &Candidate,
        context: &TagContext,
    ) {
        t.set(field::TITLE, Some(&track.title));
        let credits = if track.artist_credits.is_empty() { &c.artist_credits } else { &track.artist_credits };
        t.set(field::ARTIST, Some(track.artist.as_deref().unwrap_or(&c.artist)));
        let sort = if credits.is_empty() { c.artist_sort() } else { Some(ArtistCredit::sort_joined(credits)) };
        t.set(field::ARTIST_SORT, sort.as_deref());
        for credit in credits {
            t.add(field::ARTISTS, Some(&credit.name));
        }
        for id in credits.iter().filter_map(|c| c.artist_id.as_deref()) {
            t.add(field::MB_ARTIST_ID, Some(id));
        }
        let count = medium.tracks.len().to_string();
        t.set(field::TRACK_NUMBER, Some(&track.position.to_string()));
        t.set(field::TRACK_TOTAL, Some(&count));
        t.set(field::TOTAL_TRACKS, Some(&count));
        t.set(field::DISC_NUMBER, Some(&disc_number.to_string()));
        t.set(field::DISC_SUBTITLE, medium.title.as_deref());
        t.set(field::MEDIA, context.media_override.as_deref().or(medium.format.as_deref()));
        // ISRCs differ between editions (remasters get their own).
        if !context.album_only {
            for isrc in &track.isrcs {
                t.add(field::ISRC, Some(isrc));
            }
        }
        if c.source == Source::Musicbrainz {
            t.set(field::MB_TRACK_ID, track.recording_id.as_deref());
            if !context.album_only {
                t.set(field::MB_RELEASE_TRACK_ID, track.track_id.as_deref());
            }
        }
        for work in &track.works {
            t.add(field::WORK, Some(&work.title));
            t.add(field::MB_WORK_ID, work.id.as_deref());
        }
        for (f, value) in Self::credit_fields(&track.credits) {
            t.add(f, Some(&value));
        }
        if context.include_discogs_credits
            && let Some(discogs) = &context.discogs
            && discogs.id() != c.id()
        {
            let dtracks = discogs.all_tracks();
            let own = if c.all_tracks_is_empty() { dtracks.len() } else { c.all_tracks().len() };
            if index < dtracks.len() && dtracks.len() == own {
                for (f, value) in Self::credit_fields(&dtracks[index].credits) {
                    t.add(f, Some(&value));
                }
            }
        }
        t.set(field::ACOUSTID, context.acoust_ids.get(&index).map(|s| s.as_str()));
        if context.write_fingerprints {
            t.set(field::ACOUSTID_FINGERPRINT, context.fingerprints.get(&index).map(|s| s.as_str()));
        }
    }

    /// Role/name credits (MusicBrainz relationships or Discogs extra
    /// artists) → Picard fields. Instruments and vocals become PERFORMER
    /// "Name (role)".
    pub fn credit_fields(credits: &[TrackCredit]) -> Vec<(&'static str, String)> {
        let mut out = vec![];
        for credit in credits {
            for role in credit.role.split(',').map(providerkit::trim_whitespaces).filter(|r| !r.is_empty()) {
                let key = Self::normalized_role(role);
                let f = match key.as_str() {
                    "composer" | "composed by" | "music by" => field::COMPOSER,
                    "lyricist" | "lyrics by" | "words by" | "librettist" => field::LYRICIST,
                    "writer" | "written-by" | "written by" | "songwriter" => field::WRITER,
                    "arranger" | "arranged by" | "orchestrator" | "orchestrated by" => field::ARRANGER,
                    "conductor" | "conducted by" => field::CONDUCTOR,
                    "producer" | "produced by" | "co-producer" | "executive producer" | "executive-producer" => {
                        field::PRODUCER
                    }
                    "engineer" | "recorded by" | "recording engineer" | "recording" | "mastered by" | "mastering"
                    | "mastering engineer" => field::ENGINEER,
                    "mixer" | "mixed by" | "mix" => field::MIXER,
                    "remixer" | "remix" | "remixed by" => field::REMIXER,
                    "performer" => field::PERFORMER,
                    _ => {
                        out.push((field::PERFORMER, format!("{} ({key})", credit.name)));
                        continue;
                    }
                };
                out.push((f, credit.name.clone()));
            }
        }
        out
    }

    /// "Guitar [Acoustic]" → "acoustic guitar"; "Vocals" → "vocals".
    pub fn normalized_role(raw: &str) -> String {
        let role = providerkit::trim_whitespaces(&raw.to_lowercase()).to_string();
        if let (Some(open), Some(close)) = (role.find('['), role.find(']'))
            && open < close
        {
            let qualifier = providerkit::trim_whitespaces(&role[open + 1..close]);
            let base = providerkit::trim_whitespaces(&role[..open]);
            return if qualifier.is_empty() { base.to_string() } else { format!("{qualifier} {base}") };
        }
        role
    }
}

fn split(joined: Option<&str>) -> Vec<String> {
    joined
        .unwrap_or("")
        .split(',')
        .map(|s| providerkit::trim_whitespaces(s).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TagMerge;
    use providerkit::WorkCredit;

    fn track(position: i64, title: &str, ms: i64, rec: &str, trk: &str) -> CandidateTrack {
        let mut t = CandidateTrack::new(position, title);
        t.duration_ms = Some(ms);
        t.recording_id = Some(rec.into());
        t.track_id = Some(trk.into());
        t
    }

    fn candidate() -> Candidate {
        let mut first = track(1, "So What", 562000, "rec-1", "trk-1");
        first.credits = vec![
            TrackCredit::new("trumpet", "Miles Davis"),
            TrackCredit::new("producer", "Teo Macero"),
            TrackCredit::new("composer", "Miles Davis"),
        ];
        first.isrcs = vec!["USSM15900001".into()];
        first.works = vec![WorkCredit { id: Some("work-1".into()), title: "So What".into() }];
        let mut c = Candidate::new(Source::Musicbrainz, "rel-1", "Kind of Blue", "Miles Davis");
        c.release_group_id = Some("rg-1".into());
        c.year = Some("1997".into());
        c.country = Some("US".into());
        c.label = Some("Columbia, Legacy".into());
        c.catalog_number = Some("CK 64935, 64935".into());
        c.media_format = Some("CD".into());
        c.track_count = Some(2);
        c.released_date = Some("1997-03-25".into());
        c.barcode = Some("074646493526".into());
        c.media = vec![CandidateMedium::new(
            1,
            Some("CD".into()),
            None,
            vec![first, track(2, "Freddie Freeloader", 583000, "rec-2", "trk-2")],
            vec![],
            None,
        )];
        c.status = Some("Official".into());
        c.primary_type = Some("Album".into());
        c.artist_credits = vec![ArtistCredit {
            name: "Miles Davis".into(),
            join_phrase: String::new(),
            artist_id: Some("art-1".into()),
            sort_name: Some("Davis, Miles".into()),
        }];
        c.first_release_date = Some("1959-08-17".into());
        c.script = Some("Latn".into());
        c
    }

    #[test]
    fn picard_fields_for_a_track() {
        let c = candidate();
        let mut d = Candidate::new(Source::Discogs, "d1", "Kind of Blue", "Miles Davis");
        let mut d1 = CandidateTrack::new(1, "So What");
        d1.credits = vec![TrackCredit::new("Bass", "Paul Chambers"), TrackCredit::new("Written-By", "Miles Davis")];
        d.tracks = vec![d1, CandidateTrack::new(2, "Freddie Freeloader")];
        d.genres = vec!["Jazz".into()];
        d.styles = vec!["Modal".into()];
        let ctx = TagContext {
            musicbrainz_disc_id: Some("disc-1".into()),
            acoust_ids: BTreeMap::from([(0, "acoust-1".into())]),
            discogs: Some(d),
            ..Default::default()
        };
        let t = PicardMapper::track_tags(&c, &c.media, 0, &ctx).unwrap();
        let first = |f| t.first(f);
        assert_eq!(first(field::TITLE), Some("So What"));
        assert_eq!(first(field::ARTIST), Some("Miles Davis"));
        assert_eq!(first(field::ARTIST_SORT), Some("Davis, Miles"));
        assert_eq!(first(field::ALBUM_ARTIST_SORT), Some("Davis, Miles"));
        assert_eq!(first(field::DATE), Some("1997-03-25"));
        assert_eq!(first(field::ORIGINAL_DATE), Some("1959-08-17"));
        assert_eq!(first(field::ORIGINAL_YEAR), Some("1959"));
        assert_eq!(t.get(field::LABEL), ["Columbia", "Legacy"]);
        assert_eq!(t.get(field::CATALOG_NUMBER), ["CK 64935", "64935"]);
        assert_eq!(
            (
                first(field::TRACK_NUMBER),
                first(field::TRACK_TOTAL),
                first(field::DISC_NUMBER),
                first(field::DISC_TOTAL)
            ),
            (Some("1"), Some("2"), Some("1"), Some("1"))
        );
        assert_eq!(
            (first(field::MB_ALBUM_ID), first(field::MB_TRACK_ID), first(field::MB_RELEASE_TRACK_ID)),
            (Some("rel-1"), Some("rec-1"), Some("trk-1"))
        );
        assert_eq!(t.get(field::MB_ARTIST_ID), ["art-1"]);
        assert_eq!(t.get(field::MB_ALBUM_ARTIST_ID), ["art-1"]);
        assert_eq!(first(field::MB_DISC_ID), Some("disc-1"));
        assert_eq!(first(field::RELEASE_STATUS), Some("official"));
        assert_eq!(t.get(field::RELEASE_TYPE), ["album"]);
        assert_eq!(t.get(field::ISRC), ["USSM15900001"]);
        assert_eq!(t.get(field::WORK), ["So What"]);
        assert_eq!(t.get(field::MB_WORK_ID), ["work-1"]);
        assert!(t.get(field::PERFORMER).contains(&"Miles Davis (trumpet)".to_string()));
        assert!(t.get(field::PERFORMER).contains(&"Paul Chambers (bass)".to_string()), "Discogs credits merged");
        assert_eq!(t.get(field::PRODUCER), ["Teo Macero"]);
        assert_eq!(t.get(field::COMPOSER), ["Miles Davis"]);
        assert_eq!(t.get(field::WRITER), ["Miles Davis"]);
        assert_eq!(t.get(field::GENRE), ["Jazz"]);
        assert_eq!(t.get(field::STYLE), ["Modal"]);
        assert_eq!(first(field::ACOUSTID), Some("acoust-1"));
        assert!(t.get(field::ACOUSTID_FINGERPRINT).is_empty());
        let second = PicardMapper::track_tags(&c, &c.media, 1, &ctx).unwrap();
        assert_eq!(second.first(field::TRACK_NUMBER), Some("2"));
        assert!(second.get(field::PERFORMER).is_empty());
        assert!(PicardMapper::track_tags(&c, &c.media, 2, &ctx).is_none());
        assert_eq!(PicardMapper::normalized_role("Guitar [Acoustic]"), "acoustic guitar");
    }

    #[test]
    fn album_only_leaves_the_edition_out() {
        let c = candidate();
        let ctx = TagContext {
            album_only: true,
            media_override: Some("SACD".into()),
            musicbrainz_disc_id: Some("disc-1".into()),
            ..Default::default()
        };
        let t = PicardMapper::track_tags(&c, &c.media, 0, &ctx).unwrap();
        assert_eq!((t.first(field::ALBUM), t.first(field::TITLE)), (Some("Kind of Blue"), Some("So What")));
        assert_eq!((t.first(field::DATE), t.first(field::ORIGINAL_YEAR)), (Some("1959-08-17"), Some("1959")));
        assert_eq!((t.first(field::MB_RELEASE_GROUP_ID), t.first(field::MB_TRACK_ID)), (Some("rg-1"), Some("rec-1")));
        assert_eq!(t.first(field::MEDIA), Some("SACD"));
        for f in [
            field::MB_ALBUM_ID,
            field::MB_RELEASE_TRACK_ID,
            field::LABEL,
            field::CATALOG_NUMBER,
            field::BARCODE,
            field::RELEASE_COUNTRY,
            field::RELEASE_STATUS,
            field::MB_DISC_ID,
            field::SCRIPT,
            field::ISRC,
        ] {
            assert!(t.get(f).is_empty(), "{f} belongs to one edition");
        }
        let mut mine = TagSet::new();
        mine.set(field::ISRC, Some("USWB19903566"));
        mine.set(field::MB_ALBUM_ID, Some("another-release"));
        let kept = field::EDITION_FIELDS.iter().map(|f| f.to_string()).collect();
        let merged = TagMerge::merge(&mine, &t, &Default::default(), &kept).0;
        assert_eq!(merged.first(field::ISRC), Some("USWB19903566"), "the files' own ISRC stays");
        assert!(merged.get(field::MB_ALBUM_ID).is_empty());
    }

    #[test]
    fn tags_by_disc_position() {
        let media = vec![
            CandidateMedium::new(
                1,
                Some("CD".into()),
                Some("Atto primo".into()),
                vec![CandidateTrack::new(1, "Ouverture"), CandidateTrack::new(2, "Notte e giorno")],
                vec![],
                None,
            ),
            CandidateMedium::new(
                2,
                Some("CD".into()),
                Some("Atto secondo".into()),
                vec![CandidateTrack::new(1, "Eh via buffone")],
                vec![],
                None,
            ),
        ];
        let mut opera = Candidate::new(Source::Musicbrainz, "dg", "Don Giovanni", "Mozart");
        opera.track_count = Some(3);
        opera.media = media.clone();
        let ctx = TagContext::default();
        let t = PicardMapper::track_tags_by_disc(&opera, &media, 2, 0, &ctx).unwrap();
        assert_eq!(
            (t.first(field::TITLE), t.first(field::DISC_NUMBER), t.first(field::DISC_TOTAL)),
            (Some("Eh via buffone"), Some("2"), Some("2"))
        );
        assert_eq!(
            (t.first(field::DISC_SUBTITLE), t.first(field::TRACK_NUMBER), t.first(field::TRACK_TOTAL)),
            (Some("Atto secondo"), Some("1"), Some("1"))
        );
        assert!(PicardMapper::track_tags_by_disc(&opera, &media, 3, 0, &ctx).is_none());
        assert!(PicardMapper::track_tags_by_disc(&opera, &media, 1, 2, &ctx).is_none());
    }
}
