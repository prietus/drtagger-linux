mod parity;
mod parity_identify;
mod parity_loudness;
mod parity_sacd;
mod parity_split;
mod parity_tags;
mod parity_vision;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use drtagger_core::keys::{self, Provider};
use drtagger_core::store::Store;
use librarykit::LibraryScanner;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "drtagger-cli", version, about = "Headless drtagger")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show where drtagger keeps its data and which provider keys are set.
    Status,
    /// Store a provider key read from stdin (empty input deletes it).
    /// Providers: acoustid, discogs, fanart.
    SetKey { provider: String },
    /// Scan folders, ISOs or CUE files and add the albums to the queue
    /// (the Mac app's `--add`).
    Add {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// List the albums in the queue.
    List,
    /// Identify the albums in the queue (MusicBrainz, AcoustID, Discogs),
    /// with the keys stored by `set-key`. With paths, only albums under them.
    Identify {
        paths: Vec<PathBuf>,
        /// Batches over a tagged library: a release ID in the tags that fits
        /// the files, or a barcode naming one fitting edition, decides it.
        #[arg(long)]
        fast: bool,
        /// Print every step of the identification.
        #[arg(long, short)]
        verbose: bool,
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },
    /// Preview and write tags for every album in the queue with a chosen
    /// (or confidently matched) release: ReplayGain, cover, backups of the
    /// originals and, with a library root, the library layout.
    Tag {
        paths: Vec<PathBuf>,
        /// Only print what would change.
        #[arg(long)]
        dry_run: bool,
        /// Move the files into this library after writing.
        #[arg(long)]
        library_root: Option<PathBuf>,
        /// Do not measure loudness (no ReplayGain tags).
        #[arg(long)]
        no_replaygain: bool,
        /// Do not look for, embed or save a cover.
        #[arg(long)]
        no_artwork: bool,
    },
    /// Put back the original tags of the albums drtagger wrote.
    Restore { paths: Vec<PathBuf> },
    /// Process the whole queue unattended: identify every album; those with
    /// a sure match get their files, tags, cover and ReplayGain, the rest
    /// are set aside for review. Prints every album's outcome.
    Batch {
        /// import (tag, then move into the library) or update (tag in place).
        #[arg(default_value = "import")]
        mode: String,
        /// Identify and report what would change; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Continue an interrupted batch instead of starting over.
        #[arg(long)]
        resume: bool,
        /// The library folder (needed to import, extract and split).
        #[arg(long)]
        library_root: Option<PathBuf>,
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },
    /// Forget the batch statuses; the albums stay in the queue.
    BatchClear,
    /// Ask CUETools DB about the CUE albums in the queue (and check the
    /// split tracks' CRCs against every known rip).
    Ctdb {
        paths: Vec<PathBuf>,
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },
    /// Serve CDDB on 127.0.0.1 for rippers (server localhost, path
    /// /~cddb/cddb.cgi), answered from MusicBrainz.
    CddbServer {
        #[arg(long, default_value_t = drtagger_core::cddb_server::DEFAULT_PORT)]
        port: u16,
    },
    /// Split the CUE image albums in the queue into verified FLAC tracks
    /// (the Mac app's `--split-into`). With paths, only albums under them.
    Split {
        /// Library root the tracks go to.
        #[arg(long)]
        into: PathBuf,
        paths: Vec<PathBuf>,
        #[arg(long, default_value = splitkit::DEFAULT_ALBUM_FOLDER)]
        album_template: String,
        #[arg(long, default_value = splitkit::DEFAULT_TRACK_FILE)]
        track_template: String,
        /// Keep accents and other scripts in file names.
        #[arg(long)]
        no_ascii: bool,
        /// Move the images and CUEs to the trash after a verified split.
        #[arg(long)]
        trash_originals: bool,
        /// ffmpeg to use instead of the bundled one.
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },
    /// Extract the SACD ISOs in the queue into DSF files (the Mac app's
    /// `--extract-into`). With paths, only ISOs under them.
    Extract {
        /// Library root the tracks go to.
        #[arg(long)]
        into: PathBuf,
        paths: Vec<PathBuf>,
        /// Also extract the multichannel area when there is one.
        #[arg(long)]
        multichannel: bool,
        /// Drop the audio between a track's end and the next track
        /// (sacd_extract) instead of keeping it in the previous track.
        #[arg(long)]
        drop_pauses: bool,
        #[arg(long, default_value = splitkit::DEFAULT_ALBUM_FOLDER)]
        album_template: String,
        #[arg(long, default_value = splitkit::DEFAULT_TRACK_FILE)]
        track_template: String,
        #[arg(long)]
        no_ascii: bool,
        /// Move the ISO to the trash after a verified extraction.
        #[arg(long)]
        trash_originals: bool,
    },
    /// Measure EBU R128 loudness and print the ReplayGain tags drtagger
    /// would write. Files given together are one album.
    Loudness {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },
    /// Show which ffmpeg drtagger would use.
    Ffmpeg {
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },
    /// Remove albums from the queue by path (all of them with --all).
    Remove {
        paths: Vec<PathBuf>,
        #[arg(long)]
        all: bool,
    },
    /// Print the parity JSON (scan, CUE sheets, folder names) that
    /// parity/compare.py diffs against the Mac implementation.
    #[command(hide = true)]
    Parity {
        #[arg(required = true)]
        roots: Vec<PathBuf>,
        /// A file with one folder name per line for the name parser.
        #[arg(long)]
        names: Option<PathBuf>,
    },
    /// Print the tag parity JSON (read, write, keep, strip, restore) that
    /// parity/compare.py diffs against `MacDump tags`.
    #[command(hide = true)]
    ParityTags {
        #[arg(long)]
        cover: PathBuf,
        /// Read tags and pictures only: no audio hashing, no writes.
        #[arg(long)]
        preview: bool,
        #[arg(required = true)]
        roots: Vec<PathBuf>,
    },
    /// Print the loudness parity JSON that parity/compare.py diffs against
    /// `MacDump loudness`.
    #[command(hide = true)]
    ParityLoudness {
        #[arg(long)]
        ffmpeg: PathBuf,
        #[arg(required = true)]
        folders: Vec<PathBuf>,
    },
    /// Print the identification parity JSON that parity/compare_identify.py
    /// diffs against `MacDump identify`, answered from its recordings.
    #[command(hide = true)]
    ParityIdentify {
        #[arg(long)]
        ffmpeg: PathBuf,
        /// MacDump's recordings.
        #[arg(long)]
        replay: PathBuf,
        /// Requests without a recording go to the network (and are recorded).
        #[arg(long)]
        fallback_live: bool,
        #[arg(required = true)]
        roots: Vec<PathBuf>,
    },
    /// Print the artwork scan benchmark JSON that parity/compare_vision.py
    /// diffs against `MacDump vision-scan`.
    #[command(hide = true)]
    ParityVisionScan {
        /// Paths in the dump are relative to this folder.
        #[arg(long)]
        root: PathBuf,
        #[arg(long, default_value_t = 12)]
        max_files: usize,
        #[arg(long, default_value_t = 3)]
        threads: usize,
        /// Only print the image list ("album<TAB>file") MacDump reads.
        #[arg(long)]
        list: bool,
        #[arg(required = true)]
        roots: Vec<PathBuf>,
    },
    /// Print cover feature-print distances for the pairs listed in a file
    /// (lines "a<TAB>b" relative to --root), as `MacDump vision-pairs`.
    #[command(hide = true)]
    ParityVisionPairs {
        #[arg(long)]
        root: PathBuf,
        pairs: PathBuf,
    },
    /// Store an identification result (the Mac app's JSON) and the chosen
    /// candidate on a queued album, so tagging parity starts from the same
    /// identification as the Mac run.
    #[command(hide = true)]
    ParitySetIdentification {
        album: PathBuf,
        identification: PathBuf,
        #[arg(long)]
        selected: Option<String>,
    },
    /// Print the SACD parity JSON that parity/compare.py diffs against
    /// `MacDump sacd`.
    #[command(hide = true)]
    ParitySacd {
        #[arg(long)]
        into: PathBuf,
        #[arg(required = true)]
        isos: Vec<PathBuf>,
    },
    /// Print the split parity JSON that parity/compare.py diffs against
    /// `MacDump split`.
    #[command(hide = true)]
    ParitySplit {
        /// Folder holding ffmpeg and ffprobe.
        #[arg(long)]
        ffmpeg: PathBuf,
        #[arg(long)]
        into: PathBuf,
        #[arg(required = true)]
        roots: Vec<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Status => status(),
        Command::SetKey { provider } => set_key(&provider),
        Command::Add { paths } => add(&paths),
        Command::List => list(),
        Command::Split { into, paths, album_template, track_template, no_ascii, trash_originals, ffmpeg } => {
            let options = splitkit::SplitOptions {
                album_folder_template: album_template,
                track_file_template: track_template,
                ascii_file_names: !no_ascii,
                overwrite_existing: true,
                ..Default::default()
            };
            split(&into, &paths, &options, trash_originals, ffmpeg.as_deref())
        }
        Command::Ffmpeg { ffmpeg } => ffmpeg_info(ffmpeg.as_deref()),
        Command::Identify { paths, fast, verbose, ffmpeg } => identify(&paths, fast, verbose, ffmpeg.as_deref()),
        Command::Tag { paths, dry_run, library_root, no_replaygain, no_artwork } => {
            tag(&paths, dry_run, library_root, no_replaygain, no_artwork)
        }
        Command::Restore { paths } => restore(&paths),
        Command::Batch { mode, dry_run, resume, library_root, ffmpeg } => {
            batch(&mode, dry_run, resume, library_root, ffmpeg.as_deref())
        }
        Command::BatchClear => drtagger_core::batch::clear(&Store::open_default()?),
        Command::Ctdb { paths, ffmpeg } => ctdb(&paths, ffmpeg.as_deref()),
        Command::CddbServer { port } => {
            let server = drtagger_core::cddb_server::CddbServer::start(port)?;
            println!("CDDB server on http://127.0.0.1:{}/~cddb/cddb.cgi (Ctrl-C to stop)", server.port);
            loop {
                std::thread::park();
            }
        }
        Command::Extract {
            into,
            paths,
            multichannel,
            drop_pauses,
            album_template,
            track_template,
            no_ascii,
            trash_originals,
        } => {
            let options = sacdkit::SACDExtractOptions {
                pause_policy: if drop_pauses {
                    sacdkit::PausePolicy::Drop
                } else {
                    sacdkit::PausePolicy::AppendToPrevious
                },
                album_folder_template: album_template,
                track_file_template: track_template,
                ascii_file_names: !no_ascii,
                overwrite_existing: true,
                ..Default::default()
            };
            extract(&into, &paths, multichannel, &options, trash_originals)
        }
        Command::ParityIdentify { ffmpeg, replay, fallback_live, roots } => {
            let mut out = std::io::stdout().lock();
            serde_json::to_writer(&mut out, &parity_identify::dump(&roots, &ffmpeg, &replay, fallback_live))?;
            writeln!(out)?;
            Ok(())
        }
        Command::ParityVisionScan { root, max_files, threads, list, roots } => {
            let mut out = std::io::stdout().lock();
            match parity_vision::scan(&root, &roots, max_files, threads, list) {
                serde_json::Value::String(tsv) => write!(out, "{tsv}")?,
                dump => serde_json::to_writer(&mut out, &dump)?,
            }
            writeln!(out)?;
            Ok(())
        }
        Command::ParityVisionPairs { root, pairs } => {
            let mut out = std::io::stdout().lock();
            serde_json::to_writer(&mut out, &parity_vision::pairs(&root, &pairs)?)?;
            writeln!(out)?;
            Ok(())
        }
        Command::ParitySetIdentification { album, identification, selected } => {
            let store = Store::open_default()?;
            let mut record = store
                .fetch(&librarykit::standardized(&album))?
                .ok_or_else(|| anyhow::anyhow!("{} is not in the queue", album.display()))?;
            let result: identifykit::IdentificationResult = serde_json::from_slice(&std::fs::read(&identification)?)?;
            record.set_identification(Some(&result));
            record.selected_candidate_id = selected.filter(|s| !s.is_empty());
            record.state = drtagger_core::store::AlbumState::NeedsReview;
            store.save(&record)?;
            Ok(())
        }
        Command::ParityLoudness { ffmpeg, folders } => {
            let mut out = std::io::stdout().lock();
            serde_json::to_writer(&mut out, &parity_loudness::dump(&folders, &ffmpeg))?;
            writeln!(out)?;
            Ok(())
        }
        Command::Loudness { files, ffmpeg } => loudness(&files, ffmpeg.as_deref()),
        Command::ParitySacd { into, isos } => {
            let mut out = std::io::stdout().lock();
            serde_json::to_writer(&mut out, &parity_sacd::dump(&isos, &into))?;
            writeln!(out)?;
            Ok(())
        }
        Command::Remove { paths, all } => remove(&paths, all),
        Command::Parity { roots, names } => {
            let names = match names {
                Some(p) => std::fs::read_to_string(p)?.lines().filter(|l| !l.is_empty()).map(str::to_string).collect(),
                None => vec![],
            };
            let mut out = std::io::stdout().lock();
            serde_json::to_writer(&mut out, &parity::dump(&roots, &names))?;
            writeln!(out)?;
            Ok(())
        }
        Command::ParitySplit { ffmpeg, into, roots } => {
            let mut out = std::io::stdout().lock();
            serde_json::to_writer(&mut out, &parity_split::dump(&roots, &ffmpeg, &into))?;
            writeln!(out)?;
            Ok(())
        }
        Command::ParityTags { cover, preview, roots } => {
            let mut out = std::io::stdout().lock();
            serde_json::to_writer(&mut out, &parity_tags::dump(&roots, &cover, preview))?;
            writeln!(out)?;
            Ok(())
        }
    }
}

