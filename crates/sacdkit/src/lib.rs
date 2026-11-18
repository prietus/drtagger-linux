//! Scarletbook (SACD ISO) reader and extractor: area TOCs, track lists,
//! ISRCs, genres and texts; frame reassembly from audio sectors; DST
//! decoding (dstkit) in parallel batches; DSF writing with ID3 tags. Port
//! of the Mac app's SACDKit; the DSF files are byte-identical.

mod disc;
mod dsf_writer;
mod extractor;
mod frame_reader;
pub mod scarletbook;

pub use disc::{ReadError, SACDArea, SACDDisc, SACDTrack, read};
pub use dsf_writer::{BLOCK_SIZE, DSFWriter, WriterError, bit_reverse, channel_type};
pub use extractor::{
    PausePolicy, SACDExtractError, SACDExtractOptions, SACDExtractOutcome, SACDExtractProgress, SACDExtractor,
    SACDTrackOutput, comment, id3,
};
pub use frame_reader::{ReaderError, SACDFrame, SACDFrameReader};
pub use scarletbook::SACDTime;
