//! Port of SACDKitTests' synthetic suite: a Scarletbook image with a real
//! audio area — frames packed into 2048-byte sectors the way discs do it
//! (2016 audio bytes per sector after a supplementary packet, frames
//! starting mid-sector, time codes in the frame table) — plus SACDTRL1/2,
//! SACD_IGL and SACDTTxt.

use flackit::{DSFFile, FileSource, ID3v2Bridge};
use sacdkit::scarletbook::{BYTES_PER_CHANNEL_PER_FRAME, SECTOR_SIZE};
use sacdkit::{
    DSFWriter, PausePolicy, SACDExtractOptions, SACDExtractor, SACDFrameReader, SACDTime, bit_reverse, read,
};
use std::path::{Path, PathBuf};

struct Track {
    start_frame: i64,
    duration_frames: i64,
    title: &'static str,
    performer: Option<&'static str>,
    isrc: Option<&'static str>,
}

struct FakeDisc {
    channels: usize,
    tracks: Vec<Track>,
    total_frames: i64,
    dst: bool,
}

fn frame_data(index: i64, bytes: usize) -> Vec<u8> {
    let mut x = (index as u32).wrapping_mul(2654435761).wrapping_add(12345);
    (0..bytes)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect()
}

fn be16(v: usize) -> [u8; 2] {
    (v as u16).to_be_bytes()
}
fn be32(v: usize) -> [u8; 4] {
    (v as u32).to_be_bytes()
}
fn put(d: &mut [u8], bytes: &[u8], at: usize) {
    d[at..at + bytes.len()].copy_from_slice(bytes);
}

impl FakeDisc {
    fn frame_bytes(&self) -> usize {
        self.channels * BYTES_PER_CHANNEL_PER_FRAME
    }