fn status() -> Result<()> {
    let rt = drtagger_core::runtime();
    println!("drtagger {}", drtagger_core::VERSION);
    println!("data:  {}", drtagger_core::paths::data_dir().display());
    println!("cache: {}", drtagger_core::paths::cache_dir().display());
    for p in Provider::ALL {
        let state = match rt.block_on(keys::get(p)) {
            Ok(Some(_)) => "set".to_string(),
            Ok(None) => "not set".to_string(),
            Err(e) => format!("keyring error: {e:#}"),
        };
        println!("{:<24} {state}", p.label());
    }
    Ok(())
}

fn set_key(provider: &str) -> Result<()> {
    let Some(p) = Provider::from_name(provider) else {
        bail!("unknown provider {provider:?} (acoustid, discogs, fanart)");
    };
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    drtagger_core::runtime().block_on(keys::set(p, &value))?;
    println!("{}: {}", p.label(), if value.trim().is_empty() { "deleted" } else { "stored" });
    Ok(())
}

fn add(paths: &[PathBuf]) -> Result<()> {
    let roots: Vec<PathBuf> = paths.iter().map(std::path::absolute).collect::<Result<_, _>>()?;
    for root in &roots {
        if librarykit::volume::is_network_volume(root) {
            eprintln!("Reading from a network volume…");
            break;
        }
    }
    let tty = std::io::stderr().is_terminal();
    let result = LibraryScanner::new().scan_with_progress(&roots, &mut |folder| {
        if tty {
            let name = folder.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
            eprint!("\r\x1b[2KScanning {name}");
        }
    });
    if tty {
        eprint!("\r\x1b[2K");
    }
    let mut store = Store::open_default()?;
    let summary = store.upsert(&result)?;
    drtagger_core::sets::after_scan(&store, &result)?;
    for issue in &result.issues {
        eprintln!("{}: {}", issue.url.display(), issue.message);
    }
    for album in &result.albums {
        println!(
            "{:<12} {:>3} tracks  {}",
            album.kind.raw_value(),
            album.track_count(),
            album.title_hint().unwrap_or_else(|| album.folder_name())
        );
    }
    println!("{summary}");
    Ok(())
}

