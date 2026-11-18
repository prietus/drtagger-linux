//! Reassembles frames (1/75 s) from an area's audio sectors. Sector layout:
//! header byte = packet_info_count:3, frame_info_count:3, reserved:1, dst:1;
//! packet entries of 2 bytes (frame_start:1, reserved:1, data_type:3,
//! length:11); frame infos of 3 bytes (min, sec, frame) for DSD or 4 for
//! DST; then the payloads. A frame runs from one frame_start packet to the
//! next, possibly across sectors.

use crate::scarletbook::{DATA_TYPE_AUDIO, SACDTime, SECTOR_SIZE};
use std::collections::VecDeque;
use std::fmt;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SACDFrame {
    pub timecode: SACDTime,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderError {
    CannotOpen(String),
    Truncated { sector: u64 },
    Malformed { sector: u64, reason: String },
}

impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReaderError::CannotOpen(p) => write!(f, "Cannot open {p}."),
            ReaderError::Truncated { sector } => write!(f, "Image ends before sector {sector}."),
            ReaderError::Malformed { sector, reason } => write!(f, "Audio sector {sector} is malformed: {reason}."),
        }
    }
}

impl std::error::Error for ReaderError {}

struct Packet {
    frame_start: bool,
    data_type: u8,
    start: usize,
    end: usize,
}

struct AudioSector {
    packets: Vec<Packet>,
    timecodes: Vec<SACDTime>,
}

impl AudioSector {
    fn parse(s: &[u8], number: u64) -> Result<AudioSector, ReaderError> {
        let malformed = |r: &str| ReaderError::Malformed { sector: number, reason: r.into() };
        let header = s[0];
        let packet_count = ((header >> 5) & 7) as usize;
        let frame_count = ((header >> 2) & 7) as usize;
        let is_dst = header & 1 != 0;
        let mut offset = 1;
        let mut lengths = vec![];
        for _ in 0..packet_count {
            if offset + 2 > s.len() {
                return Err(malformed("packet table truncated"));
            }
            let word = (s[offset] as usize) << 8 | s[offset + 1] as usize;
            offset += 2;
            lengths.push(((word >> 15) & 1 == 1, ((word >> 11) & 7) as u8, word & 0x7FF));
        }
        let info = if is_dst { 4 } else { 3 };
        let mut timecodes = vec![];
        for _ in 0..frame_count {
            if offset + info > s.len() {
                return Err(malformed("frame table truncated"));
            }
            timecodes.push(SACDTime::msf(s[offset], s[offset + 1], s[offset + 2]));
            offset += info;
        }
        let mut packets = vec![];
        for (frame_start, data_type, length) in lengths {
            if offset + length > s.len() {
                return Err(malformed("packet runs past the sector"));
            }
            packets.push(Packet { frame_start, data_type, start: offset, end: offset + length });
            offset += length;
        }
        Ok(AudioSector { packets, timecodes })
    }
}

pub struct SACDFrameReader {
    file: File,
    end_sector: u64,
    next_sector: u64,
    buffer: Vec<u8>,
    buffer_first_sector: u64,
    current: Option<SACDFrame>,
    queue: VecDeque<SACDFrame>,
    exhausted: bool,
    pub sectors_read: u64,
    pub total_sectors: u64,
}

const SECTORS_PER_READ: u64 = 512;

impl SACDFrameReader {
    pub fn new(path: &Path, first_sector: u32, last_sector: u32) -> Result<Self, ReaderError> {
        let file = File::open(path).map_err(|_| ReaderError::CannotOpen(path.display().to_string()))?;
        Ok(SACDFrameReader {
            file,
            end_sector: last_sector as u64,
            next_sector: first_sector as u64,
            buffer: vec![],
            buffer_first_sector: 0,
            current: None,
            queue: VecDeque::new(),
            exhausted: false,
            sectors_read: 0,
            total_sectors: (last_sector as u64 + 1).saturating_sub(first_sector as u64),
        })
    }

    /// The next complete frame, or `None` after the last sector.
    pub fn next_frame(&mut self) -> Result<Option<SACDFrame>, ReaderError> {
        while self.queue.is_empty() {
            if self.exhausted {
                return Ok(None);
            }
            let Some(range) = self.read_sector()? else {
                self.exhausted = true;
                if let Some(c) = self.current.take() {
                    self.queue.push_back(c);
                }
                continue;
            };
            let sector = &self.buffer[range];
            let parsed = AudioSector::parse(sector, self.next_sector - 1)?;
            let mut frame_index = 0;
            for packet in parsed.packets.iter().filter(|p| p.data_type == DATA_TYPE_AUDIO) {
                if packet.frame_start {
                    if let Some(c) = self.current.take() {
                        self.queue.push_back(c);
                    }
                    let timecode = parsed.timecodes.get(frame_index).copied().unwrap_or(SACDTime::new(0));
                    frame_index += 1;
                    self.current = Some(SACDFrame { timecode, data: Vec::with_capacity(16384) });
                }
                if let Some(c) = self.current.as_mut() {
                    c.data.extend_from_slice(&sector[packet.start..packet.end]);
                }
            }
        }
        Ok(self.queue.pop_front())
    }

    fn read_sector(&mut self) -> Result<Option<std::ops::Range<usize>>, ReaderError> {
        if self.next_sector > self.end_sector {
            return Ok(None);
        }
        let in_buffer = self.next_sector >= self.buffer_first_sector
            && ((self.next_sector - self.buffer_first_sector + 1) as usize) * SECTOR_SIZE <= self.buffer.len();
        if !in_buffer {
            let count = SECTORS_PER_READ.min(self.end_sector - self.next_sector + 1) as usize;
            self.buffer.resize(count * SECTOR_SIZE, 0);
            let mut filled = 0;
            let offset = self.next_sector * SECTOR_SIZE as u64;
            while filled < self.buffer.len() {
                match self.file.read_at(&mut self.buffer[filled..], offset + filled as u64) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            self.buffer.truncate(filled);
            self.buffer_first_sector = self.next_sector;
            if self.buffer.len() < SECTOR_SIZE {
                return Err(ReaderError::Truncated { sector: self.next_sector });
            }
        }
        let start = (self.next_sector - self.buffer_first_sector) as usize * SECTOR_SIZE;
        self.next_sector += 1;
        self.sectors_read += 1;
        Ok(Some(start..start + SECTOR_SIZE))
    }
}
