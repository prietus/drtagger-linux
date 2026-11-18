//! The album queue, persisted in SQLite (the Mac app's SwiftData
//! `AlbumRecord`). The scanner's full `DetectedAlbum` is kept as JSON so the
//! inspector can show discs, tracks, CUE and SACD details without
//! rescanning; the scalar columns exist for sorting, filtering and the
//! sidebar row. Later phases add columns through `MIGRATIONS`.

use anyhow::{Context, Result};
use librarykit::{AlbumKind, DetectedAlbum, ScanIssue, ScanResult, standardized};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Lifecycle of an album in the queue, stored as the Mac app's raw values.
///   pending → scanning → scanned → identifying → confident | needsReview
///   → applying → done, with `error` reachable from any step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AlbumState {
    Pending,
    Scanning,
    Scanned,
    Identifying,
    Confident,
    NeedsReview,
    Applying,
    Done,
    Error,
}

impl AlbumState {
    pub const ALL: [AlbumState; 9] = [
        AlbumState::Pending,
        AlbumState::Scanning,
        AlbumState::Scanned,
        AlbumState::Identifying,
        AlbumState::Confident,
        AlbumState::NeedsReview,
        AlbumState::Applying,
        AlbumState::Done,
        AlbumState::Error,
    ];

    pub fn raw_value(self) -> &'static str {
        match self {
            AlbumState::Pending => "pending",
            AlbumState::Scanning => "scanning",
            AlbumState::Scanned => "scanned",
            AlbumState::Identifying => "identifying",
            AlbumState::Confident => "confident",
            AlbumState::NeedsReview => "needsReview",
            AlbumState::Applying => "applying",
            AlbumState::Done => "done",
            AlbumState::Error => "error",
        }
    }

    pub fn from_raw(raw: &str) -> AlbumState {
        Self::ALL.into_iter().find(|s| s.raw_value() == raw).unwrap_or(AlbumState::Pending)
    }

    pub fn label(self) -> &'static str {
        match self {
            AlbumState::Pending => "Pending",
            AlbumState::Scanning => "Scanning",
            AlbumState::Scanned => "Ready to identify",
            AlbumState::Identifying => "Identifying",
            AlbumState::Confident => "Confident match",
            AlbumState::NeedsReview => "Needs review",
            AlbumState::Applying => "Applying",
            AlbumState::Done => "Done",
            AlbumState::Error => "Error",
        }
    }

    /// States a relaunch or rescan collapses back to `Scanned`.
    fn is_transient(self) -> bool {
        matches!(self, AlbumState::Pending | AlbumState::Scanning | AlbumState::Error)
    }
}

/// One album in the queue.
#[derive(Clone, Debug)]
pub struct AlbumRecord {
    /// Album folder or ISO file (standardised); the primary key.
    pub path: PathBuf,
    pub kind: AlbumKind,
    pub state: AlbumState,
    pub display_title: String,
    pub artist_hint: Option<String>,
    pub year_hint: Option<String>,
    pub disc_count: i64,
    pub track_count: i64,
    /// "flac,dsf".
    pub formats: String,
    pub has_dst: bool,
    pub has_multichannel: bool,
    pub details_json: Option<String>,
    pub issues_json: Option<String>,
    pub error_message: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub added_at: i64,
    pub updated_at: i64,
    // Phase 3: disc identity and split results (JSON).
    pub toc_json: Option<String>,
    pub split_outcomes_json: Option<String>,
    // Phase 4: SACD extraction results (JSON).
    pub sacd_outcomes_json: Option<String>,
    // Phase 6: CUETools DB report, identification and the chosen release.
    pub ctdb_json: Option<String>,
    pub identification_json: Option<String>,
    pub selected_candidate_id: Option<String>,
    /// The album is certain but not the edition: tag the album only.
    pub album_only: bool,
    // Phase 8: tag locks, the original metadata of every written file, the
    // write reports, the artwork choice and where the organizer moved files.
    pub tag_locks_json: Option<String>,
    pub tag_backups_json: Option<String>,
    pub tag_reports_json: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub tagged_at: Option<i64>,
    pub cover_option_id: Option<String>,
    pub file_moves_json: Option<String>,
    // Release sets: several records that are one multi-disc release.
    pub set_id: Option<String>,
    pub set_position: Option<i64>,
    pub set_total: Option<i64>,
    pub set_title: Option<String>,
    // Bulk mode.
    pub batch_status: Option<String>,
    pub batch_note: Option<String>,
    /// The release the files already name ("mb:<id>" / "discogs:<id>") and
    /// their DISCNUMBER, read from one track at scan time.
    pub tagged_release_key: Option<String>,
    pub tagged_disc_number: Option<i64>,
}