fn list() -> Result<()> {
    let store = Store::open_default()?;
    for r in store.all()? {
        let artist = r.artist_hint.as_deref().unwrap_or("");
        let year = r.year_hint.as_deref().map(|y| format!(" ({y})")).unwrap_or_default();
        println!(
            "{:<18} {:<12} {:>3}  {}{}{}  {}",
            r.state.label(),
            r.kind.raw_value(),
            r.track_count,
            if artist.is_empty() { String::new() } else { format!("{artist} – ") },
            r.display_title,
            year,
            r.path.display()
        );
    }
    Ok(())
}

fn locate(override_path: Option<&std::path::Path>) -> Result<drtagger_core::ffmpeg::Location> {
    let o = override_path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    drtagger_core::ffmpeg::locate(&o).ok_or_else(|| anyhow::anyhow!(drtagger_core::ffmpeg::NOT_FOUND))
}

fn ffmpeg_info(override_path: Option<&std::path::Path>) -> Result<()> {
    let location = locate(override_path)?;
    let info = drtagger_core::ffmpeg::inspect(&location).map_err(anyhow::Error::msg)?;
    println!("{:?}: {}", location.source, location.ffmpeg.display());
    println!("ffprobe: {}", location.ffprobe.as_deref().map_or("(from PATH)".into(), |p| p.display().to_string()));
    println!("{}", info.version_line);
    println!("GPL build: {}   DST decoder: {}", info.is_gpl, info.has_dst_decoder);
    Ok(())
}

