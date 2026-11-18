# drtagger for Linux — Design

App name **drtagger**, app ID `us.priet.drtagger`, repo `drtagger-linux`
(github.com/prietus/drtagger-linux, MIT; LGPL parts as on Mac).
Decisions agreed with Carlos on 2026-11-17. This file is the source of truth
for the Linux port; change it when a decision changes.

## 1. Goal

A native GNOME app (Rust, GTK4 + libadwaita) with **feature parity** with
drtagger for Mac. The Mac app stays the reference and keeps evolving; Linux
follows it. The specification is `~/mactagger/DESIGN.md` plus the Mac code
itself: when the two disagree, the code wins (e.g. a tag write goes to a
temporary file that replaces the track with rename(2) after the audio hash
is checked — Mac commit fddf6f1, which superseded 3e46e10's in-place copy).

Parity means the same inputs, identification signals, scoring, tag output,
file layout, verification and workflow — not the same pixels. The UI follows
the GNOME HIG.

No hurry: every phase is finished, verified against the Mac app and committed
on Carlos's request before the next one starts.

## 2. Decisions

| Topic | Decision |
|---|---|
| Language / UI | Rust stable, gtk4-rs + libadwaita-rs, GObject subclasses, UI in Blueprint (`.blp`) |
| Relationship to Mac | Mac is the reference; core crates keep a clean API so they could later be exposed to Swift (uniffi) — not a goal now |
| Data | Independent from Mac (own index and settings). Files written (tags, `cover.jpg`, folder layout) are identical, so one NAS library serves both |
| Distribution | Flatpak (GNOME runtime), x86_64 + aarch64; bundles LGPL ffmpeg, Chromaprint, ONNX Runtime + models |
| Tags | Port of FLACKit/TagKit (own readers/writers), byte-identical to the Mac output |
| Audio I/O | ffmpeg as a child process, as on Mac (decode, FLAC encode, ffprobe) |
| Fingerprints | libchromaprint via FFI (same fingerprints as Mac) |
| DST | Port of DSTKit to Rust (LGPL 2.1+ crate, like the Swift package) |
| OCR | PaddleOCR (detection + recognition incl. Japanese, vertical text) on ONNX Runtime; accepted only after a benchmark against Vision on real scans |
| Barcodes | zxing-cpp |
| Cover similarity (Vision feature print) | image embedding model on ONNX Runtime; thresholds re-tuned against Mac's decisions |
| Images | libvips (decode, resize, JPEG 90 %) |
| Store | SQLite via rusqlite (replaces SwiftData) |
| Network | reqwest + tokio, bridged to the GLib main loop; same rate limits, User-Agent and disk cache as Mac |
| Secrets | libsecret (Secret portal inside Flatpak) |
| Settings | GSettings |
| Localisation | gettext, English base + Spanish |
| Rip inbox | inotify on the local inbox; logs from EAC, XLD, **whipper and CUERipper** |
| Resident mode | Background portal (keeps running with the window closed, autostart), desktop notifications; GNOME has no tray, "N to review" lives in the notification and the window |
| CDDB server | Same responder on 127.0.0.1:8880, off by default |

## 3. Architecture

Cargo workspace, one crate per Mac package so ports and tests map 1:1:

| Crate | Mac source | Notes |
|---|---|---|
| `flackit` | Packages/FLACKit | FLAC/DSF/WAV/AIFF/DFF, Vorbis, ID3v2, APEv2, MP4 atoms |
| `librarykit` | Packages/LibraryKit | scanner, CUE + encodings, SACDProbe, DiscTOC, split plan; `statfs` → `/proc/self/mounts` |
| `splitkit` | Packages/SplitKit | ffmpeg driver, image splitter, CTDB CRCs, path templates |
| `dstkit` | Packages/DSTKit | LGPL |
| `sacdkit` | Packages/SACDKit | reader, frame reader, extractor, DSF writer |
| `loudnesskit` | Packages/LoudnessKit | vDSP → plain Rust (auto-vectorised) |
| `chromaprint-sys` / `chromaprintkit` | Packages/Chromaprint | FFI to libchromaprint (FFT via KissFFT/FFTW) |
| `providerkit` | Packages/ProviderKit | MB, AcoustID, Discogs, CAA, fanart, iTunes, Deezer, CTDB, key checks |
| `visionkit` | Vision calls in IdentifyKit | OCR, barcodes, feature print behind one trait |
| `identifykit` | Packages/IdentifyKit | signals, fingerprints, identifier, MatchScorer, CoverArtFetcher, CDDB responder |
| `tagkit` | Packages/TagKit | TagSet, PicardMapper, TagMerge, backups, artwork, organiser |
| `drtagger-core` | App services | store, queue/batch state machine, inbox watcher, library index |
| `drtagger-cli` | App launch arguments | `--add`, `--identify`, `--split-into`, `--tag`, `--restore`, batch; used by tests |
| `drtagger` | App (SwiftUI) | GTK4/libadwaita UI |

**Parity harness** (`parity/`). `MacDump` is a Swift executable that links
the Mac packages in place (`MACTAGGER_DIR`, default `~/mactagger`; nothing
in that repo changes) and prints JSON; `drtagger-cli parity` prints the
same shape from the Rust crates; `compare.py` diffs them with paths reduced
to `$ROOTn/…`, strings in NFC and collections matched by key. Each phase
extends both dumps (scan, CUE/TOC/plans and folder names so far; later
split CRCs/MD5s, DSF bytes, tag blocks, loudness, candidate rankings with
recorded network responses). `mirror.py` recreates a sample tree on the VM
without the audio: small files copied, ISO TOC sectors copied, everything
else sparse at its real size. Dumps of the private library stay in the
git-ignored `parity/local/`. The ported Swift unit tests run in CI; the
goldens are a local check.

Mac bugs found by the harness are fixed in the port, not copied, and
listed here for the Mac app:
- A CUE `FILE` name with an accent on an NFS share is reported missing:
  `URL.appending(path:)` decomposes the name (È → E + U+0300) and the
  NFC file is not found, so a complete multi-file CUE rip becomes a track
  folder with the CUE as a hint ("Storia di un minuto", "Leño Leño",
  "en directo - leño" on the NAS). **Fixed on Mac 2026-11-17**
  (`CueFile.url(relativeTo:)` keeps the sheet's bytes and falls back to a
  case/normalisation-insensitive lookup, like the port); the NAS parity
  run is now identical.
- `LibraryOrganizer.plan` names a third duplicate "x.2.3.flac" (it appends
  to the previous candidate); the port writes "x.3.flac". Not fixed on Mac.
- `FingerprintService.consensus` leaves releases with the same hit count
  and best score in dictionary order, which Swift randomises per process.
  Only the first 8 votes are fetched and expanded to their release groups,
  so the same album can end with a different candidate pool from one run
  to the next (Walkin': 35 releases tie at 5 tracks / 0.9995). The port
  breaks ties by release ID; the parity reference is built from a copy of
  the Mac packages with that one change. Not fixed on Mac.
- `TagService.apply` with "organize after apply" on a track folder sets
  `m.path` to the album's new folder and only then stores
  `tagBackups = backupsByOwner[m.path]` / `tagReports`, which are keyed by
  the old path: the record keeps no backups or reports, so Restore cannot
  put the original tags back (seen on Diamond Dogs, Ella and Oscar and
  Corre, corre in the 2026-11-18 run). The port looks them up by the old
  path. Not fixed on Mac.
- `AppSettings` reads booleans with `object(forKey:) as? Bool`, so a
  launch override such as `-library.organizeAfterApply NO` (a string in
  the argument domain) is ignored and the stored value or default wins;
  `bool(forKey:)` would accept it. Not fixed on Mac.
- Moving a track folder into the library (Apply with organize, or the
  batch's "already tagged; moved into the library") changes the record's
  path but keeps its scan details, which still name the old files. A
  later identification (the next batch: such albums have no `taggedAt`,
  so `start` makes them pending again) reads files that are gone and ends
  "No sure match" (A Love Supreme, Diamond Dogs in the 2026-11-18 run).
  The port scans the new folder after the move and stores its details.
  Not fixed on Mac.

## 4. Development setup

- Code lives on the Mac in `~/drtagger-linux`; `scripts/vm.sh` rsyncs it to
  the UTM VM (Arch Linux ARM, GNOME 51, GTK 4.24, libadwaita 1.10,
  `alarm@alarm.local`, SSH key, passwordless sudo) and runs cargo there.
  Carlos sees the UI in the UTM window.
- Samples: `parity/mirror.py` mirrors `~/mactagger-samples` and the NAS
  (`~/nfs` isos/rips/dsf, `~/nfs-music`) into the VM's `~/samples` as
  sparse trees (≈290 MB on disk). Phases that read audio copy the real
  files they need; the full sample set is 12 GB and the VM has ~25 GB free.
- CI: GitHub Actions in an Arch/Fedora container (recent GTK) for build +
  offline tests; Flatpak build job later.

## 5. Phases

0. **Scaffold.** **Done 2026-11-17.** Workspace with one crate per Mac
   package (empty), `drtagger-core` (XDG paths, tokio runtime, keyring via
   oo7 with the Mac's account names), `drtagger-cli` (`status`,
   `set-key`), `scripts/vm.sh` (sync / cargo / cli / run / sh), CI
   (Arch container: fmt, clippy `-D warnings`, tests, schema check),
   GSettings schema mirroring every Mac `AppSettings` key and default,
   window (`AdwNavigationSplitView`, size remembered), Preferences
   (General / Providers / Advanced, all settings bound, keys load and save
   through the keyring), About, shortcuts dialog, `.desktop` + metainfo
   (both validate). Verified on the VM: keyring store/delete from the CLI,
   window, Preferences and About rendered (actions driven over D-Bus
   `org.gtk.Actions`). Pending: the Flatpak manifest
   (`build-aux/us.priet.drtagger.json`) was unbuilt until phase 11;
   the icon is the Mac PNG until a GNOME-style SVG in phase 8; strings in
   Rust code are not yet gettext-wrapped (phase 8).
1. **Library scan.** **Done 2026-11-17.** `librarykit`: AudioFormat and
   file rules, CUE parser with Foundation-strict encoding detection
   (CP1252/CP1251 undefined bytes fail, Shift-JIS through Apple's CP932
   code points), DiscTOC (MusicBrainz / FreeDB / CTDB), CueSplitPlan,
   SACDProbe, FolderNameParser, scanner (Finder ordering via ICU numeric
   collation in the system locale, symlinks skipped like resource values
   do, CUE file names resolved ignoring case and Unicode normalisation as
   macOS file systems do), `/proc/self/mounts` volume lookup. The Swift
   unit tests are ported except the two that read `~/mactagger-samples`
   (covered by the parity run) and ProcessPaths (an NFD workaround Linux
   does not need); new tests cover the Linux-specific parts. `drtagger-core::store`: SQLite (paths as
   BLOB keys, so non-UTF-8 names survive), `AlbumRecord` columns and
   upsert rules of the Mac app, interrupted states reset on open,
   migrations by `user_version`. `drtagger-cli add | list | remove`.
   Parity: `~/mactagger-samples` identical (14 albums, 8 issues, 10 CUEs,
   8002 file and folder names through the name parser and disc-token
   rules); NAS identical (1048 albums, 828 CUEs with TOCs, disc IDs and
   split plans; after the Mac NFD fix above) except one unreadable NAS
   folder the mirror cannot reproduce. Release-set grouping after a scan stays in
   phase 9.
2. **Containers.** **Done 2026-11-17.** `flackit`: FLAC (blocks, large ones
   by reference; STREAMINFO; Vorbis; rewrite with byte-exact audio, used by
   splitting), DSF reading, ID3v2 (v2.3/2.4 read, v2.3 write) and its
   bridge, with Foundation's text rules (UTF-16 without BOM is big-endian,
   UTF-8 drops a BOM, strict decoding). `tagkit`: TagSet and the Picard
   field list, ID3Mapper, containers FLAC / DSF / WAV / AIFF / DSDIFF /
   APEv2 / MP4, TaggedFile (read, tags-only, preview, backup, write with
   audio SHA-256 check through a temporary file + rename keeping the
   original's permission bits, restore), TagMerge, LibraryIndexer. FLACKit's
   WAVFile/AudioFile and the DSF rewrite are not ported (nothing in the Mac
   app uses them). macOS-only steps are dropped (creation date, the
   provenance "._" sidecar). The Vorbis vendor for files that have none is
   "drtagger for Linux" (`tagkit::set_vendor`; the harness sets the Mac's).
   Deferred: PicardMapper (phase 6, needs providers), LibraryOrganizer
   (phase 3, needs path templates), ArtworkProcessor.prepared (resize +
   JPEG, with the cover work in phase 6). Tests: the Swift container,
   merge, index and composed-name tests ported (fixtures generated on the
   Mac: tone WAV/AIFF/ALAC, cover JPEG/PNG; FLACKit's tone.flac). Parity:
   41 sample files (all 7 containers) read identically and 14 write cycles
   (write + cover, keep, strip, restore) give byte-identical files; 10374
   NAS files read identically (tags, pictures with sizes and hashes, raw
   metadata, preserved frames); only differences are an error message
   wording for a truncated DSF and one NAS folder Mac cannot list.
3. **Split and verify.** **Done 2026-11-17.** `scripts/build-ffmpeg.sh`
   builds the Mac's exact ffmpeg (8.0, same checksum and component list,
   LGPL, static, clang) into Vendor/ffmpeg; the binaries stay out of git.
   `splitkit`: PCMFormat, CRC-32, PathTemplate (ASCII names through the
   system ICU: "Any-Latin", diacritics, width folding — the same results
   as Foundation on all 8002 corpus names), provisional tags, an ffmpeg
   driver (processes with one thread per pipe, a `Cancel` token that kills
   children, low priority for streaming decodes), ImageSplitter (decode,
   plan, TOC, encode, STREAMINFO-MD5 verification with decode fallback,
   CTDB track and disc CRCs, staging in the cache directory instead of a
   RAM-backed /tmp). `tagkit::LibraryOrganizer` (with EXDEV copy+delete
   for moves onto another file system). `drtagger-core`: ffmpeg locator
   (override → bundled next to the executable or Vendor/ffmpeg → PATH) and
   inspection, `disc::compute_toc` and `disc::split` (outcomes and TOC
   stored on the record; originals to the freedesktop trash after a
   verified split); store migration 2. CLI: `split --into`, `ffmpeg`.
   CUETools DB lookup/verification moves to phase 6 with the providers.
   **FLAC bytes:** the same ffmpeg build encodes identically on both for
   most material, but Apple clang 21 and clang 23 compile ffmpeg's LPC
   floating point differently, so an occasional frame is packed
   differently (1 of 13 CD tracks, all 24/96 tracks in the test set; no
   libm call or FMA setting involved — tested). The audio is identical
   (STREAMINFO MD5, CRC), so split parity is checked on audio, CRCs, tags,
   plans and names (`compare.py --ignore sha`), not encoder bytes.
   Parity: the NAS's only two CUE images (the samples) plus a synthetic
   set (WavPack and TTA images, HTOA as track 00, a two-disc "(Disc N)"
   box, a 24/96 image) give identical outcomes: plans, TOCs, disc IDs,
   sample counts, PCM MD5s, CRCs, CTDB track/disc CRCs, provisional tags,
   folder and file names.
4. **SACD.** **Done 2026-11-18.** `dstkit` (LGPL-2.1+, port of the Swift
   port of FFmpeg's dstdec.c; the sample loop without bounds checks or
   allocations) and `sacdkit` (disc reader: area TOCs, SACDTRL1/2,
   SACD_IGL ISRCs and genres, SACDTTxt texts in the disc's character set;
   frame reassembly from audio sectors; DSF writer; extractor with both
   pause policies, DST batches decoded in parallel with rayon, one decoder
   per worker, order kept; ID3 tags from the disc text; cancellation).
   `drtagger-core::disc::extract_sacd` (outcomes on the record, store
   migration 3, ISO to the trash after a verified extraction). CLI
   `extract --into [--multichannel] [--drop-pauses]`. The Swift synthetic
   Scarletbook tests are ported. Parity: the five sample ISOs (2 DSD, 3
   DST, one with a 5-channel DST area) read identically (areas, tracks,
   texts, ISRCs, genres) and their 11 extractions (stereo with both
   policies, multichannel) give 185 DSF files (31.4 GB) byte-identical to
   the Mac's — so also to sacd_extract, which the Mac matched. CPU time
   for the whole set: Rust 1666 s, Swift 2189 s.
5. **Loudness.** **Done 2026-11-18.** `loudnesskit`: R128Meter (K-weighting
   biquads in double precision, 100 ms sub-blocks, 400 ms blocks, absolute
   and relative gates, sample peak, 4×/2× polyphase true peak),
   LoudnessAnalyzer (ffmpeg streaming decode to 32-bit PCM, DSD and
   >192 kHz at 88.2 kHz, two tracks at a time), album gating over all
   blocks, ReplayGain 2.0 / R128 tag strings. CLI `loudness <files>`.
   Swift tests ported, including the cross-check against ffmpeg's ebur128.
   Parity on 35 tracks in 9 albums (FLAC, WAV, ALAC, DSF, 24/96): loudness
   equal to ~1e-11 LU, true peak to ~1e-6 dB; 186 of 187 tag values
   identical, the other differs in the 6th decimal of a peak factor
   (1.000429 vs 1.000430). The cause is vDSP_conv's float summation order
   in the true-peak interpolation, which is not public; the port sums in
   double (the closest of the tried orders, and the more exact one).
6. **Providers and identification (no vision).** **Done 2026-11-18.**
   `providerkit`: blocking clients for MusicBrainz, Discogs, AcoustID,
   CUETools DB (quick-xml), Cover Art Archive, fanart.tv, iTunes and Deezer
   over ureq/rustls, with the Mac's rate limits (slot reservation, 503
   back-off), Foundation-compatible URL encoding and key checks. Requests go
   through a replaceable `Transport`: live, `Recorder` or `Replayer` (keys
   blanked in the recordings). `chromaprintkit`: the same Chromaprint 1.5.1
   sources as the Mac, KissFFT instead of vDSP, built with `cc`.
   `identifykit`: signals, catalog parser, Foundation folding, MatchScorer,
   fingerprint consensus, the Identifier pipeline, cover art sources and the
   CDDB responder; image recognition plugs in through `ArtworkScanning`
   (phase 7). `tagkit`: PicardMapper and ArtworkProcessor (image crate,
   JPEG q90). Core: CTDB report, identify (state and selected candidate in
   the store, migration 4), CDDB server on 127.0.0.1. CLI: `identify`,
   `ctdb`, `cddb-server`. All Swift tests ported, plus live suites.
   Parity (`MacDump identify` records every HTTP exchange; `drtagger-cli
   parity-identify` replays; `parity/compare_identify.py`): 24 albums (9
   samples, 15 from the NAS: Japanese, XRCD, MQA-CD, 24/96, DSF, SACD ISOs,
   untagged, editions) identical in everything — signals, every candidate
   with score, reasons and order, verdict flags, log, Picard tags and
   artwork sources; 229 fingerprints bit-identical to vDSP's. Live
   direction (Linux queries the services, Mac replays Linux's recordings):
   identical too. The Mac reference is built with the vote tie-break fix
   (see the Mac bugs list).
7. **Vision.** **Done 2026-11-18.** `visionkit`: zxing-cpp 2 (bundled,
   static) for EAN-13/EAN-8/UPC-E; PaddleOCR PP-OCRv5 mobile detection +
   recognition (zh/ja/en) and DINOv2 ViT-S/14 embeddings on ONNX Runtime
   1.31 (`ort` 2.0, pyke's static build). Models are not in git:
   `scripts/fetch-models.sh` pins Hugging Face revisions and sha256s into
   `models/` (118 MB; looked up via `$DRTAGGER_MODELS`, `$XDG_DATA_DIRS`,
   next to the binary, the source tree). Without models barcodes still
   work. `ArtworkScanner` follows the Mac's (1800 px, three orientations
   for barcodes, rotated rescans of elongated images) with two additions
   zxing/PaddleOCR need: barcodes retried at full size and, for small
   images, enlarged ×2/×3; tall text boxes read in both quarter turns
   (Vision reads rotated lines natively). Plugged into core identify.
   `CoverCheck` with DINOv2 distances mapped onto Vision's scale.
   Benchmark (`MacDump vision-scan|vision-pairs`, `drtagger-cli
   parity-vision-scan|parity-vision-pairs`, `parity/compare_vision.py`,
   `parity/cover_pairs.py`, `parity/compare_cover.py`) on 1151 images of
   197 NAS rips (≤12 prioritised per album, as identification reads them):
   barcodes 96.4% of Vision's per image, 96.9% per album, 3 extra (misreads);
   catalog numbers 77% of Vision's, but what Vision alone finds is mostly
   OCR noise ("IGOR191965", "DURING1978"): against ground truth both find
   the same — album tags 25/61 each, catalogs in folder names 31 vs 30 of
   32. Japanese text is read far better than Vision's. Cover check on 832
   pairs (own release, other editions, the artist's other albums, other
   albums): the mapping decides as Vision at 0.3/0.35/0.6/0.95 in
   100/99.9/98.0/93.6% of pairs, and at 0.95 is right more often (own
   scans warned 11% vs 16%, other albums 99.7% vs 93%). Cost: OCR is CPU
   only, ~4.8 s per image on the 6-core VM with 3 at once vs Vision's 0.57 s
   on the Neural Engine (dense booklet pages dominate: ~80 ms per text
   line per core); about 20 s per album of 12 scans. Accepted.
8. **UI.** **Done 2026-11-18.** Core first: `drtagger-core::tagging`
   (TagService: plan with album/track diff, locks, artwork options and
   choice, cover check warning; Apply with ReplayGain first, backups,
   audio-verified writes, cover file, library layout; organize-only;
   Restore), `settings` (AppSettings), store migration 5 (locks, backups,
   reports, cover choice, file moves, set and batch columns), set folders
   for split/extract, CLI `tag` / `restore`. Parity against the real Mac
   app (`--add --identify --tag` on copies of Diamond Dogs, Ella and Oscar,
   Corre, corre; Linux tagged the same copies from the Mac's stored
   identification): same library layout and file names, 49 of 50 files
   byte-identical (tags, ReplayGain, embedded cover, cover.jpg), the other
   differs only in the known 6th decimal of one ReplayGain peak; Restore is
   byte-exact. Two Mac bugs found (see the list). GTK app (`app/`): queue
   sidebar (states, kind, subtitle, set badge, batch notes, filter, context
   menu, Delete, drag and drop, add folders / image files, scan status with
   the network stall watchdog, empty state), album inspector (header,
   details: source, SACD, discs with CUE and tracks, artwork strip; notes,
   errors), image viewer (files, data, URLs), workflow step bar with the
   Mac's next-action logic (Process All, Extract & Identify, auto preview
   on change), identification (verdict, signals, providers, candidates
   with photos, badges, reasons, plausible-format folding, log, release
   gallery, compare covers with obis), disc identity (IDs, FreeDB check,
   CUETools DB with verification, split with per-track results), SACD
   extraction, tag preview (summary, warnings, cover picker, Apply effects,
   album and track fields with locks, Restore, Apply). Work runs on worker
   threads with their own SQLite connection; progress, errors and busy
   state are per service (identify / disc / tags) as on Mac. Preferences:
   template preview, key Test buttons and links, cover sizes, ffmpeg status
   and chooser, CDDB server (started by the app) with status. Spanish via
   gettext: `po/es.po` (scripts/update-po.py reuses the Mac catalog for 226
   of 332 strings), compiled by build.rs. Screens checked on the VM over
   D-Bus actions (`scripts/shot.sh`; hidden `win.select-album`,
   `win.scroll-inspector`).
9. **Release sets and bulk.** **Done 2026-11-18.** `drtagger-core::sets`
   (LibraryController grouping: siblings by disc token or SACD album set
   after every scan, joining a set already in the queue, lone "disc 2 of 3"
   sets of one, folders whose files name one release; group, ungroup, disc
   positions; only the grouping columns are written back),
   `library_index` (LibraryIndexService: JSON in the data dir, incremental
   refresh, duplicates by release ID or barcode + catalog),
   `batch` (BatchRunner: four identification workers and one CPU worker on
   threads with their own store connection, SACD extraction first, the
   Mac's decision ladder — confident, files' own MBID, clear barcode,
   clear fingerprints, cover tie-break, album-only pick, SACD without a
   SACD edition — then files, plan, the other-release and duplicate
   checks, preview notes, organize-only or Apply, index update;
   statuses on the records, resume after quitting). The batch columns are
   written only through `Store::set_batch`, so a worker that read a record
   earlier (the disc section's TOC) cannot put an old status back. Provider
   keys can come from ACOUSTID_KEY / DISCOGS_TOKEN / FANART_KEY (the Mac's
   `--acoustid-key` arguments). CLI `batch [import|update] [--dry-run]
   [--resume] --library-root`, `batch-clear`. App: Process Queue dialog,
   Stop, batch table (sortable columns, filter, CSV report, activate to
   open), sidebar filter with counts, batch status line (progress, review
   and error counts, Stop / Resume / Close Batch, preview, time left,
   activity), review banner (Leave As Is, Next to Review), Release set
   section (members with disc positions, Group with siblings, Split /
   Extract All Discs, Ungroup), context menu (Group as One Release, Group
   with Folder Siblings, Ungroup Release Set); hidden `win.start-batch`
   for scripts. The Swift LibraryController grouping tests are ported.
   Parity: the real Mac app (`--add --batch import --dry-run`, queue in
   memory, library root and index in scratch) and the CLI on the 7 sample
   albums give the same status and note for every album, reached the same
   way (fast path / fingerprints). A real import on Linux, a second batch
   (duplicate copy set aside as "Already in the library") and a two-disc
   set (identified as one release) checked on the VM. One more Mac bug
   (moved albums keep their old scan details; see the list).
   The album-only pick is the batch's, as on Mac (no separate UI).
10. **Rip inbox.** **Done 2026-11-18, Linux first** (the Mac app does not
    have it yet; Carlos chose to build it here and port it to Mac later
    with Linux as the reference). Follows the plan agreed on 2026-10-19.
    `drtagger-core::inbox`: every top-level entry of the local rip folder
    is one rip; a snapshot (names, sizes, mtimes; logs read) decides when
    it is finished: a ripper log (EAC incl. UTF-16 and old code-page logs,
    XLD, whipper, CUERipper; checked on 458 NAS logs) and 60 s without a
    change, else the quiet time (default 10 min); an ISO when it stops
    growing that long. Finished rips are scanned into the queue (with set
    grouping) and handed to the batch (`BatchRunner::enqueue`: joins a
    running import, waits for a running preview to end, else starts one);
    what was handed over is remembered with its fingerprint
    (`inbox-seen.json`), so only a changed rip comes back. Split and
    extracted tracks are written to `<inbox>/.drtagger-staging`, tagged
    there and moved; moves across file systems (local disk → NAS) copy to a
    hidden temporary file, flush, compare SHA-256 with the original, rename,
    and only then delete the original (`tagkit::library_organizer`,
    for every move). Unsure rips stay in the inbox as reviews; Apply by
    hand moves them. App: Preferences group (watch, folder, quiet time,
    start at login), polling every 15 s, notification when a batch ends
    ("n imported / n to review / n failed", opening the review filter),
    window hidden instead of closed while the inbox is on, `--background`
    start, login item through the Background portal in the Flatpak or an
    XDG autostart file outside it, idle ONNX Runtime sessions released
    after a minute without identification (`visionkit::release_idle_sessions`;
    2.7 GB → 0.29 GB after a batch). Checked on the VM: three rips written
    file by file into the inbox (Walkin' CUE image with log, Diamond Dogs
    with log, Corre, corre without log), library on tmpfs (another file
    system): Walkin' split in staging, tagged and moved; Diamond Dogs
    (already tagged) moved with its log, CUE and scans; Corre, corre picked
    up after the quiet time and left for review; no partial copies.
    Not done: leftovers of a CUE-image rip (image, CUE, log, scans) stay in
    the inbox after its tracks are imported; a menu-bar/tray icon (GNOME
    has none; notifications and the background portal instead); re-checking
    reviews periodically and the MusicBrainz release editor (later items of
    the plan).
11. **Distribution.** **Done 2026-11-18 for aarch64**; x86_64 waits for the
    CI (no x86 machine here). `build-aux/us.priet.drtagger.json`: GNOME 50,
    offline build — crates from `build-aux/cargo-sources.json`
    (`scripts/cargo-sources.py`, run after every Cargo.lock change), FFmpeg
    8.0 from its tarball with the LGPL configuration (the build fails on a
    GPL result), ONNX Runtime 1.31 per architecture (pyke's raw-LZMA2 tar,
    unpacked with xz), the five models; translations kept in the app
    (`separate-locales: false`, a bundle carries no Locale extension).
    ICU through rust_icu's pre-generated bindings for
    RUST_ICU_MAJOR_VERSION_NUMBER (the SDK has no libclang for bindgen).
    Inside the sandbox originals go to the host trash through the Trash
    portal (`drtagger-core::trash`) and the login item through the
    Background portal. Licences: THIRD_PARTY_NOTICES.md, the LGPL texts and
    CRATE-LICENSES.txt in /app/share/licenses. `scripts/release.sh` builds a
    bundle for the local architecture; `.github/workflows/flatpak.yml`
    builds both and drafts a release on a v* tag. Checked: the aarch64
    bundle (120 MB) installs and runs on the VM (scan, TOC, bundled ffmpeg,
    models, Spanish catalog). Found by the SDK build: Chromaprint compiled
    against the system's chromaprint.h on the VM; the vendored header is
    used now. Still to check inside the sandbox: keyring (Secret portal)
    and trash (Trash portal); Flathub later.

Order note: phases 1-7 also deliver `drtagger-cli`, so each one is usable
and testable headless before the UI exists.
