//! Direct Stream Transfer decoder producing DSD bits.
//!
//! Rust port of the Mac app's DSTKit, itself a port of FFmpeg's
//! libavcodec/dstdec.c (LGPL 2.1+, Copyright (c) 2014 Peter Ross), stopped
//! before its DSD-to-PCM stage so the lossless bitstream can be written to
//! DSF. One DST frame holds 1/75 s: 588 × 64 = 37632 samples per channel at
//! DSD64. Output bytes are interleaved per channel with the earliest sample
//! in the MSB, the same layout uncompressed SACD frames use.
//!
//! Supported: the "same segmentation, single segment" layout every SACD in
//! the wild uses; multi-segment frames are reported as unsupported.
//!
//! This crate is LGPL-2.1-or-later (see LICENSE).

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DSTError {
    BadChannelCount(usize),
    BadSampleRate(u32),
    EmptyFrame,
    InvalidData(String),
    Unsupported(String),
}

impl fmt::Display for DSTError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DSTError::BadChannelCount(n) => write!(f, "DST supports up to 6 channels, not {n}."),
            DSTError::BadSampleRate(r) => write!(f, "Unsupported DSD sample rate {r}."),
            DSTError::EmptyFrame => write!(f, "Empty DST frame."),
            DSTError::InvalidData(m) => write!(f, "Invalid DST data: {m}."),
            DSTError::Unsupported(m) => write!(f, "Unsupported DST feature: {m}."),
        }
    }
}

impl std::error::Error for DSTError {}

fn invalid(m: &str) -> DSTError {
    DSTError::InvalidData(m.into())
}

pub const MAX_CHANNELS: usize = 6;
const MAX_ELEMENTS: usize = 12;
const MAX_COEFFICIENTS: usize = 128;

const FSETS_PRED: [[i32; 3]; 3] = [[-8, 0, 0], [-16, 8, 0], [-9, -5, 6]];
const PROBS_PRED: [[i32; 3]; 3] = [[-8, 0, 0], [-16, 8, 0], [-24, 24, -8]];

const fn bit_reverse_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut v = 0;
    while v < 256 {
        t[v] = (v as u8).reverse_bits();
        v += 1;
    }
    t
}
const BIT_REVERSE: [u8; 256] = bit_reverse_table();

/// Filter coefficient sets or probability tables.
struct Table {
    elements: usize,
    length: [usize; MAX_ELEMENTS],
    coeff: Vec<i32>, // MAX_ELEMENTS × MAX_COEFFICIENTS
}

impl Table {
    fn new() -> Self {
        Table { elements: 1, length: [0; MAX_ELEMENTS], coeff: vec![0; MAX_ELEMENTS * MAX_COEFFICIENTS] }
    }
}

/// MSB-first bit reader over one frame.
pub struct BitReader<'a> {
    bytes: &'a [u8],
    pub position: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        BitReader { bytes, position: 0 }
    }

    pub fn bits_left(&self) -> isize {
        self.bytes.len() as isize * 8 - self.position as isize
    }

    #[inline(always)]
    pub fn bit(&mut self) -> u32 {
        let byte = self.position >> 3;
        let v = self.bytes.get(byte).map_or(0, |b| ((b >> (7 - (self.position & 7))) & 1) as u32);
        self.position += 1;
        v
    }

    /// Up to 32 bits; past the end reads zeros.
    #[inline(always)]
    pub fn bits(&mut self, n: usize) -> u32 {
        if n == 0 {
            return 0;
        }
        let byte = self.position >> 3;
        let offset = self.position & 7;
        let mut window = 0u64;
        for k in 0..5 {
            window = (window << 8) | *self.bytes.get(byte + k).unwrap_or(&0) as u64;
        }
        let v = (window >> (40 - offset - n)) & ((1u64 << n) - 1);
        self.position += n;
        v as u32
    }

    pub fn sbits(&mut self, n: usize) -> i32 {
        let v = self.bits(n) as i64;
        (if v >= 1 << (n - 1) { v - (1 << n) } else { v }) as i32
    }

    /// Unary count of zero bits followed by k literal bits (JPEG-LS style
    /// Rice-Golomb, no escape), then a sign bit when non-zero.
    pub fn signed_rice_golomb(&mut self, k: usize) -> Result<i64, DSTError> {
        let mut zeros: i64 = 0;
        while self.bit() == 0 {
            zeros += 1;
            if self.bits_left() <= 0 {
                return Err(invalid("golomb code runs past the frame"));
            }
        }
        let mut v = self.bits(k) as i64 + (zeros << k);
        if v != 0 && self.bit() == 1 {
            v = -v;
        }
        Ok(v)
    }
}