    fn write(&self, path: &Path) {
        let s = SECTOR_SIZE;
        // Audio area: pack frames into sectors.
        let mut pending = vec![];
        let mut starts: Vec<(usize, SACDTime)> = vec![];
        for f in 0..self.total_frames {
            starts.push((pending.len(), SACDTime::new(f)));
            if self.dst {
                pending.push(0x00); // "uncompressed" DST frame header
            }
            pending.extend(frame_data(f, self.frame_bytes()));
        }
        let mut audio = vec![];
        let (mut consumed, mut next_start) = (0, 0);
        let per_sector = 2016;
        while consumed < pending.len() {
            let take = per_sector.min(pending.len() - consumed);
            let chunk = &pending[consumed..consumed + take];
            let mut in_chunk = vec![];
            while next_start < starts.len() && starts[next_start].0 < consumed + take {
                in_chunk.push((starts[next_start].0 - consumed, starts[next_start].1));
                next_start += 1;
            }
            let mut audio_packets: Vec<(bool, &[u8])> = vec![];
            let (mut cursor, mut cursor_is_start) = (0, false);
            for (s0, _) in &in_chunk {
                if *s0 > cursor {
                    audio_packets.push((cursor_is_start, &chunk[cursor..*s0]));
                }
                cursor = *s0;
                cursor_is_start = true;
            }
            audio_packets.push((cursor_is_start, &chunk[cursor..]));
            let header_size = 1 + 2 * (audio_packets.len() + 1) + if self.dst { 4 } else { 3 } * in_chunk.len();
            let supplementary = s - header_size - take;
            let zeros = vec![0u8; supplementary];
            let mut packets: Vec<(bool, usize, &[u8])> = vec![(false, 3, &zeros)];
            packets.extend(audio_packets.iter().map(|(st, p)| (*st, 2, *p)));
            let mut sector = vec![(packets.len() << 5 | in_chunk.len() << 2 | self.dst as usize) as u8];
            for (st, kind, payload) in &packets {
                sector.extend(be16(if *st { 0x8000 } else { 0 } | kind << 11 | payload.len()));
            }
            for (_, tc) in &in_chunk {
                sector.extend([tc.minutes() as u8, tc.seconds() as u8, tc.frames() as u8]);
                if self.dst {
                    sector.push(8);
                }
            }
            for (_, _, payload) in &packets {
                sector.extend_from_slice(payload);
            }
            assert_eq!(sector.len(), s);
            audio.extend(sector);
            consumed += take;
        }
        let audio_start = 560;
        let audio_sectors = audio.len() / s;
        let mut image = vec![0u8; audio_start * s];

        // Master TOC
        let m = 510 * s;
        put(&mut image, b"SACDMTOC", m);
        put(&mut image, &[1, 0x14], m + 8);
        put(&mut image, &be16(1), m + 16);
        put(&mut image, &be16(1), m + 18);
        put(&mut image, &be32(544), m + 64);
        put(&mut image, b"FAKE-001", m + 88);
        put(&mut image, &be16(1999), m + 120);
        put(&mut image, &[3, 14], m + 122);
        put(&mut image, b"en", m + 136);
        put(&mut image, &[1], m + 138);
        put(&mut image, &[1, 0, 0, 23], m + 104); // disc genre: "Rock Music"
        // Master text
        let t = 511 * s;
        put(&mut image, b"SACDText", t);
        put(&mut image, &be16(100), t + 16);
        put(&mut image, &be16(140), t + 18);
        put(&mut image, b"Fake Album\0", t + 100);
        put(&mut image, b"Fake Artist\0", t + 140);
        // Area TOC
        let a = 544 * s;
        put(&mut image, if self.channels > 2 { b"MULCHTOC" } else { b"TWOCHTOC" }, a);
        put(&mut image, &[1, 0x14], a + 8);
        put(&mut image, &be16(8), a + 10);
        put(&mut image, &[4], a + 20);
        put(&mut image, &[if self.dst { 0 } else { 2 }], a + 21);
        put(&mut image, &[self.channels as u8], a + 32);
        let play = SACDTime::new(self.total_frames);
        put(&mut image, &[play.minutes() as u8, play.seconds() as u8, play.frames() as u8], a + 64);
        put(&mut image, &[0, self.tracks.len() as u8], a + 68);
        put(&mut image, &be32(audio_start), a + 72);
        put(&mut image, &be32(audio_start + audio_sectors - 1), a + 76);
        put(&mut image, b"en", a + 88);
        put(&mut image, &[1], a + 90);
        // TRL1 / TRL2
        let (trl1, trl2) = (545 * s, 546 * s);
        put(&mut image, b"SACDTRL1", trl1);
        put(&mut image, b"SACDTRL2", trl2);
        let fb = self.frame_bytes();
        for (k, tr) in self.tracks.iter().enumerate() {
            put(&mut image, &be32(audio_start + tr.start_frame as usize * fb / per_sector), trl1 + 8 + 4 * k);
            put(&mut image, &be32(tr.duration_frames as usize * fb / per_sector), trl1 + 8 + 1020 + 4 * k);
            let (st, du) = (SACDTime::new(tr.start_frame), SACDTime::new(tr.duration_frames));
            put(&mut image, &[st.minutes() as u8, st.seconds() as u8, st.frames() as u8, 0], trl2 + 8 + 4 * k);
            put(&mut image, &[du.minutes() as u8, du.seconds() as u8, du.frames() as u8, 0], trl2 + 8 + 1020 + 4 * k);
        }
        // IGL (2 sectors): ISRCs then genres
        let igl = 547 * s;
        put(&mut image, b"SACD_IGL", igl);
        for (k, tr) in self.tracks.iter().enumerate() {
            if let Some(isrc) = tr.isrc {
                put(&mut image, isrc.as_bytes(), igl + 8 + 12 * k);
            }
            put(&mut image, &[1, 0, 0, 14], igl + 8 + 3060 + 4 * k); // Jazz
        }
        // Track text (2 sectors)
        let txt = 549 * s;
        put(&mut image, b"SACDTTxt", txt);
        let mut pos = 600;
        for (k, tr) in self.tracks.iter().enumerate() {
            put(&mut image, &be16(pos), txt + 8 + 2 * k);
            let mut items = vec![(1u8, tr.title)];
            if let Some(p) = tr.performer {
                items.push((2, p));
            }
            put(&mut image, &[items.len() as u8, 0, 0, 0], txt + pos);
            let mut p = pos + 4;
            for (kind, text) in items {
                let mut item = vec![kind, 0x20];
                item.extend(text.as_bytes());
                item.push(0);
                put(&mut image, &item, txt + p);
                p += 2 + text.len() + 1;
                p = (p + 3) & !3;
            }
            pos = p;
        }
        image.extend(audio);
        std::fs::write(path, image).unwrap();
    }
}

