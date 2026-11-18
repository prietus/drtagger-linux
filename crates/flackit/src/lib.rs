//! Audio container parsing shared by the tag writers: FLAC (metadata blocks,
//! STREAMINFO, Vorbis comments, rewrite with byte-exact audio), DSF and
//! ID3v2 (v2.3/v2.4 read, v2.3 write). Port of the Mac app's FLACKit; text
//! decoding follows Foundation's rules so tags read the same on both.

mod dsf;
mod error;
mod flac;
mod id3;
mod source;
pub mod text;
mod vorbis;

pub use dsf::DSFFile;
pub use error::FlacError;
pub use flac::{BlockType, FLACFile, MetadataBlock, Payload, StreamInfo};
pub use id3::{ID3v2Bridge, ID3v2Error, ID3v2Frame, ID3v2Tag};
pub use source::{DataSource, FileSource, MemSource, SparseSource};
pub use vorbis::VorbisComment;