/// Arithmetic decoder (10.11).
struct ArithCoder {
    a: u32,
    c: u32,
}

impl ArithCoder {
    #[inline(always)]
    fn get(&mut self, p: u32, gb: &mut BitReader) -> u32 {
        let k = (self.a >> 8) | ((self.a >> 7) & 1);
        let q = k.wrapping_mul(p);
        let aq = self.a.wrapping_sub(q);
        let e = self.c < aq;
        if e {
            self.a = aq;
        } else {
            self.a = q;
            self.c = self.c.wrapping_sub(aq);
        }
        if self.a < 2048 {
            let n = (11 - (31 - self.a.leading_zeros() as i32)) as usize;
            self.a <<= n;
            self.c = (self.c << n) | gb.bits(n);
        }
        e as u32
    }
}

fn log2_floor(v: usize) -> usize {
    if v == 0 { 0 } else { usize::BITS as usize - 1 - v.leading_zeros() as usize }
}

pub struct DSTDecoder {
    pub channels: usize,
    pub sample_rate: u32,
    pub samples_per_frame: usize,
    fsets: Table,
    probs: Table,
    map_f: [usize; MAX_CHANNELS],
    map_p: [usize; MAX_CHANNELS],
    half_prob: [bool; MAX_CHANNELS],
    /// filter[(elem * 16 + byte_index) * 256 + byte_value]
    filter: Vec<i16>,
}

impl DSTDecoder {
    /// `sample_rate` is the DSD bit rate (2822400 for DSD64).
    pub fn new(channels: usize, sample_rate: u32) -> Result<DSTDecoder, DSTError> {
        if !(1..=MAX_CHANNELS).contains(&channels) {
            return Err(DSTError::BadChannelCount(channels));
        }
        let fs44 = sample_rate / 44100;
        if !sample_rate.is_multiple_of(44100) || fs44 < 8 || !fs44.is_multiple_of(8) || fs44 > 512 * 8 {
            return Err(DSTError::BadSampleRate(sample_rate));
        }
        Ok(DSTDecoder {
            channels,
            sample_rate,
            samples_per_frame: 588 * fs44 as usize,
            fsets: Table::new(),
            probs: Table::new(),
            map_f: [0; MAX_CHANNELS],
            map_p: [0; MAX_CHANNELS],
            half_prob: [false; MAX_CHANNELS],
            filter: vec![0; MAX_ELEMENTS * 16 * 256],
        })
    }

    pub fn output_bytes_per_frame(&self) -> usize {
        self.samples_per_frame / 8 * self.channels
    }

    /// Decodes one frame into interleaved MSB-first DSD bytes.
    pub fn decode(&mut self, frame: &[u8]) -> Result<Vec<u8>, DSTError> {
        if frame.len() <= 1 {
            return Err(DSTError::EmptyFrame);
        }
        let out_bytes = self.output_bytes_per_frame();
        let mut gb = BitReader::new(frame);

        // Uncompressed frame: 0, x, 000000, then raw DSD bytes.
        if gb.bit() == 0 {
            gb.bit();
            if gb.bits(6) != 0 {
                return Err(invalid("bad uncompressed header"));
            }
            let mut out = vec![0u8; out_bytes];
            let n = (frame.len() - 1).min(out_bytes);
            out[..n].copy_from_slice(&frame[1..1 + n]);
            return Ok(out);
        }

        // Segmentation (10.4 - 10.6): only one segment for all channels.
        if gb.bit() != 1 {
            return Err(DSTError::Unsupported("not same segmentation".into()));
        }
        if gb.bit() != 1 {
            return Err(DSTError::Unsupported("not same segmentation for all channels".into()));
        }
        if gb.bit() != 1 {
            return Err(DSTError::Unsupported("not end of channel segmentation".into()));
        }

        // Mapping (10.7 - 10.9).
        let same_map = gb.bit() == 1;
        Self::read_map(&mut gb, self.channels, &mut self.fsets, &mut self.map_f)?;
        if same_map {
            self.probs.elements = self.fsets.elements;
            self.map_p = self.map_f;
        } else {
            Self::read_map(&mut gb, self.channels, &mut self.probs, &mut self.map_p)?;
        }

        // Half probability (10.10).
        for ch in 0..self.channels {
            self.half_prob[ch] = gb.bit() == 1;
        }

        // Filter coefficient sets (10.12) and probability tables (10.13).
        Self::read_table(&mut gb, &mut self.fsets, &FSETS_PRED, 7, 9, true, 0)?;
        Self::read_table(&mut gb, &mut self.probs, &PROBS_PRED, 6, 7, false, 1)?;

        // Arithmetic coded data (10.11).
        if gb.bit() != 0 {
            return Err(invalid("reserved bit set"));
        }
        let mut ac = ArithCoder { a: 4095, c: gb.bits(12) };
        self.build_filter()?;

        let first_coeff = self.fsets.coeff[0] as i64;
        ac.get((BIT_REVERSE[(first_coeff & 127) as usize] >> 1) as u32 + 1, &mut gb);

        let mut out = vec![0u8; out_bytes];
        self.decode_samples(&mut out, &mut gb, &mut ac);
        Ok(out)
    }