/// Records under `paths` (every record when there are none).
fn records_under(store: &Store, paths: &[PathBuf]) -> Result<Vec<drtagger_core::store::AlbumRecord>> {
    let filters: Vec<PathBuf> =
        paths.iter().map(|p| std::path::absolute(p).map(|a| librarykit::standardized(&a))).collect::<Result<_, _>>()?;
    Ok(store
        .all()?
        .into_iter()
        .filter(|r| filters.is_empty() || filters.iter().any(|f| r.path.starts_with(f)))
        .collect())
}

fn batch(
    mode: &str,
    dry_run: bool,
    resume: bool,
    library_root: Option<PathBuf>,
    ffmpeg: Option<&std::path::Path>,
) -> Result<()> {
    use drtagger_core::batch::{BatchRunner, Mode, queue_order, reset_interrupted};
    let mode = match mode {
        "import" => Mode::ImportToLibrary,
        "update" => Mode::UpdateInPlace,
        other => bail!("unknown batch mode {other:?} (import or update)"),
    };
    let settings = drtagger_core::settings::Settings {
        keys: drtagger_core::identify::ProviderKeys::load(),
        library_root,
        ffmpeg_override_path: ffmpeg.map(|p| p.display().to_string()).unwrap_or_default(),
        ..Default::default()
    };
    if settings.library_root.is_none() {
        bail!("a batch needs --library-root");
    }
    let tool = drtagger_core::ffmpeg::locate(&settings.ffmpeg_override_path)
        .filter(|l| l.ffprobe.is_some())
        .map(|l| drtagger_core::ffmpeg::tool(&l));
    let store = Store::open_default()?;
    reset_interrupted(&store)?;
    let runner = BatchRunner::new(
        std::sync::Arc::new(drtagger_core::library_index::LibraryIndex::load()),
        std::sync::Arc::new(drtagger_core::tagging::Tagger::new()),
    );
    let handle = if resume {
        runner.resume(&store, mode, dry_run, settings, tool)
    } else {
        runner.start(&store, mode, dry_run, settings, tool)?
    };
    if let Some(h) = handle {
        let _ = h.join();
    }
    let mut all = store.all()?;
    all.sort_by(queue_order);
    for r in all {
        let status = r.batch_status.as_deref().unwrap_or("none");
        let note = r.batch_note.as_deref().map(|n| format!(" – {n}")).unwrap_or_default();
        println!("batch: {}: {status}{note}", r.display_title);
    }
    println!("batch: finished");
    Ok(())
}

