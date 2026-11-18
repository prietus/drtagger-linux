use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlacError {
    NotAFlacFile,
    Truncated,
    InvalidMetadataBlock(String),
    InvalidVorbisComment(String),
    UnsupportedStreamInfo,
    Io(String),
}

impl fmt::Display for FlacError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlacError::NotAFlacFile => write!(f, "not a supported audio file"),
            FlacError::Truncated => write!(f, "file is truncated"),
            FlacError::InvalidMetadataBlock(r) => write!(f, "invalid metadata block: {r}"),
            FlacError::InvalidVorbisComment(r) => write!(f, "invalid Vorbis comment: {r}"),
            FlacError::UnsupportedStreamInfo => write!(f, "missing or unsupported STREAMINFO"),
            FlacError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FlacError {}

impl From<std::io::Error> for FlacError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof { FlacError::Truncated } else { FlacError::Io(e.to_string()) }
    }
}
