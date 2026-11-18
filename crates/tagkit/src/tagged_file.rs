//! One entry point per file, dispatching on the container. Every write goes
//! to a temporary file next to the original while the audio payload is
//! hashed; only when that hash equals the original's does the temporary
//! file replace the track (rename, so a crash never leaves half a file).

use crate::containers::{ContainerContents, WriteSeek, ape, dsf, flac, iff::Flavor, mp4};
use crate::picture::{Picture, PictureInfo};
use crate::tag_set::TagSet;
use flackit::{DataSource, FileSource, FlacError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::OpenOptions;
use std::io::BufWriter;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagContainer {
    Flac,
    Dsf,
    Wav,
    Aiff,
    Dff,
    Ape,
    Mp4,
}

impl TagContainer {
    pub fn from_path(path: &Path) -> Option<TagContainer> {
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        Some(match ext.as_str() {
            "flac" => TagContainer::Flac,
            "dsf" => TagContainer::Dsf,
            "wav" | "wave" => TagContainer::Wav,
            "aiff" | "aif" | "aifc" => TagContainer::Aiff,
            "dff" => TagContainer::Dff,
            "ape" | "wv" | "tta" => TagContainer::Ape,
            "m4a" | "mp4" | "m4b" => TagContainer::Mp4,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagFileError {
    UnsupportedContainer(String),
    Malformed(String),
    AudioChanged { expected: String, actual: String },
    Io(String),
}

impl fmt::Display for TagFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TagFileError::UnsupportedContainer(ext) => write!(f, "Unsupported file type: .{ext}"),
            TagFileError::Malformed(why) => write!(f, "File structure not understood: {why}"),
            TagFileError::AudioChanged { expected, actual } => write!(
                f,
                "Audio bytes changed during write ({} → {}); original restored.",
                &expected[..expected.len().min(8)],
                &actual[..actual.len().min(8)]
            ),
            TagFileError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TagFileError {}

impl From<std::io::Error> for TagFileError {
    fn from(e: std::io::Error) -> Self {
        TagFileError::Io(e.to_string())
    }
}

impl From<FlacError> for TagFileError {
    fn from(e: FlacError) -> Self {
        match e {
            FlacError::Io(s) => TagFileError::Io(s),
            other => TagFileError::Malformed(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagWriteReport {
    #[serde(with = "librarykit_path")]
    pub path: PathBuf,
    pub container: TagContainer,
    pub audio_digest: String,
    pub bytes_written: i64,
    pub picture_count: usize,
}

/// Everything needed to put a file's metadata back exactly as it was.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagBackup {
    #[serde(with = "librarykit_path")]
    pub path: PathBuf,
    pub container: TagContainer,
    #[serde(with = "b64")]
    pub raw_metadata: Vec<u8>,
    pub audio_digest: String,
    pub tags: TagSet,
    pub pictures: Vec<PictureInfo>,
    /// Seconds since the Unix epoch.
    pub created_at: f64,
}

impl TagBackup {
    pub fn moved(&self, path: &Path) -> TagBackup {
        TagBackup { path: path.to_path_buf(), ..self.clone() }
    }
}

mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(d: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(d))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        base64::engine::general_purpose::STANDARD.decode(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// Paths as JSON strings when UTF-8, else as bytes (same as librarykit).
mod librarykit_path {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};

    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Text(String),
        Bytes { bytes: Vec<u8> },
    }

    pub fn serialize<S: Serializer>(p: &Path, s: S) -> Result<S::Ok, S::Error> {
        match p.to_str() {
            Some(t) => Repr::Text(t.to_string()),
            None => Repr::Bytes { bytes: p.as_os_str().as_bytes().to_vec() },
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
        Ok(match Repr::deserialize(d)? {
            Repr::Text(t) => PathBuf::from(t),
            Repr::Bytes { bytes } => PathBuf::from(OsString::from_vec(bytes)),
        })
    }
}

pub struct TaggedFile;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Depth {
    TagsOnly,
    Preview,
    Full,
}

impl TaggedFile {
    pub fn container(path: &Path) -> Result<TagContainer, TagFileError> {
        TagContainer::from_path(path).ok_or_else(|| {
            TagFileError::UnsupportedContainer(
                path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default(),
            )
        })
    }

    fn read_with(path: &Path, depth: Depth) -> Result<ContainerContents, TagFileError> {
        let kind = Self::container(path)?;
        let source = FileSource::open(path)?;
        let digest = depth == Depth::Full;
        match kind {
            TagContainer::Flac => flac::read(&source, digest, depth != Depth::TagsOnly),
            TagContainer::Dsf => dsf::read(&source, digest),
            TagContainer::Wav => Flavor::Wav.read(&source, digest),
            TagContainer::Aiff => Flavor::Aiff.read(&source, digest),
            TagContainer::Dff => Flavor::Dff.read(&source, digest),
            TagContainer::Ape => ape::read(&source, digest),
            TagContainer::Mp4 => mp4::read(&source, digest),
        }
    }

    /// Tags, pictures, raw metadata and the audio digest.
    pub fn read(path: &Path) -> Result<ContainerContents, TagFileError> {
        Self::read_with(path, Depth::Full)
    }

    /// Tags only: no audio is read, no picture payloads for FLAC — cheap
    /// enough for indexing a whole library over the network.
    pub fn read_tags(path: &Path) -> Result<TagSet, TagFileError> {
        Ok(Self::read_with(path, Depth::TagsOnly)?.tags)
    }

    /// Tags and pictures without hashing the audio.
    pub fn read_for_preview(path: &Path) -> Result<ContainerContents, TagFileError> {
        Self::read_with(path, Depth::Preview)
    }

    pub fn backup(path: &Path) -> Result<TagBackup, TagFileError> {
        let container = Self::container(path)?;
        let c = Self::read(path)?;
        Ok(TagBackup {
            path: path.to_path_buf(),
            container,
            raw_metadata: c.raw_metadata,
            audio_digest: c.audio_digest,
            tags: c.tags,
            pictures: c.pictures.iter().map(PictureInfo::from).collect(),
            created_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()),
        })
    }

    /// `pictures`: `None` keeps the file's pictures, `Some(&[])` removes
    /// them, otherwise they are replaced. A changed audio hash leaves the
    /// original untouched.
    pub fn write(
        path: &Path,
        tags: &TagSet,
        pictures: Option<&[Picture]>,
        expected_digest: Option<&str>,
    ) -> Result<TagWriteReport, TagFileError> {
        let kind = Self::container(path)?;
        let before = match expected_digest {
            Some(d) => d.to_string(),
            None => Self::read(path)?.audio_digest,
        };
        let picture_count = pictures.map_or(0, <[Picture]>::len);
        Self::replacing(path, kind, &before, picture_count, |source, out| match kind {
            TagContainer::Flac => flac::write(source, tags, pictures, out),
            TagContainer::Dsf => dsf::write(source, tags, pictures, out),
            TagContainer::Wav => Flavor::Wav.write(source, tags, pictures, out),
            TagContainer::Aiff => Flavor::Aiff.write(source, tags, pictures, out),
            TagContainer::Dff => Flavor::Dff.write(source, tags, pictures, out),
            TagContainer::Ape => ape::write(source, tags, pictures, out),
            TagContainer::Mp4 => mp4::write(source, tags, pictures, out),
        })
    }

    pub fn restore(path: &Path, backup: &TagBackup) -> Result<TagWriteReport, TagFileError> {
        let kind = Self::container(path)?;
        let raw = &backup.raw_metadata;
        Self::replacing(path, kind, &backup.audio_digest, backup.pictures.len(), |source, out| match kind {
            TagContainer::Flac => flac::restore(source, raw, out),
            TagContainer::Dsf => dsf::restore(source, raw, out),
            TagContainer::Wav => Flavor::Wav.restore(source, raw, out),
            TagContainer::Aiff => Flavor::Aiff.restore(source, raw, out),
            TagContainer::Dff => Flavor::Dff.restore(source, raw, out),
            TagContainer::Ape => ape::restore(source, raw, out),
            TagContainer::Mp4 => mp4::restore(source, raw, out),
        })
    }

    /// Existing pictures with the front cover swapped (or added).
    pub fn replacing_front(existing: &[Picture], front: Picture) -> Vec<Picture> {
        std::iter::once(front).chain(existing.iter().filter(|p| !p.is_front()).cloned()).collect()
    }

    fn replacing(
        path: &Path,
        kind: TagContainer,
        expected: &str,
        picture_count: usize,
        body: impl FnOnce(&dyn DataSource, &mut dyn WriteSeek) -> Result<String, TagFileError>,
    ) -> Result<TagWriteReport, TagFileError> {
        let name = path.file_name().ok_or_else(|| TagFileError::Malformed("no file name".into()))?;
        let mut tmp_name = b".".to_vec();
        tmp_name.extend_from_slice(name.as_bytes());
        tmp_name.extend_from_slice(b".drtagger.tmp");
        let tmp = path.with_file_name(std::ffi::OsStr::from_bytes(&tmp_name));
        let _ = std::fs::remove_file(&tmp);
        // The new file keeps the original's permission bits.
        let mode = std::fs::metadata(path)?.permissions().mode() & 0o7777;
        let result = (|| -> Result<String, TagFileError> {
            let source = FileSource::open(path)?;
            let file = OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
            let mut out = BufWriter::with_capacity(1 << 20, file);
            let digest = body(&source, &mut out)?;
            let file = out.into_inner().map_err(|e| TagFileError::Io(e.error().to_string()))?;
            file.sync_all()?;
            Ok(digest)
        })();
        let digest = match result {
            Ok(d) => d,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        if digest != expected {
            let _ = std::fs::remove_file(&tmp);
            return Err(TagFileError::AudioChanged { expected: expected.to_string(), actual: digest });
        }
        let size = std::fs::metadata(&tmp).map_or(0, |m| m.len() as i64);
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(TagWriteReport {
            path: path.to_path_buf(),
            container: kind,
            audio_digest: digest,
            bytes_written: size,
            picture_count,
        })
    }
}
