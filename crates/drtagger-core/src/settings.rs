//! The user's configuration as the services read it (the Mac app's
//! AppSettings). The GTK app fills it from GSettings and the keyring; the
//! CLI from the keyring, its flags and these defaults (the schema's).

use crate::identify::ProviderKeys;
use sacdkit::PausePolicy;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Settings {
    /// Destination library; None when not configured.
    pub library_root: Option<PathBuf>,
    pub album_folder_template: String,
    pub track_file_template: String,
    pub ascii_file_names: bool,
    pub organize_after_apply: bool,
    pub move_originals_to_trash: bool,
    pub keep_queue_between_launches: bool,
    pub extract_multichannel: bool,
    pub sacd_pause_policy: PausePolicy,
    pub embed_front_cover: bool,
    /// Longest side of the embedded front cover.
    pub embed_cover_max_pixels: u32,
    pub save_cover_file: bool,
    pub write_genres_from_discogs: bool,
    pub compute_replay_gain: bool,
    /// ffmpeg used instead of the bundled one; empty: the bundled one.
    pub ffmpeg_override_path: String,
    pub cddb_server_enabled: bool,
    pub cddb_port: u16,
    pub keys: ProviderKeys,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            library_root: None,
            album_folder_template: splitkit::DEFAULT_ALBUM_FOLDER.into(),
            track_file_template: splitkit::DEFAULT_TRACK_FILE.into(),
            ascii_file_names: true,
            organize_after_apply: true,
            move_originals_to_trash: false,
            keep_queue_between_launches: false,
            extract_multichannel: false,
            sacd_pause_policy: PausePolicy::AppendToPrevious,
            embed_front_cover: true,
            embed_cover_max_pixels: 1500,
            save_cover_file: true,
            write_genres_from_discogs: true,
            compute_replay_gain: true,
            ffmpeg_override_path: String::new(),
            cddb_server_enabled: false,
            cddb_port: crate::cddb_server::DEFAULT_PORT,
            keys: ProviderKeys::default(),
        }
    }
}

impl Settings {
    pub fn discogs_token(&self) -> Option<&str> {
        self.keys.discogs.as_deref().map(str::trim).filter(|t| !t.is_empty())
    }

    pub fn fanart_key(&self) -> Option<&str> {
        self.keys.fanart.as_deref().map(str::trim).filter(|t| !t.is_empty())
    }

    pub fn split_options(&self, overwrite: bool) -> splitkit::SplitOptions {
        splitkit::SplitOptions {
            overwrite_existing: overwrite,
            album_folder_template: self.album_folder_template.clone(),
            track_file_template: self.track_file_template.clone(),
            ascii_file_names: self.ascii_file_names,
            ..Default::default()
        }
    }

    pub fn extract_options(&self, overwrite: bool) -> sacdkit::SACDExtractOptions {
        sacdkit::SACDExtractOptions {
            overwrite_existing: overwrite,
            pause_policy: self.sacd_pause_policy,
            album_folder_template: self.album_folder_template.clone(),
            track_file_template: self.track_file_template.clone(),
            ascii_file_names: self.ascii_file_names,
            ..Default::default()
        }
    }

    pub fn organizer_options(&self) -> tagkit::OrganizerOptions {
        tagkit::OrganizerOptions {
            album_folder_template: self.album_folder_template.clone(),
            track_file_template: self.track_file_template.clone(),
            ascii_file_names: self.ascii_file_names,
            ..Default::default()
        }
    }
}
