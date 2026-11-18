//! Which ffmpeg/ffprobe the app drives. Search order:
//!   1. an explicit override path (Preferences > Advanced),
//!   2. the LGPL build shipped with the app (next to the executable, as in
//!      the Flatpak's /app/bin) or, in a development checkout, the one
//!      scripts/build-ffmpeg.sh leaves in Vendor/ffmpeg,
//!   3. the system's ffmpeg on PATH (development fallback: distribution
//!      builds are often GPL and encode differently from the bundled 8.0).

use splitkit::FFmpegTool;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Override,
    Bundled,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub ffmpeg: PathBuf,
    pub ffprobe: Option<PathBuf>,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub location: Location,
    /// "ffmpeg version 8.0 Copyright …"
    pub version_line: String,
    pub is_gpl: bool,
    pub has_dst_decoder: bool,
}

pub const NOT_FOUND: &str = "No ffmpeg found. Run scripts/build-ffmpeg.sh or set a path in Preferences > Advanced.";

fn executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn sibling(ffmpeg: &Path) -> Option<PathBuf> {
    let probe = ffmpeg.with_file_name("ffprobe");
    executable(&probe).then_some(probe)
}

fn bundled() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let beside = dir.join("ffmpeg");
    if executable(&beside) {
        return Some(beside);
    }
    dir.ancestors().map(|a| a.join("Vendor/ffmpeg/ffmpeg")).find(|p| executable(p))
}

fn on_path() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join("ffmpeg")).find(|p| executable(p))
}

pub fn locate(override_path: &str) -> Option<Location> {
    let o = override_path.trim();
    if !o.is_empty() {
        let p = PathBuf::from(o);
        if executable(&p) {
            return Some(Location { ffprobe: sibling(&p), ffmpeg: p, source: Source::Override });
        }
    }
    if let Some(p) = bundled() {
        return Some(Location { ffprobe: sibling(&p), ffmpeg: p, source: Source::Bundled });
    }
    on_path().map(|p| Location { ffprobe: sibling(&p), ffmpeg: p, source: Source::System })
}

/// The tool for a location; ffprobe falls back to the one on PATH.
pub fn tool(location: &Location) -> FFmpegTool {
    FFmpegTool::new(location.ffmpeg.clone(), location.ffprobe.clone().unwrap_or_else(|| PathBuf::from("ffprobe")))
}

/// Runs `ffmpeg -version` and `-decoders` and reads the configuration.
pub fn inspect(location: &Location) -> Result<Info, String> {
    let version = Command::new(&location.ffmpeg)
        .args(["-hide_banner", "-version"])
        .output()
        .map_err(|e| format!("{} could not be executed: {e}", location.ffmpeg.display()))?;
    let text = String::from_utf8_lossy(&version.stdout);
    let decoders =
        Command::new(&location.ffmpeg).args(["-hide_banner", "-decoders"]).output().map_err(|e| e.to_string())?;
    let has_dst = String::from_utf8_lossy(&decoders.stdout).lines().any(|l| l.split_whitespace().nth(1) == Some("dst"));
    Ok(Info {
        location: location.clone(),
        version_line: text.lines().next().unwrap_or("").to_string(),
        is_gpl: text.contains("--enable-gpl"),
        has_dst_decoder: has_dst,
    })
}
