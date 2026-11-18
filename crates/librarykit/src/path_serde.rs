//! Paths are byte strings on Linux and a NAS may hold Latin-1 names. They
//! serialise as a JSON string when they are UTF-8 and as `{"bytes": [...]}`
//! otherwise, so the store keeps them exactly.

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

fn repr(path: &Path) -> Repr {
    match path.to_str() {
        Some(s) => Repr::Text(s.to_string()),
        None => Repr::Bytes { bytes: path.as_os_str().as_bytes().to_vec() },
    }
}

fn from_repr(r: Repr) -> PathBuf {
    match r {
        Repr::Text(s) => PathBuf::from(s),
        Repr::Bytes { bytes } => PathBuf::from(OsString::from_vec(bytes)),
    }
}

pub fn serialize<S: Serializer>(path: &Path, s: S) -> Result<S::Ok, S::Error> {
    repr(path).serialize(s)
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PathBuf, D::Error> {
    Repr::deserialize(d).map(from_repr)
}

pub mod option {
    use super::*;

    pub fn serialize<S: Serializer>(path: &Option<PathBuf>, s: S) -> Result<S::Ok, S::Error> {
        path.as_deref().map(repr).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<PathBuf>, D::Error> {
        Option::<Repr>::deserialize(d).map(|r| r.map(from_repr))
    }
}

pub mod vec {
    use super::*;

    pub fn serialize<S: Serializer>(paths: &[PathBuf], s: S) -> Result<S::Ok, S::Error> {
        paths.iter().map(|p| repr(p)).collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<PathBuf>, D::Error> {
        Vec::<Repr>::deserialize(d).map(|v| v.into_iter().map(from_repr).collect())
    }
}