impl AlbumRecord {
    pub fn new(detected: &DetectedAlbum, issues: &[ScanIssue], added_at: i64) -> Self {
        let mut r = AlbumRecord {
            path: detected.id(),
            kind: detected.kind,
            state: AlbumState::Scanned,
            display_title: String::new(),
            artist_hint: None,
            year_hint: None,
            disc_count: 0,
            track_count: 0,
            formats: String::new(),
            has_dst: false,
            has_multichannel: false,
            details_json: None,
            issues_json: None,
            error_message: None,
            added_at,
            updated_at: added_at,
            toc_json: None,
            split_outcomes_json: None,
            sacd_outcomes_json: None,
            ctdb_json: None,
            identification_json: None,
            selected_candidate_id: None,
            album_only: false,
            tag_locks_json: None,
            tag_backups_json: None,
            tag_reports_json: None,
            tagged_at: None,
            cover_option_id: None,
            file_moves_json: None,
            set_id: None,
            set_position: None,
            set_total: None,
            set_title: None,
            batch_status: None,
            batch_note: None,
            tagged_release_key: None,
            tagged_disc_number: None,
        };
        r.apply(detected, issues);
        r
    }

    /// Refreshes every scan-derived column.
    pub fn apply(&mut self, detected: &DetectedAlbum, issues: &[ScanIssue]) {
        self.kind = detected.kind;
        self.display_title = detected.title_hint().unwrap_or_else(|| detected.folder_name());
        self.artist_hint = detected.artist_hint();
        self.year_hint = detected.year_hint();
        self.disc_count = if detected.kind == AlbumKind::SacdIso { 1 } else { detected.discs.len().max(1) as i64 };
        self.track_count = detected.track_count() as i64;
        let mut formats: Vec<&str> = detected.formats().iter().map(|f| f.raw_value()).collect();
        formats.sort();
        self.formats = formats.join(",");
        self.has_dst = detected.sacd.as_ref().is_some_and(|s| s.has_dst());
        self.has_multichannel = detected.sacd.as_ref().is_some_and(|s| s.multichannel_area().is_some());
        self.details_json = serde_json::to_string(detected).ok();
        self.issues_json = serde_json::to_string(issues).ok();
        self.error_message = None;
        self.updated_at = now_ms();
    }

    pub fn detected(&self) -> Option<DetectedAlbum> {
        serde_json::from_str(self.details_json.as_deref()?).ok()
    }

    pub fn toc(&self) -> Option<librarykit::DiscTOC> {
        serde_json::from_str(self.toc_json.as_deref()?).ok()
    }

    pub fn set_toc(&mut self, toc: Option<&librarykit::DiscTOC>) {
        self.toc_json = toc.and_then(|t| serde_json::to_string(t).ok());
        self.updated_at = now_ms();
    }

    /// One outcome per split disc.
    pub fn split_outcomes(&self) -> Vec<splitkit::SplitOutcome> {
        self.split_outcomes_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
    }

    pub fn set_split_outcomes(&mut self, outcomes: &[splitkit::SplitOutcome]) {
        self.split_outcomes_json = serde_json::to_string(outcomes).ok();
        self.updated_at = now_ms();
    }

    /// One outcome per extracted area.
    pub fn sacd_outcomes(&self) -> Vec<sacdkit::SACDExtractOutcome> {
        self.sacd_outcomes_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
    }