fn tag(
    paths: &[PathBuf],
    dry_run: bool,
    library_root: Option<PathBuf>,
    no_replaygain: bool,
    no_artwork: bool,
) -> Result<()> {
    use drtagger_core::tagging::{Tagger, chosen_candidate};
    let mut settings = drtagger_core::settings::Settings {
        keys: drtagger_core::identify::ProviderKeys::load(),
        library_root,
        compute_replay_gain: !no_replaygain,
        embed_front_cover: !no_artwork,
        save_cover_file: !no_artwork,
        ..Default::default()
    };
    settings.organize_after_apply = settings.library_root.is_some();
    let store = Store::open_default()?;
    let tagger = Tagger::new();
    let tty = std::io::stderr().is_terminal();
    let activity = |a: drtagger_core::tagging::Activity| {
        if tty {
            eprint!(
                "\r\x1b[2K  {}{}",
                a.fraction.map(|f| format!("{:3.0}% ", f * 100.0)).unwrap_or_default(),
                a.message
            );
        }
    };
    for record in records_under(&store, paths)? {
        if chosen_candidate(&record).is_none() {
            continue;
        }
        println!("{}", record.display_title);
        let mut members = [record];
        let plan = tagger.build_plan(&members, &settings, false, &activity);
        if tty {
            eprint!("\r\x1b[2K");
        }
        let plan = match plan {
            Ok(p) => p,
            Err(e) => {
                println!("  no plan: {e}");
                continue;
            }
        };
        for w in &plan.warnings {
            println!("  ! {w}");
        }
        for c in plan.album_changes().iter().filter(|c| c.kind() != tagkit::ChangeKind::Unchanged) {
            println!("  {:<28} {} → {}", c.name, c.old.join("; "), c.new.join("; "));
        }
        for t in &plan.tracks {
            let own: Vec<_> =
                plan.track_only_changes(t).into_iter().filter(|c| c.kind() != tagkit::ChangeKind::Unchanged).collect();
            if !own.is_empty() {
                println!("  {}", t.path.file_name().unwrap_or_default().to_string_lossy());
                for c in own {
                    println!("    {:<26} {} → {}", c.name, c.old.join("; "), c.new.join("; "));
                }
            }
        }
        if let Some(cover) = &plan.cover {
            println!("  cover: {} ({}×{})", cover.option.label, cover.width, cover.height);
        }
        if dry_run {
            continue;
        }
        if !plan.has_work() {
            println!("  nothing to write");
            continue;
        }
        let outcome = tagger.apply(&store, &mut members, &plan, &settings, None, &activity);
        if tty {
            eprint!("\r\x1b[2K");
        }
        let r = &members[0];
        println!(
            "  {} file(s) written, state {}{}",
            r.tag_reports().len(),
            r.state.raw_value(),
            r.error_message.as_ref().map(|e| format!(" – {e}")).unwrap_or_default()
        );
        if !outcome.problems.is_empty() {
            for p in &outcome.problems {
                println!("  ! {p}");
            }
        }
    }
    Ok(())
}

