//! Chromaprint fingerprints (port of ChromaprintKit). The library takes
//! 16-bit interleaved PCM at its source rate and channel count, resamples
//! and mixes down itself, and returns the compressed base64 fingerprint
//! AcoustID expects, plus the raw 32-bit values (`fpcalc -raw`).
//!
//! The C++ sources are the same Chromaprint 1.5.1 the Mac app vendors; only
//! the FFT differs (KissFFT here, vDSP there), so fingerprints of the same
//! audio agree in all but a few bits. [`similarity`] measures how close two
//! raw fingerprints are.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::fmt;

#[repr(C)]
struct ChromaprintContext {
    _private: [u8; 0],
}

const ALGORITHM_DEFAULT: c_int = 1; // CHROMAPRINT_ALGORITHM_TEST2

unsafe extern "C" {
    fn chromaprint_new(algorithm: c_int) -> *mut ChromaprintContext;
    fn chromaprint_free(ctx: *mut ChromaprintContext);
    fn chromaprint_start(ctx: *mut ChromaprintContext, sample_rate: c_int, num_channels: c_int) -> c_int;
    fn chromaprint_feed(ctx: *mut ChromaprintContext, data: *const i16, size: c_int) -> c_int;
    fn chromaprint_finish(ctx: *mut ChromaprintContext) -> c_int;
    fn chromaprint_get_fingerprint(ctx: *mut ChromaprintContext, fingerprint: *mut *mut c_char) -> c_int;
    fn chromaprint_get_raw_fingerprint(ctx: *mut ChromaprintContext, fp: *mut *mut u32, size: *mut c_int) -> c_int;
    fn chromaprint_decode_fingerprint(
        encoded: *const c_char,
        encoded_size: c_int,
        fp: *mut *mut u32,
        size: *mut c_int,
        algorithm: *mut c_int,
        base64: c_int,
    ) -> c_int;
    fn chromaprint_dealloc(ptr: *mut c_void);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FingerprinterError {
    ContextAllocationFailed,
    StartFailed,
    FeedFailed,
    FinishFailed,
    GetFingerprintFailed,
}

impl fmt::Display for FingerprinterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FingerprinterError::ContextAllocationFailed => "Could not create chromaprint context.",
            FingerprinterError::StartFailed => "chromaprint_start failed.",
            FingerprinterError::FeedFailed => "chromaprint_feed failed.",
            FingerprinterError::FinishFailed => "chromaprint_finish failed.",
            FingerprinterError::GetFingerprintFailed => "chromaprint_get_fingerprint failed.",
        })
    }
}

impl std::error::Error for FingerprinterError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    /// Compressed, base64url: what AcoustID takes.
    pub fingerprint: String,
    /// Whole seconds, as passed in.
    pub duration_seconds: i64,
    /// The values `fpcalc -raw` prints.
    pub raw: Vec<u32>,
}

struct Context(*mut ChromaprintContext);

impl Drop for Context {
    fn drop(&mut self) {
        unsafe { chromaprint_free(self.0) }
    }
}

/// Fingerprints interleaved 16-bit PCM (`samples.len()` = frames × channels).
pub fn fingerprint(
    samples: &[i16],
    sample_rate: i64,
    channels: i64,
    duration_seconds: i64,
) -> Result<Fingerprint, FingerprinterError> {
    let ctx = unsafe { chromaprint_new(ALGORITHM_DEFAULT) };
    if ctx.is_null() {
        return Err(FingerprinterError::ContextAllocationFailed);
    }
    let ctx = Context(ctx);
    unsafe {
        if chromaprint_start(ctx.0, sample_rate as c_int, channels as c_int) != 1 {
            return Err(FingerprinterError::StartFailed);
        }
        if chromaprint_feed(ctx.0, samples.as_ptr(), samples.len() as c_int) != 1 {
            return Err(FingerprinterError::FeedFailed);
        }
        if chromaprint_finish(ctx.0) != 1 {
            return Err(FingerprinterError::FinishFailed);
        }
        let mut out: *mut c_char = std::ptr::null_mut();
        if chromaprint_get_fingerprint(ctx.0, &mut out) != 1 || out.is_null() {
            return Err(FingerprinterError::GetFingerprintFailed);
        }
        let fingerprint = CStr::from_ptr(out).to_string_lossy().into_owned();
        chromaprint_dealloc(out.cast());
        let mut raw_ptr: *mut u32 = std::ptr::null_mut();
        let mut size: c_int = 0;
        let mut raw = vec![];
        if chromaprint_get_raw_fingerprint(ctx.0, &mut raw_ptr, &mut size) == 1 && !raw_ptr.is_null() {
            raw = std::slice::from_raw_parts(raw_ptr, size.max(0) as usize).to_vec();
            chromaprint_dealloc(raw_ptr.cast());
        }
        Ok(Fingerprint { fingerprint, duration_seconds, raw })
    }
}