    pub fn set_sacd_outcomes(&mut self, outcomes: &[sacdkit::SACDExtractOutcome]) {
        self.sacd_outcomes_json = serde_json::to_string(outcomes).ok();
        self.updated_at = now_ms();
    }

    pub fn ctdb(&self) -> Option<crate::identify::CtdbReport> {
        serde_json::from_str(self.ctdb_json.as_deref()?).ok()
    }

    pub fn set_ctdb(&mut self, report: Option<&crate::identify::CtdbReport>) {
        self.ctdb_json = report.and_then(|r| serde_json::to_string(r).ok());
        self.updated_at = now_ms();
    }

    pub fn identification(&self) -> Option<identifykit::IdentificationResult> {
        serde_json::from_str(self.identification_json.as_deref()?).ok()
    }

    pub fn set_identification(&mut self, result: Option<&identifykit::IdentificationResult>) {
        self.identification_json = result.and_then(|r| serde_json::to_string(r).ok());
        self.updated_at = now_ms();
    }

    pub fn tag_locks(&self) -> std::collections::BTreeSet<String> {
        self.tag_locks_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
    }

    pub fn set_tag_locks(&mut self, locks: &std::collections::BTreeSet<String>) {
        self.tag_locks_json = if locks.is_empty() { None } else { serde_json::to_string(locks).ok() };
        self.updated_at = now_ms();
    }

    /// Original metadata of every file written by Apply; kept until Restore.
    pub fn tag_backups(&self) -> Vec<tagkit::TagBackup> {
        self.tag_backups_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
    }

    pub fn set_tag_backups(&mut self, backups: &[tagkit::TagBackup]) {
        self.tag_backups_json = if backups.is_empty() { None } else { serde_json::to_string(backups).ok() };
        self.updated_at = now_ms();
    }

    pub fn tag_reports(&self) -> Vec<tagkit::TagWriteReport> {
        self.tag_reports_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
    }

    pub fn set_tag_reports(&mut self, reports: &[tagkit::TagWriteReport]) {
        self.tag_reports_json = if reports.is_empty() { None } else { serde_json::to_string(reports).ok() };
        self.updated_at = now_ms();
    }

    /// Where the organizer moved files: old path → new path.
    pub fn file_moves(&self) -> std::collections::BTreeMap<PathBuf, PathBuf> {
        self.file_moves_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
    }

    pub fn set_file_moves(&mut self, moves: &std::collections::BTreeMap<PathBuf, PathBuf>) {
        self.file_moves_json = if moves.is_empty() { None } else { serde_json::to_string(moves).ok() };
        self.updated_at = now_ms();
    }

    /// Follows recorded moves so stored paths still point at the files.
    pub fn resolved(&self, path: &Path) -> PathBuf {
        let moves = self.file_moves();
        let mut current = path.to_path_buf();
        for _ in 0..8 {
            match moves.get(&current) {
                Some(next) => current = next.clone(),
                None => break,
            }
        }
        current
    }

    /// The disc position this record's files cover: its set position, else
    /// the "disc 2 of 3" evidence in its own name, else 1.
    pub fn disc_position(&self) -> i64 {
        self.set_position.or_else(|| self.detected().and_then(|d| d.disc_position()).map(|p| p.0 as i64)).unwrap_or(1)
    }

    pub fn selected_candidate(&self) -> Option<identifykit::ScoredCandidate> {
        let id = self.selected_candidate_id.as_deref()?;
        self.identification()?.candidates.into_iter().find(|c| c.id() == id)
    }

    pub fn issues(&self) -> Vec<ScanIssue> {
        self.issues_json.as_deref().and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
    }

    /// Has a CUE, so a disc TOC and a CUETools DB check make sense.
    pub fn has_cue_sheet(&self) -> bool {
        matches!(self.kind, AlbumKind::CueImage | AlbumKind::CueMultiFile)
    }