fn restore(paths: &[PathBuf]) -> Result<()> {
    let store = Store::open_default()?;
    let tagger = drtagger_core::tagging::Tagger::new();
    for record in records_under(&store, paths)? {
        if record.tag_backups().is_empty() {
            continue;
        }
        let title = record.display_title.clone();
        let mut members = [record];
        match tagger.restore(&store, &mut members) {
            Ok(()) => println!("{title}: restored"),
            Err(e) => println!("{title}: failed – {e}"),
        }
    }
    Ok(())
}

fn identify(paths: &[PathBuf], fast: bool, verbose: bool, ffmpeg: Option<&std::path::Path>) -> Result<()> {
    use drtagger_core::identify::ProviderKeys;
    let tool = drtagger_core::ffmpeg::tool(&locate(ffmpeg)?);
    let keys = ProviderKeys::load();
    if keys.acoust_id.is_none() {
        eprintln!("No AcoustID key: fingerprints are skipped (drtagger-cli set-key acoustid).");
    }
    let store = Store::open_default()?;
    let tty = std::io::stderr().is_terminal();
    for mut record in records_under(&store, paths)? {
        println!("{}", record.display_title);
        // The disc ID, as the inspector computes it when it shows a CUE disc.
        if record.has_cue_sheet() && record.toc().is_none() {
            match drtagger_core::disc::compute_toc(&record, &tool) {
                Ok(toc) => {
                    record.set_toc(toc.as_ref());
                    store.save(&record)?;
                }
                Err(e) => println!("  no disc TOC: {e:#}"),
            }
        }
        let mut members = [record];
        let result = drtagger_core::identify::identify(&store, &mut members, &[1], &tool, &keys, fast, &|p| {
            if tty {
                eprint!("\r\x1b[2K  {:3.0}% {}", p.fraction * 100.0, p.message);
            }
        });
        if tty {
            eprint!("\r\x1b[2K");
        }
        match result {
            Err(e) => println!("  error: {e:#}"),
            Ok(r) => {
                if verbose {
                    for line in &r.log {
                        println!("  · {line}");
                    }
                }
                for (i, c) in r.candidates.iter().take(5).enumerate() {
                    let cand = &c.candidate;
                    println!(
                        "  {} {:>5.1} {:<9} {} – {} [{}]",
                        if i == 0 { "→" } else { " " },
                        c.score,
                        c.confidence.raw(),
                        cand.artist,
                        cand.title,
                        [
                            cand.year.clone(),
                            cand.country.clone(),
                            cand.media_format.clone(),
                            cand.catalog_number.clone()
                        ]
                        .into_iter()
                        .flatten()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ")
                    );
                }
                if r.candidates.is_empty() {
                    println!("  no candidates");
                }
                println!("  {}", members[0].state.label());
            }
        }
    }
    Ok(())
}

