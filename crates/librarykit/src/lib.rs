//! Album discovery: walks folders, recognises SACD ISOs, CD images with CUE
//! sheets, multi-file CUEs and plain track folders (single or multi-disc),
//! and parses CUE sheets with encoding detection. Port of the Mac app's
//! LibraryKit; behaviour and JSON shape follow the Swift code.

mod audio_format;
mod cue;
mod disc_toc;
mod folder_name;
mod natural;
pub mod path_serde;
mod sacd_probe;
mod scanner;
mod split_plan;
mod text_decoder;
pub mod volume;

pub use audio_format::{AudioFormat, FileRules};
pub use cue::{CueFile, CueIndex, CueParseError, CueRem, CueSheet, CueTime, CueTrack};
pub use disc_toc::{DiscTOC, TocError};
pub use folder_name::{FolderNameParser, ParsedName, SourceHint};
pub use natural::{natural_cmp, naturally_sorted};
pub use sacd_probe::{FrameFormat, SACDInfo, SACDProbe, SacdArea, SacdProbeError};
pub use scanner::{
    AlbumKind, DetectedAlbum, DetectedDisc, DetectedTrackFile, LibraryScanner, ScanIssue, ScanResult, resolve_on_disk,
    standardized,
};
pub use split_plan::{CueSplitPlan, PlanError, SplitTrack};
pub use text_decoder::{Decoded, TextDecoder};