    /// One image file per disc: splitting produces the tracks.
    pub fn is_splittable(&self) -> bool {
        self.kind == AlbumKind::CueImage
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Schema steps, applied in order; `PRAGMA user_version` counts how many ran.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE albums (
        path BLOB PRIMARY KEY NOT NULL,
        kind TEXT NOT NULL,
        state TEXT NOT NULL,
        display_title TEXT NOT NULL,
        artist_hint TEXT,
        year_hint TEXT,
        disc_count INTEGER NOT NULL,
        track_count INTEGER NOT NULL,
        formats TEXT NOT NULL,
        has_dst INTEGER NOT NULL,
        has_multichannel INTEGER NOT NULL,
        details_json TEXT,
        issues_json TEXT,
        error_message TEXT,
        added_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX albums_added ON albums (added_at, display_title);",
    "ALTER TABLE albums ADD COLUMN toc_json TEXT;
    ALTER TABLE albums ADD COLUMN split_outcomes_json TEXT;",
    "ALTER TABLE albums ADD COLUMN sacd_outcomes_json TEXT;",
    "ALTER TABLE albums ADD COLUMN ctdb_json TEXT;
    ALTER TABLE albums ADD COLUMN identification_json TEXT;
    ALTER TABLE albums ADD COLUMN selected_candidate_id TEXT;
    ALTER TABLE albums ADD COLUMN album_only INTEGER NOT NULL DEFAULT 0;",
    "ALTER TABLE albums ADD COLUMN tag_locks_json TEXT;
    ALTER TABLE albums ADD COLUMN tag_backups_json TEXT;
    ALTER TABLE albums ADD COLUMN tag_reports_json TEXT;
    ALTER TABLE albums ADD COLUMN tagged_at INTEGER;
    ALTER TABLE albums ADD COLUMN cover_option_id TEXT;
    ALTER TABLE albums ADD COLUMN file_moves_json TEXT;
    ALTER TABLE albums ADD COLUMN set_id TEXT;
    ALTER TABLE albums ADD COLUMN set_position INTEGER;
    ALTER TABLE albums ADD COLUMN set_total INTEGER;
    ALTER TABLE albums ADD COLUMN set_title TEXT;
    ALTER TABLE albums ADD COLUMN batch_status TEXT;
    ALTER TABLE albums ADD COLUMN batch_note TEXT;
    ALTER TABLE albums ADD COLUMN tagged_release_key TEXT;
    ALTER TABLE albums ADD COLUMN tagged_disc_number INTEGER;",
];

const COLUMN_NAMES: &[&str] = &[
    "path",
    "kind",
    "state",
    "display_title",
    "artist_hint",
    "year_hint",
    "disc_count",
    "track_count",
    "formats",
    "has_dst",
    "has_multichannel",
    "details_json",
    "issues_json",
    "error_message",
    "added_at",
    "updated_at",
    "toc_json",
    "split_outcomes_json",
    "sacd_outcomes_json",
    "ctdb_json",
    "identification_json",
    "selected_candidate_id",
    "album_only",
    "tag_locks_json",
    "tag_backups_json",
    "tag_reports_json",
    "tagged_at",
    "cover_option_id",
    "file_moves_json",
    "set_id",
    "set_position",
    "set_total",
    "set_title",
    "batch_status",
    "batch_note",
    "tagged_release_key",
    "tagged_disc_number",
];

fn columns() -> &'static str {
    static COLUMNS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    COLUMNS.get_or_init(|| COLUMN_NAMES.join(", "))
}

fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

fn from_row(row: &Row) -> rusqlite::Result<AlbumRecord> {
    let path: Vec<u8> = row.get(0)?;
    let kind: String = row.get(1)?;
    let state: String = row.get(2)?;
    Ok(AlbumRecord {
        path: PathBuf::from(std::ffi::OsString::from_vec(path)),
        kind: AlbumKind::from_raw(&kind).unwrap_or(AlbumKind::TrackFolder),
        state: AlbumState::from_raw(&state),
        display_title: row.get(3)?,
        artist_hint: row.get(4)?,
        year_hint: row.get(5)?,
        disc_count: row.get(6)?,
        track_count: row.get(7)?,
        formats: row.get(8)?,
        has_dst: row.get(9)?,
        has_multichannel: row.get(10)?,
        details_json: row.get(11)?,
        issues_json: row.get(12)?,
        error_message: row.get(13)?,
        added_at: row.get(14)?,
        updated_at: row.get(15)?,
        toc_json: row.get(16)?,
        split_outcomes_json: row.get(17)?,
        sacd_outcomes_json: row.get(18)?,
        ctdb_json: row.get(19)?,
        identification_json: row.get(20)?,
        selected_candidate_id: row.get(21)?,
        album_only: row.get(22)?,
        tag_locks_json: row.get(23)?,
        tag_backups_json: row.get(24)?,
        tag_reports_json: row.get(25)?,
        tagged_at: row.get(26)?,
        cover_option_id: row.get(27)?,
        file_moves_json: row.get(28)?,
        set_id: row.get(29)?,
        set_position: row.get(30)?,
        set_total: row.get(31)?,
        set_title: row.get(32)?,
        batch_status: row.get(33)?,
        batch_note: row.get(34)?,
        tagged_release_key: row.get(35)?,
        tagged_disc_number: row.get(36)?,
    })
}

/// What an upsert did, for the status line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpsertSummary {
    pub added: usize,
    pub updated: usize,
    pub folders_visited: usize,
}

