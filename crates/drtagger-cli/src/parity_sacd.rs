//! SACD parity dump: the same JSON as `MacDump sacd` (parity/MacDump/
//! Sources/MacDump/Sacd.swift). Each disc is read; the stereo area is
//! extracted with both pause policies and the multichannel area with the
//! default one; every DSF is hashed, then deleted.

use sacdkit::{PausePolicy, SACDExtractOptions, SACDExtractor};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn dump(isos: &[PathBuf], into: &Path) -> Value {
    splitkit::provisional_tags::set_vendor("drtagger for Mac");
    let root = librarykit::standardized(&std::path::absolute(into).unwrap());
    let mut out = vec![];
    for iso in isos {
        let mut entry = json!({ "iso": iso.file_name().unwrap().to_string_lossy() });
        match sacdkit::read(iso) {
            Err(e) => entry["error"] = e.to_string().into(),
            Ok(disc) => {
                entry["areas"] = serde_json::to_value(&disc.areas).unwrap();
                let mut runs = vec![];
                for area in &disc.areas {
                    let policies: &[PausePolicy] = if area.is_multichannel {
                        &[PausePolicy::AppendToPrevious]
                    } else {
                        &[PausePolicy::AppendToPrevious, PausePolicy::Drop]
                    };
                    for policy in policies {
                        let options = SACDExtractOptions {
                            pause_policy: *policy,
                            overwrite_existing: true,
                            ..Default::default()
                        };
                        let mut run = json!({ "multichannel": area.is_multichannel, "policy": policy });
                        match SACDExtractor::extract(&disc, area, &root, &options, None, &mut |_| {}) {
                            Err(e) => run["error"] = e.to_string().into(),
                            Ok(o) => {
                                run["outputFolder"] = librarykit::standardized(&o.output_folder)
                                    .strip_prefix(&root)
                                    .unwrap()
                                    .to_string_lossy()
                                    .into_owned()
                                    .into();
                                run["droppedLeadIn"] = o.dropped_lead_in_frames.into();
                                run["droppedPause"] = o.dropped_pause_frames.into();
                                run["verified"] = o.verified().into();
                                run["warnings"] = o.warnings.clone().into();
                                run["tracks"] = o
                                    .tracks
                                    .iter()
                                    .map(|t| {
                                        let sha = std::fs::read(&t.url)
                                            .map_or_else(|_| "unreadable".into(), |d| hex::encode(Sha256::digest(d)));
                                        let _ = std::fs::remove_file(&t.url);
                                        json!({
                                            "number": t.number, "file": t.url.file_name().unwrap().to_string_lossy(),
                                            "frames": t.frames, "expectedFrames": t.expected_frames,
                                            "sampleCount": t.sample_count, "fileSize": t.file_size,
                                            "contiguous": t.contiguous, "sha": sha,
                                        })
                                    })
                                    .collect::<Vec<_>>()
                                    .into();
                            }
                        }
                        runs.push(run);
                    }
                }
                entry["runs"] = runs.into();
            }
        }
        out.push(entry);
    }
    json!({ "discs": out })
}