/// The raw values of a compressed base64 fingerprint; None when it does
/// not decode.
pub fn decode(encoded: &str) -> Option<Vec<u32>> {
    let mut fp: *mut u32 = std::ptr::null_mut();
    let mut size: c_int = 0;
    let mut algorithm: c_int = 0;
    unsafe {
        let ok = chromaprint_decode_fingerprint(
            encoded.as_ptr().cast(),
            encoded.len() as c_int,
            &mut fp,
            &mut size,
            &mut algorithm,
            1,
        );
        if ok != 1 || fp.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(fp, size.max(0) as usize).to_vec();
        chromaprint_dealloc(fp.cast());
        if v.is_empty() { None } else { Some(v) }
    }
}

/// Share of equal bits between two raw fingerprints at the best alignment
/// within ±`max_offset` frames (0…1; 1 = identical). Frames are ~0.124 s.
pub fn similarity(a: &[u32], b: &[u32], max_offset: usize) -> f64 {
    let mut best = 0f64;
    for off in -(max_offset as isize)..=(max_offset as isize) {
        let (xa, xb) = if off >= 0 {
            (a.get(off as usize..).unwrap_or(&[]), b)
        } else {
            (a, b.get((-off) as usize..).unwrap_or(&[]))
        };
        let n = xa.len().min(xb.len());
        if n == 0 {
            continue;
        }
        let errors: u32 = xa[..n].iter().zip(&xb[..n]).map(|(x, y)| (x ^ y).count_ones()).sum();
        best = best.max(1.0 - errors as f64 / (32.0 * n as f64));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    // A deterministic, non-periodic signal: two drifting tones and noise.
    fn signal(seconds: usize, rate: usize, seed: u32) -> Vec<i16> {
        let mut state = seed;
        (0..seconds * rate)
            .flat_map(|i| {
                let t = i as f64 / rate as f64;
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let noise = (state as f64 / u32::MAX as f64 - 0.5) * 2000.0;
                let f = 220.0 * (1.0 + (t / 3.0 + seed as f64).sin() * 0.5);
                let v = (t * f * std::f64::consts::TAU).sin() * 8000.0
                    + (t * f * 1.5 * std::f64::consts::TAU).sin() * 4000.0
                    + noise;
                let s = v as i16;
                [s, s]
            })
            .collect()
    }

    #[test]
    fn fingerprint_round_trips_through_decode() {
        let pcm = signal(30, 44100, 0x2545_f491);
        let fp = fingerprint(&pcm, 44100, 2, 30).unwrap();
        assert!(fp.fingerprint.starts_with("AQAA"), "{}", &fp.fingerprint[..8]);
        assert!(fp.raw.len() > 200, "{} frames", fp.raw.len());
        assert_eq!(decode(&fp.fingerprint).unwrap(), fp.raw);
        assert_eq!(similarity(&fp.raw, &fp.raw, 10), 1.0);
    }

    #[test]
    fn similarity_tells_different_audio_apart() {
        let a = fingerprint(&signal(20, 44100, 0x2545_f491), 44100, 2, 20).unwrap();
        let quiet: Vec<i16> = signal(20, 44100, 0x2545_f491).iter().map(|s| s / 2).collect();
        let b = fingerprint(&quiet, 44100, 2, 20).unwrap();
        // Half the level is the same audio for Chromaprint.
        assert!(similarity(&a.raw, &b.raw, 10) > 0.95);
        let c = fingerprint(&signal(20, 44100, 0x1234_5677), 44100, 2, 20).unwrap();
        assert!(similarity(&a.raw, &c.raw, 0) < 0.85, "{}", similarity(&a.raw, &c.raw, 0));
    }

    #[test]
    fn junk_does_not_decode() {
        assert_eq!(decode("@@@"), None);
    }
}
