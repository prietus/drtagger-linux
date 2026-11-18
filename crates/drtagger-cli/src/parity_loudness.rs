//! Loudness parity dump: the same JSON as `MacDump loudness` (parity/MacDump/
//! Sources/MacDump/Loudness.swift): each folder's audio files measured as
//! one album, with the ReplayGain tags the app would write.

use loudnesskit::{LoudnessAnalyzer, replaygain};
use serde_json::{Value, json};
use splitkit::FFmpegTool;
use std::path::{Path, PathBuf};
use tagkit::TagContainer;

pub fn dump(folders: &[PathBuf], ffmpeg_dir: &Path) -> Value {
    let tool = FFmpegTool::new(ffmpeg_dir.join("ffmpeg"), ffmpeg_dir.join("ffprobe"));
    let mut out = vec![];
    for folder in folders {
        let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
            .map(|e| e.flatten().map(|e| e.path()).collect::<Vec<PathBuf>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|p: &PathBuf| {
                !p.file_name().unwrap().to_string_lossy().starts_with('.') && TagContainer::from_path(p).is_some()
            })
            .collect();
        files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        let mut entry = json!({ "folder": folder.file_name().unwrap().to_string_lossy() });
        match LoudnessAnalyzer::new(tool.clone()).analyze_album(&files, &|_, _| {}) {
            Err(e) => entry["error"] = e.to_string().into(),
            Ok(album) => {
                entry["albumLUFS"] = album.integrated_lufs.into();
                entry["albumPeak"] = album.true_peak_dbtp.into();
                entry["tracks"] = files
                    .iter()
                    .zip(&album.tracks)
                    .map(|(f, t)| {
                        let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                        let dsd = ext == "dsf" || ext == "dff";
                        json!({
                            "file": f.file_name().unwrap().to_string_lossy(), "lufs": t.integrated_lufs,
                            "truePeak": t.true_peak_dbtp, "samplePeak": t.sample_peak_dbfs,
                            "blocks": t.block_powers.len(), "duration": t.duration_seconds,
                            "sampleRate": t.sample_rate, "channels": t.channels,
                            "tags": replaygain::tags(t, Some(&album), dsd)
                                .into_iter().map(|(k, v)| vec![k, v]).collect::<Vec<_>>(),
                        })
                    })
                    .collect::<Vec<_>>()
                    .into();
            }
        }
        out.push(entry);
    }
    json!({ "albums": out })
}
