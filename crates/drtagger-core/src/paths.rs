//! XDG locations. Inside Flatpak these resolve to the sandbox's own
//! `~/.var/app/us.priet.drtagger/...` directories.

use std::path::PathBuf;

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// Persistent data: the album store, backups, the library index.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("drtagger")
}

/// Provider response cache; safe to delete.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("drtagger")
}