impl std::fmt::Display for UpsertSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} added, {} updated, {} folders scanned", self.added, self.updated, self.folders_visited)
    }
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// The app's store in the data directory.
    pub fn open_default() -> Result<Store> {
        let dir = crate::paths::data_dir();
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Self::open(&dir.join("drtagger.sqlite"))
    }

    pub fn open(path: &Path) -> Result<Store> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // The app's main thread and its workers each hold a connection.
        conn.busy_timeout(std::time::Duration::from_secs(15))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let mut store = Store { conn };
        store.migrate()?;
        store.reset_transient_states()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Store> {
        let mut store = Store { conn: Connection::open_in_memory()? };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let tx = self.conn.transaction()?;
        for (i, step) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            tx.execute_batch(step).with_context(|| format!("store migration {}", i + 1))?;
        }
        tx.pragma_update(None, "user_version", MIGRATIONS.len() as i64)?;
        tx.commit()?;
        Ok(())
    }

    /// A previous run may have died mid-operation; those states mean
    /// nothing after a relaunch.
    fn reset_transient_states(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE albums SET state = 'scanned' WHERE state IN ('scanning', 'identifying', 'applying')",
            [],
        )?;
        Ok(())
    }

    pub fn fetch(&self, path: &Path) -> Result<Option<AlbumRecord>> {
        fetch_record(&self.conn, path)
    }

    /// Every record in queue order: by scan, then by title.
    pub fn all(&self) -> Result<Vec<AlbumRecord>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {} FROM albums ORDER BY added_at, display_title COLLATE NOCASE", columns()))?;
        let rows = stmt.query_map([], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save(&self, r: &AlbumRecord) -> Result<()> {
        save_record(&self.conn, r)
    }

    /// Saves a record whose path (the key) changed from `old`.
    /// Saves a record whose path (the key) changed from `old`. The batch
    /// columns move with it as they are stored.
    pub fn save_moved(&self, old: &Path, r: &AlbumRecord) -> Result<()> {
        if old == r.path {
            return save_record(&self.conn, r);
        }
        let stored: Option<(Option<String>, Option<String>)> = self
            .conn
            .query_row("SELECT batch_status, batch_note FROM albums WHERE path = ?1", [path_bytes(old)], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        self.conn.execute("DELETE FROM albums WHERE path = ?1", [path_bytes(old)])?;
        save_record(&self.conn, r)?;
        if let Some((status, note)) = stored {
            self.set_batch(&r.path, status.as_deref(), note.as_deref())?;
        }
        Ok(())
    }

    /// The batch status and note of one album: the only way they change
    /// once the record exists.
    pub fn set_batch(&self, path: &Path, status: Option<&str>, note: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE albums SET batch_status = ?2, batch_note = ?3 WHERE path = ?1",
            params![path_bytes(path), status, note],
        )?;
        Ok(())
    }

    pub fn remove(&self, path: &Path) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM albums WHERE path = ?1", [path_bytes(path)])? > 0)
    }

    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM albums", [])?;
        Ok(())
    }

    /// Stores every album of a scan. Existing records (same path) are
    /// refreshed instead of duplicated; states that carry identification
    /// progress survive, only pending / scanning / error collapse to scanned.
    pub fn upsert(&mut self, result: &ScanResult) -> Result<UpsertSummary> {
        let scan_time = now_ms(); // one timestamp per scan so the queue orders it by title
        let mut summary = UpsertSummary { folders_visited: result.folders_visited, ..Default::default() };
        let tx = self.conn.transaction()?;
        for album in &result.albums {
            let issues = issues_for(result, album);
            match fetch_record(&tx, &album.id())? {
                Some(mut existing) => {
                    existing.apply(album, &issues);
                    if existing.state.is_transient() {
                        existing.state = AlbumState::Scanned;
                    }
                    save_record(&tx, &existing)?;
                    summary.updated += 1;
                }
                None => {
                    save_record(&tx, &AlbumRecord::new(album, &issues, scan_time))?;
                    summary.added += 1;
                }
            }
        }
        tx.commit()?;
        Ok(summary)
    }
}