    /// The per-sample loop (10.11 + filter prediction).
    #[inline(never)]
    fn decode_samples(&self, out: &mut [u8], gb: &mut BitReader, ac: &mut ArithCoder) {
        let channels = self.channels;
        let mut lo = [0xAAAA_AAAA_AAAA_AAAAu64; MAX_CHANNELS];
        let mut hi = [0xAAAA_AAAA_AAAA_AAAAu64; MAX_CHANNELS];
        let bytes = gb.bytes;
        let mut pos = gb.position;
        let (mut a, mut c) = (ac.a, ac.c);
        for i in 0..self.samples_per_frame {
            let out_index = (i >> 3) * channels;
            let out_shift = 7 - (i & 7);
            for ch in 0..channels {
                let felem = self.map_f[ch];
                let base = &self.filter[felem * 16 * 256..(felem + 1) * 16 * 256];
                let (l, h) = (lo[ch], hi[ch]);
                let mut sum: i32 = 0;
                for b in 0..8 {
                    sum = sum.wrapping_add(base[b * 256 + ((l >> (8 * b)) & 0xFF) as usize] as i32);
                }
                for b in 0..8 {
                    sum = sum.wrapping_add(base[(8 + b) * 256 + ((h >> (8 * b)) & 0xFF) as usize] as i32);
                }
                let predict = sum as i16;

                let prob = if !self.half_prob[ch] || i >= self.fsets.length[felem] {
                    let pelem = self.map_p[ch];
                    let index = (predict.unsigned_abs() >> 3) as usize;
                    self.probs.coeff[pelem * MAX_COEFFICIENTS + index.min(self.probs.length[pelem] - 1)] as u32
                } else {
                    128
                };

                // Arithmetic decoder step, inlined.
                let k = (a >> 8) | ((a >> 7) & 1);
                let q = k.wrapping_mul(prob);
                let aq = a.wrapping_sub(q);
                let residual = if c < aq {
                    a = aq;
                    1
                } else {
                    a = q;
                    c = c.wrapping_sub(aq);
                    0
                };
                if a < 2048 {
                    let n = (11 - (31 - a.leading_zeros() as i32)) as usize;
                    let byte = pos >> 3;
                    let off = pos & 7;
                    let mut window = 0u64;
                    for kk in 0..3 {
                        window = (window << 8) | *bytes.get(byte + kk).unwrap_or(&0) as u64;
                    }
                    let read = (window >> (24 - off - n)) & ((1u64 << n) - 1);
                    pos += n;
                    a <<= n;
                    c = (c << n) | read as u32;
                }
                let v = ((predict >> 15) as i32 ^ residual) & 1;
                out[out_index + ch] |= (v as u8) << out_shift;

                hi[ch] = (h << 1) | (l >> 63);
                lo[ch] = (l << 1) | v as u64;
            }
        }
        gb.position = pos;
        ac.a = a;
        ac.c = c;
    }

    fn read_map(
        gb: &mut BitReader,
        channels: usize,
        table: &mut Table,
        map: &mut [usize; MAX_CHANNELS],
    ) -> Result<(), DSTError> {
        table.elements = 1;
        map[0] = 0;
        if gb.bit() == 0 {
            for slot in map.iter_mut().take(channels).skip(1) {
                let bits = log2_floor(table.elements) + 1;
                *slot = gb.bits(bits) as usize;
                if *slot == table.elements {
                    table.elements += 1;
                    if table.elements >= MAX_ELEMENTS {
                        return Err(invalid("too many map elements"));
                    }
                } else if *slot > table.elements {
                    return Err(invalid("map element out of range"));
                }
            }
        } else {
            *map = [0; MAX_CHANNELS];
        }
        Ok(())
    }