fn ctdb(paths: &[PathBuf], ffmpeg: Option<&std::path::Path>) -> Result<()> {
    let tool = drtagger_core::ffmpeg::tool(&locate(ffmpeg)?);
    let store = Store::open_default()?;
    for mut record in records_under(&store, paths)?.into_iter().filter(|r| r.has_cue_sheet()) {
        println!("{}", record.display_title);
        if let Err(e) = drtagger_core::identify::check_ctdb(&store, &mut record, &tool) {
            println!("  error: {e:#}");
            continue;
        }
        let Some(report) = record.ctdb() else {
            println!("  no TOC");
            continue;
        };
        if !report.is_known_disc() {
            println!("  unknown to CUETools DB");
            continue;
        }
        println!(
            "  {} rip(s), confidence {} (best {}), {} metadata entr(ies)",
            report.entry_count,
            report.total_confidence,
            report.best_confidence,
            report.metadata.len()
        );
        if let Some(v) = &report.verification {
            println!(
                "  {}/{} tracks match{}",
                v.matched_track_count(),
                v.tracks.len(),
                if v.disc_matches_best_entry { ", disc CRC matches the best rip" } else { "" }
            );
        }
    }
    Ok(())
}

fn split(
    into: &std::path::Path,
    paths: &[PathBuf],
    options: &splitkit::SplitOptions,
    trash_originals: bool,
    ffmpeg: Option<&std::path::Path>,
) -> Result<()> {
    let tool = drtagger_core::ffmpeg::tool(&locate(ffmpeg)?);
    let into = std::path::absolute(into)?;
    let filters: Vec<PathBuf> =
        paths.iter().map(|p| std::path::absolute(p).map(|a| librarykit::standardized(&a))).collect::<Result<_, _>>()?;
    let store = Store::open_default()?;
    let tty = std::io::stderr().is_terminal();
    for mut record in store.all()? {
        if !record.is_splittable() || !(filters.is_empty() || filters.iter().any(|f| record.path.starts_with(f))) {
            continue;
        }
        println!("{}", record.display_title);
        let result =
            drtagger_core::disc::split(&store, &mut record, &into, &tool, options, trash_originals, &mut |m, f| {
                if tty {
                    eprint!("\r\x1b[2K  {:3.0}% {m}", f * 100.0);
                }
            });
        if tty {
            eprint!("\r\x1b[2K");
        }
        match result {
            Ok(outcomes) => {
                for o in outcomes {
                    let crc = o.ctdb_disc_crc32.map(|c| format!("  CTDB disc CRC {c:08x}")).unwrap_or_default();
                    println!(
                        "  {} tracks → {}  {}{crc}  ({:.1} s)",
                        o.tracks.len(),
                        o.output_folder.display(),
                        if o.verified { "verified" } else { "NOT VERIFIED" },
                        o.elapsed_seconds
                    );
                    for w in &o.warnings {
                        println!("  warning: {w}");
                    }
                }
            }
            Err(e) => println!("  error: {e:#}"),
        }
    }
    Ok(())
}