fn fetch_record(conn: &Connection, path: &Path) -> Result<Option<AlbumRecord>> {
    Ok(conn
        .query_row(&format!("SELECT {} FROM albums WHERE path = ?1", columns()), [path_bytes(path)], from_row)
        .optional()?)
}

const BATCH_COLUMNS: [&str; 2] = ["batch_status", "batch_note"];

fn save_record(conn: &Connection, r: &AlbumRecord) -> Result<()> {
    static SQL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let sql = SQL.get_or_init(|| {
        let placeholders: Vec<String> = (1..=COLUMN_NAMES.len()).map(|i| format!("?{i}")).collect();
        // The batch columns belong to the batch (`set_batch`): a worker
        // that read a record before the batch marked it must not put the
        // old status back.
        let updates: Vec<String> = COLUMN_NAMES
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, c)| !BATCH_COLUMNS.contains(c))
            .map(|(i, c)| format!("{c} = ?{}", i + 1))
            .collect();
        format!(
            "INSERT INTO albums ({}) VALUES ({}) ON CONFLICT(path) DO UPDATE SET {}",
            columns(),
            placeholders.join(", "),
            updates.join(", ")
        )
    });
    conn.execute(
        sql,
        params![
            path_bytes(&r.path),
            r.kind.raw_value(),
            r.state.raw_value(),
            r.display_title,
            r.artist_hint,
            r.year_hint,
            r.disc_count,
            r.track_count,
            r.formats,
            r.has_dst,
            r.has_multichannel,
            r.details_json,
            r.issues_json,
            r.error_message,
            r.added_at,
            r.updated_at,
            r.toc_json,
            r.split_outcomes_json,
            r.sacd_outcomes_json,
            r.ctdb_json,
            r.identification_json,
            r.selected_candidate_id,
            r.album_only,
            r.tag_locks_json,
            r.tag_backups_json,
            r.tag_reports_json,
            r.tagged_at,
            r.cover_option_id,
            r.file_moves_json,
            r.set_id,
            r.set_position,
            r.set_total,
            r.set_title,
            r.batch_status,
            r.batch_note,
            r.tagged_release_key,
            r.tagged_disc_number,
        ],
    )?;
    Ok(())
}