    fn read_table(
        gb: &mut BitReader,
        table: &mut Table,
        pred: &[[i32; 3]; 3],
        length_bits: usize,
        coeff_bits: usize,
        is_signed: bool,
        offset: i32,
    ) -> Result<(), DSTError> {
        let read = |gb: &mut BitReader| if is_signed { gb.sbits(coeff_bits) } else { gb.bits(coeff_bits) as i32 };
        for i in 0..table.elements {
            table.length[i] = gb.bits(length_bits) as usize + 1;
            if table.length[i] > MAX_COEFFICIENTS {
                return Err(invalid("table too long"));
            }
            let row = i * MAX_COEFFICIENTS;
            if gb.bit() == 0 {
                for j in 0..table.length[i] {
                    table.coeff[row + j] = read(gb) + offset;
                }
            } else {
                let method = gb.bits(2) as usize;
                if method == 3 {
                    return Err(invalid("bad coding method"));
                }
                for j in 0..=method {
                    table.coeff[row + j] = read(gb) + offset;
                }
                let lsb_size = gb.bits(3) as usize;
                for j in (method + 1)..table.length[i] {
                    let mut x: i32 = 0;
                    for (k, p) in pred[method].iter().enumerate().take(method + 1) {
                        x = x.wrapping_add(p.wrapping_mul(table.coeff[row + j - k - 1]));
                    }
                    let mut c = gb.signed_rice_golomb(lsb_size)?;
                    if x >= 0 {
                        c -= ((x as i64) + 4) / 8;
                    } else {
                        c += (-(x as i64) + 3) / 8;
                    }
                    if !is_signed && !(c >= offset as i64 && c < offset as i64 + (1 << coeff_bits)) {
                        return Err(invalid("probability out of range"));
                    }
                    table.coeff[row + j] = c as i32;
                }
            }
        }
        Ok(())
    }

    fn build_filter(&mut self) -> Result<(), DSTError> {
        for i in 0..self.fsets.elements {
            let length = self.fsets.length[i] as isize;
            for j in 0..16 {
                let total = (length - j as isize * 8).clamp(0, 8) as usize;
                for k in 0..256usize {
                    let mut v: i64 = 0;
                    for l in 0..total {
                        let sign: i64 = if (k >> l) & 1 == 1 { 1 } else { -1 };
                        v += sign * self.fsets.coeff[i * MAX_COEFFICIENTS + j * 8 + l] as i64;
                    }
                    self.filter[(i * 16 + j) * 256 + k] = i16::try_from(v).map_err(|_| invalid("filter overflow"))?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bits_msb_first() {
        let bytes = [0b1011_0010, 0b0111_1111, 0xFF, 0x00, 0x80];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.bit(), 1);
        assert_eq!(r.bit(), 0);
        assert_eq!(r.bits(3), 0b110);
        assert_eq!(r.bits(5), 0b01001);
        assert_eq!(r.sbits(3), -1);
        assert_eq!(r.bits(11), 0b111_1111_1111);
        assert_eq!(r.bits_left(), 40 - 24);
        assert_eq!(r.bits(16), 0x0080);
        assert_eq!(r.bit(), 0);
    }

    #[test]
    fn rice_golomb() {
        let bytes = [0b0011_0110, 0b0000_0000];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.signed_rice_golomb(2).unwrap(), -10);
        assert_eq!(r.signed_rice_golomb(2).unwrap(), 0);
    }

    #[test]
    fn rejects_bad_parameters() {
        assert_eq!(DSTDecoder::new(7, 2_822_400).err(), Some(DSTError::BadChannelCount(7)));
        assert_eq!(DSTDecoder::new(2, 48000).err(), Some(DSTError::BadSampleRate(48000)));
        let d = DSTDecoder::new(2, 2_822_400).unwrap();
        assert_eq!(d.samples_per_frame, 37632);
        assert_eq!(d.output_bytes_per_frame(), 9408);
    }

    #[test]
    fn passes_uncompressed_frames_through() {
        let mut d = DSTDecoder::new(2, 2_822_400).unwrap();
        let payload: Vec<u8> = (0..9408u32).map(|i| i.wrapping_mul(31) as u8).collect();
        let mut frame = vec![0x00];
        frame.extend(&payload);
        assert_eq!(d.decode(&frame).unwrap(), payload);
        let short = d.decode(&[0x00, 0xAB, 0xCD]).unwrap();
        assert_eq!(short.len(), 9408);
        assert_eq!(&short[..3], &[0xAB, 0xCD, 0]);
        assert_eq!(d.decode(&[0x3F, 0x00]).err(), Some(invalid("bad uncompressed header")));
    }

    #[test]
    fn rejects_multi_segment_frames() {
        let mut d = DSTDecoder::new(2, 2_822_400).unwrap();
        assert_eq!(d.decode(&[0b1000_0000, 0, 0]).err(), Some(DSTError::Unsupported("not same segmentation".into())));
    }
}