fn extract(
    into: &std::path::Path,
    paths: &[PathBuf],
    multichannel: bool,
    options: &sacdkit::SACDExtractOptions,
    trash_originals: bool,
) -> Result<()> {
    let into = std::path::absolute(into)?;
    let filters: Vec<PathBuf> =
        paths.iter().map(|p| std::path::absolute(p).map(|a| librarykit::standardized(&a))).collect::<Result<_, _>>()?;
    let store = Store::open_default()?;
    let tty = std::io::stderr().is_terminal();
    for mut record in store.all()? {
        if record.kind != librarykit::AlbumKind::SacdIso
            || !(filters.is_empty() || filters.iter().any(|f| record.path.starts_with(f)))
        {
            continue;
        }
        println!("{}", record.display_title);
        let result = drtagger_core::disc::extract_sacd(
            &store,
            &mut record,
            &into,
            options,
            drtagger_core::disc::SacdJob { multichannel, trash_originals },
            None,
            &mut |m, f| {
                if tty {
                    eprint!("\r\x1b[2K  {:3.0}% {m}", f * 100.0);
                }
            },
        );
        if tty {
            eprint!("\r\x1b[2K");
        }
        match result {
            Ok(outcomes) => {
                for o in outcomes {
                    println!(
                        "  {} ch, {} tracks → {}  {}  ({:.1} s)",
                        o.channel_count,
                        o.tracks.len(),
                        o.output_folder.display(),
                        if o.verified() { "verified" } else { "NOT VERIFIED" },
                        o.elapsed_seconds
                    );
                    for w in &o.warnings {
                        println!("  warning: {w}");
                    }
                }
            }
            Err(e) => println!("  error: {e:#}"),
        }
    }
    Ok(())
}

fn loudness(files: &[PathBuf], ffmpeg: Option<&std::path::Path>) -> Result<()> {
    let tool = drtagger_core::ffmpeg::tool(&locate(ffmpeg)?);
    let album = loudnesskit::LoudnessAnalyzer::new(tool).analyze_album(files, &|done, total| {
        if std::io::stderr().is_terminal() {
            eprint!("\r\x1b[2K  {done}/{total}");
        }
    })?;
    if std::io::stderr().is_terminal() {
        eprint!("\r\x1b[2K");
    }
    let lufs = |v: Option<f64>| v.map_or("silent".into(), |l| format!("{l:.2} LUFS"));
    for (f, t) in files.iter().zip(&album.tracks) {
        println!("{}", f.display());
        println!("  {}  true peak {:.2} dBTP  {:.1} s", lufs(t.integrated_lufs), t.true_peak_dbtp, t.duration_seconds);
        let dsd = f.extension().is_some_and(|e| e.eq_ignore_ascii_case("dsf") || e.eq_ignore_ascii_case("dff"));
        for (k, v) in loudnesskit::replaygain::tags(t, Some(&album), dsd) {
            println!("  {k}={v}");
        }
    }
    println!("album: {}  true peak {:.2} dBTP", lufs(album.integrated_lufs), album.true_peak_dbtp);
    Ok(())
}

fn remove(paths: &[PathBuf], all: bool) -> Result<()> {
    let store = Store::open_default()?;
    if all {
        store.clear()?;
        println!("queue cleared");
        return Ok(());
    }
    for p in paths {
        let p = librarykit::standardized(&std::path::absolute(p)?);
        println!("{}: {}", p.display(), if store.remove(&p)? { "removed" } else { "not in the queue" });
    }
    Ok(())
}