fn make_disc(dst: bool) -> FakeDisc {
    FakeDisc {
        channels: 2,
        tracks: vec![
            Track {
                start_frame: 150,
                duration_frames: 200,
                title: "First Song",
                performer: Some("Someone"),
                isrc: Some("USABC0100001"),
            },
            Track { start_frame: 375, duration_frames: 150, title: "Second / Song", performer: None, isrc: None },
            Track {
                start_frame: 525,
                duration_frames: 75,
                title: "Third",
                performer: Some("Guest"),
                isrc: Some("USABC0100003"),
            },
        ],
        total_frames: 600,
        dst,
    }
}

fn fake_iso(dst: bool) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let iso = dir.path().join("fake.iso");
    make_disc(dst).write(&iso);
    (dir, iso)
}

fn vorbis(path: &Path) -> flackit::VorbisComment {
    let dsf = DSFFile::parse(&FileSource::open(path).unwrap()).unwrap();
    ID3v2Bridge::to_vorbis(dsf.id3_tag.as_ref().unwrap(), "x")
}

#[test]
fn reads_area_tracks_texts_and_codes() {
    let (_d, iso) = fake_iso(false);
    let disc = read(&iso).unwrap();
    assert_eq!(disc.title().as_deref(), Some("Fake Album"));
    assert_eq!(disc.artist().as_deref(), Some("Fake Artist"));
    assert_eq!(disc.year().as_deref(), Some("1999"));
    assert_eq!(disc.areas.len(), 1);
    let area = disc.stereo_area().unwrap();
    assert_eq!(area.channel_count, 2);
    assert!(!area.is_dst());
    assert_eq!(area.tracks.len(), 3);
    assert_eq!(area.play_time.total_frames, 600);
    let (t1, t2, t3) = (&area.tracks[0], &area.tracks[1], &area.tracks[2]);
    assert_eq!(t1.title.as_deref(), Some("First Song"));
    assert_eq!(t1.performer.as_deref(), Some("Someone"));
    assert_eq!(t1.isrc.as_deref(), Some("USABC0100001"));
    assert_eq!(t1.genre.as_deref(), Some("Jazz"));
    assert_eq!((t1.start_time.total_frames, t1.duration.total_frames), (150, 200));
    assert_eq!(t2.title.as_deref(), Some("Second / Song"));
    assert_eq!((t2.performer.as_deref(), t2.isrc.as_deref()), (None, None));
    assert_eq!((t3.title.as_deref(), t3.performer.as_deref()), (Some("Third"), Some("Guest")));
    assert_eq!(area.audio_start_sector, 560);
}

#[test]
fn frame_reader_reassembles_every_frame() {
    let (_d, iso) = fake_iso(false);
    let fake = make_disc(false);
    let disc = read(&iso).unwrap();
    let area = disc.stereo_area().unwrap();
    let mut reader = SACDFrameReader::new(&iso, area.audio_start_sector, area.audio_end_sector).unwrap();
    let mut count = 0;
    while let Some(frame) = reader.next_frame().unwrap() {
        assert_eq!(frame.timecode.total_frames, count);
        assert_eq!(frame.data, frame_data(count, fake.frame_bytes()));
        count += 1;
    }
    assert_eq!(count, 600);
}

