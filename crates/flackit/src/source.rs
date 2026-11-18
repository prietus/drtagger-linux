//! Random-access byte sources the parsers read from.

use crate::FlacError;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;

pub trait DataSource: Send + Sync {
    fn len(&self) -> Result<u64, FlacError>;

    /// Exactly `length` bytes at `offset`, or `Truncated`.
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, FlacError>;

    fn is_empty(&self) -> Result<bool, FlacError> {
        Ok(self.len()? == 0)
    }
}

/// A local file read with positioned reads (no shared cursor, so one source
/// can serve several threads).
pub struct FileSource {
    file: File,
    length: u64,
}

impl FileSource {
    pub fn open(path: &Path) -> Result<FileSource, FlacError> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        Ok(FileSource { file, length })
    }
}

impl DataSource for FileSource {
    fn len(&self) -> Result<u64, FlacError> {
        Ok(self.length)
    }

    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, FlacError> {
        if offset.checked_add(length as u64).is_none_or(|end| end > self.length) {
            return Err(FlacError::Truncated);
        }
        let mut buf = vec![0u8; length];
        self.file.read_exact_at(&mut buf, offset)?;
        Ok(buf)
    }
}

/// An in-memory buffer.
#[derive(Clone)]
pub struct MemSource(pub Arc<Vec<u8>>);

impl MemSource {
    pub fn new(data: Vec<u8>) -> Self {
        MemSource(Arc::new(data))
    }
}

impl DataSource for MemSource {
    fn len(&self) -> Result<u64, FlacError> {
        Ok(self.0.len() as u64)
    }

    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, FlacError> {
        let start = usize::try_from(offset).map_err(|_| FlacError::Truncated)?;
        let end = start.checked_add(length).ok_or(FlacError::Truncated)?;
        self.0.get(start..end).map(<[u8]>::to_vec).ok_or(FlacError::Truncated)
    }
}

/// Two non-contiguous regions (a head and a tail) of a larger file, without
/// the gap: big DSF files keep the header at 0 and the ID3 tag at the end.
/// Reads that fall in the gap return zeros.
pub struct SparseSource {
    head: Vec<u8>,
    tail: Vec<u8>,
    tail_offset: u64,
    total_length: u64,
}

impl SparseSource {
    pub fn new(head: Vec<u8>, tail: Vec<u8>, tail_offset: u64, total_length: u64) -> Self {
        SparseSource { head, tail, tail_offset, total_length }
    }
}

impl DataSource for SparseSource {
    fn len(&self) -> Result<u64, FlacError> {
        Ok(self.total_length)
    }

    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, FlacError> {
        let end = offset + length as u64;
        if end <= self.head.len() as u64 {
            return Ok(self.head[offset as usize..end as usize].to_vec());
        }
        if offset >= self.tail_offset && end <= self.tail_offset + self.tail.len() as u64 {
            let s = (offset - self.tail_offset) as usize;
            return Ok(self.tail[s..s + length].to_vec());
        }
        Ok(vec![0u8; length])
    }
}