/// Issues raised inside this album's folder (or next to the ISO).
pub fn issues_for(result: &ScanResult, album: &DetectedAlbum) -> Vec<ScanIssue> {
    let folder = if album.kind == AlbumKind::SacdIso {
        album.url.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        album.url.clone()
    };
    let prefix = path_bytes(&standardized(&folder)).to_vec();
    result.issues.iter().filter(|i| path_bytes(&standardized(&i.url)).starts_with(&prefix)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use librarykit::LibraryScanner;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for f in ["Artist - Album (2001)/01.flac", "Artist - Album (2001)/02.flac", "Other/01.dsf"] {
            let p = dir.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, [0u8; 4]).unwrap();
        }
        std::fs::write(
            dir.path().join("Other/x.cue"),
            "FILE \"gone.flac\" WAVE\n TRACK 01 AUDIO\n INDEX 01 00:00:00\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn upsert_adds_then_refreshes_keeping_progress() {
        let dir = tree();
        let db = dir.path().join("store.sqlite");
        let mut store = Store::open(&db).unwrap();
        let result = LibraryScanner::new().scan(&[dir.path().to_path_buf()]);
        let s = store.upsert(&result).unwrap();
        assert_eq!((s.added, s.updated), (2, 0));
        assert_eq!(s.to_string(), format!("2 added, 0 updated, {} folders scanned", result.folders_visited));

        let all = store.all().unwrap();
        assert_eq!(all.len(), 2);
        let album = all.iter().find(|r| r.display_title == "Album").unwrap();
        assert_eq!(album.artist_hint.as_deref(), Some("Artist"));
        assert_eq!(album.year_hint.as_deref(), Some("2001"));
        assert_eq!((album.track_count, album.disc_count, album.formats.as_str()), (2, 1, "flac"));
        assert_eq!(album.state, AlbumState::Scanned);
        assert_eq!(album.detected().unwrap().discs[0].track_files.len(), 2);

        // The orphan CUE's issue belongs to the album in that folder only.
        let other = all.iter().find(|r| r.path.ends_with("Other")).unwrap();
        assert_eq!(other.issues().len(), 1);
        assert!(album.issues().is_empty());

        let mut progressed = album.clone();
        progressed.state = AlbumState::NeedsReview;
        store.save(&progressed).unwrap();
        let mut errored = other.clone();
        errored.state = AlbumState::Error;
        store.save(&errored).unwrap();

        let s = store.upsert(&result).unwrap();
        assert_eq!((s.added, s.updated), (0, 2));
        assert_eq!(store.fetch(&album.path).unwrap().unwrap().state, AlbumState::NeedsReview);
        assert_eq!(store.fetch(&other.path).unwrap().unwrap().state, AlbumState::Scanned);
        assert_eq!(store.fetch(&album.path).unwrap().unwrap().added_at, album.added_at);
    }

    #[test]
    fn reopening_keeps_rows_and_resets_interrupted_work() {
        let dir = tree();
        let db = dir.path().join("store.sqlite");
        {
            let mut store = Store::open(&db).unwrap();
            store.upsert(&LibraryScanner::new().scan(&[dir.path().join("Other")])).unwrap();
            let mut r = store.all().unwrap().remove(0);
            r.state = AlbumState::Identifying;
            store.save(&r).unwrap();
        }
        let store = Store::open(&db).unwrap();
        let all = store.all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].state, AlbumState::Scanned);
        assert!(store.remove(&all[0].path).unwrap());
        assert!(store.all().unwrap().is_empty());
    }

    #[test]
    fn non_utf8_paths_round_trip() {
        use std::os::unix::ffi::OsStringExt;
        let store = Store::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let name = std::ffi::OsString::from_vec(b"Le\xF1o".to_vec()); // Latin-1 "Leño"
        let folder = dir.path().join(&name);
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("01.flac"), [0u8; 2]).unwrap();
        let result = LibraryScanner::new().scan(std::slice::from_ref(&folder));
        let rec = AlbumRecord::new(&result.albums[0], &[], 1);
        store.save(&rec).unwrap();
        let back = store.fetch(&folder).unwrap().unwrap();
        assert_eq!(back.path, folder);
        assert_eq!(back.detected().unwrap().discs[0].track_files[0].url, folder.join("01.flac"));
    }
}