#[test]
fn extracts_tracks_with_pause_appended_and_tags() {
    let (_d, iso) = fake_iso(false);
    let out = tempfile::tempdir().unwrap();
    let fake = make_disc(false);
    let disc = read(&iso).unwrap();
    let area = disc.stereo_area().unwrap();
    let outcome =
        SACDExtractor::extract(&disc, area, out.path(), &SACDExtractOptions::default(), None, &mut |_| {}).unwrap();
    assert!(outcome.verified(), "{:?}", outcome.warnings);
    assert!(outcome.output_folder.ends_with("Fake Artist/Fake Album (1999)"));
    assert_eq!((outcome.dropped_lead_in_frames, outcome.dropped_pause_frames), (150, 0));
    assert_eq!(outcome.tracks.iter().map(|t| t.frames).collect::<Vec<_>>(), [225, 150, 75]);
    let names: Vec<String> =
        outcome.tracks.iter().map(|t| t.url.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["01 First Song.dsf", "02 Second - Song.dsf", "03 Third.dsf"]);
    assert_eq!(outcome.tracks[0].sample_count, 225 * 37632);

    // DSF audio equals de-interleaved, bit-reversed frames.
    let dsf = DSFFile::parse(&FileSource::open(&outcome.tracks[1].url).unwrap()).unwrap();
    assert_eq!((dsf.channel_count, dsf.sample_rate, dsf.sample_count), (2, 2_822_400, 150 * 37632));
    let bytes = std::fs::read(&outcome.tracks[1].url).unwrap();
    let frame375 = frame_data(375, fake.frame_bytes());
    let expected_l: Vec<u8> = (0..4096).map(|i| bit_reverse(frame375[2 * i])).collect();
    let expected_r: Vec<u8> = (0..4096).map(|i| bit_reverse(frame375[2 * i + 1])).collect();
    assert_eq!(&bytes[92..92 + 4096], &expected_l[..]);
    assert_eq!(&bytes[92 + 4096..92 + 8192], &expected_r[..]);

    // Tags through the ID3 bridge.
    let v = vorbis(&outcome.tracks[1].url);
    assert_eq!(v.get("TITLE"), ["Second / Song"]);
    assert_eq!(v.get("ARTIST"), ["Fake Artist"]); // no performer: the disc artist
    assert_eq!(v.get("ALBUM"), ["Fake Album"]);
    assert_eq!(v.get("TRACKNUMBER"), ["2"]);
    assert_eq!(v.get("TRACKTOTAL"), ["3"]);
    assert_eq!(v.get("DATE"), ["1999"]);
    assert_eq!(v.get("GENRE"), ["Jazz"]);
    assert_eq!(v.get("CATALOGNUMBER"), ["FAKE-001"]);
    let v1 = vorbis(&outcome.tracks[0].url);
    assert_eq!(v1.get("ARTIST"), ["Someone"]);
    assert_eq!(v1.get("ISRC"), ["USABC0100001"]);

    // Drop policy matches TOC durations.
    let drop = SACDExtractOptions { pause_policy: PausePolicy::Drop, overwrite_existing: true, ..Default::default() };
    let dropped = SACDExtractor::extract(&disc, area, out.path(), &drop, None, &mut |_| {}).unwrap();
    assert!(dropped.verified());
    assert_eq!(dropped.tracks.iter().map(|t| t.frames).collect::<Vec<_>>(), [200, 150, 75]);
    assert_eq!(dropped.dropped_pause_frames, 25);

    // Refuses to overwrite by default.
    assert!(
        SACDExtractor::extract(&disc, area, out.path(), &SACDExtractOptions::default(), None, &mut |_| {}).is_err()
    );
}

#[test]
fn dst_area_with_uncompressed_frames_extracts() {
    let (_d, iso) = fake_iso(true);
    let out = tempfile::tempdir().unwrap();
    let fake = make_disc(true);
    let disc = read(&iso).unwrap();
    let area = disc.stereo_area().unwrap();
    assert!(area.is_dst());
    let mut reader = SACDFrameReader::new(&iso, area.audio_start_sector, area.audio_end_sector).unwrap();
    let mut n = 0;
    while let Some(f) = reader.next_frame().unwrap() {
        assert_eq!(f.timecode.total_frames, n);
        assert_eq!(f.data.len(), fake.frame_bytes() + 1);
        n += 1;
    }
    assert_eq!(n, 600);
    let outcome =
        SACDExtractor::extract(&disc, area, out.path(), &SACDExtractOptions::default(), None, &mut |_| {}).unwrap();
    assert!(outcome.verified(), "{:?}", outcome.warnings);
    assert_eq!(outcome.tracks.iter().map(|t| t.frames).collect::<Vec<_>>(), [225, 150, 75]);
    let bytes = std::fs::read(&outcome.tracks[1].url).unwrap();
    let frame375 = frame_data(375, fake.frame_bytes());
    assert_eq!(bytes[92], bit_reverse(frame375[0]));
    assert_eq!(bytes[92 + 4096], bit_reverse(frame375[1]));
}

#[test]
fn dsf_writer_pads_and_reports_sizes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.dsf");
    let mut w = DSFWriter::create(&path, 2, 2_822_400).unwrap();
    w.write_frame(&[0x01, 0x80, 0xFF, 0x00]).unwrap();
    let (samples, size) = w.finish(Some(&[0x49, 0x44, 0x33, 3, 0, 0, 0, 0, 0, 0])).unwrap();
    assert_eq!(samples, 16);
    assert_eq!(size, 92 + 8192 + 10);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!((bytes[92], bytes[93]), (0x80, 0xFF));
    assert_eq!((bytes[92 + 4096], bytes[92 + 4097]), (0x01, 0x00));
    let dsf = DSFFile::parse(&FileSource::open(&path).unwrap()).unwrap();
    assert_eq!((dsf.sample_count, dsf.block_size_per_channel), (16, 4096));
}
