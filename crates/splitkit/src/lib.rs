//! Image splitting through ffmpeg with verification, CUETools DB CRCs,
//! provisional tags from the CUE and path templates. Port of the Mac app's
//! SplitKit; it drives the same ffmpeg 8.0 build (scripts/build-ffmpeg.sh)
//! so the FLAC files are byte-identical.

mod ffmpeg;
mod path_template;
mod pcm;
pub mod provisional_tags;
mod splitter;

pub use ffmpeg::{Cancel, FFmpegTool, FeedDigest, StreamInfo, ToolError};
pub use path_template::{DEFAULT_ALBUM_FOLDER, DEFAULT_TRACK_FILE, PLACEHOLDERS, PathTemplate};
pub use pcm::{FormatError, PCMFormat, crc32_of_file};
pub use provisional_tags::AlbumContext;
pub use splitter::{
    ImageSplitter, SplitError, SplitOptions, SplitOutcome, SplitPhase, SplitProgress, SplitTrackOutput,
};
