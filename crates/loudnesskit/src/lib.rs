//! ITU-R BS.1770-4 / EBU R128 loudness and ReplayGain. Port of the Mac app's
//! LoudnessKit: K-weighting biquads derived for any sample rate the way
//! libebur128 does, 400 ms blocks every 100 ms, absolute (-70 LUFS) and
//! relative (-10 LU) gates, sample peak and true peak by polyphase sinc
//! interpolation (4× below 96 kHz, 2× below 192 kHz). Audio comes from the
//! bundled ffmpeg as 32-bit PCM; DSD and anything above 192 kHz is
//! resampled to 88.2 kHz first.

use serde::{Deserialize, Serialize};
use splitkit::{FFmpegTool, PCMFormat, ToolError};
use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One measurement. Powers are linear mean squares of the K-weighted
/// signal; loudness in LUFS is -0.691 + 10·log10(power).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessResult {
    /// `None` when everything is gated out (silence).
    pub integrated_lufs: Option<f64>,
    /// Silence encodes as -200.
    pub true_peak_dbtp: f64,
    pub sample_peak_dbfs: f64,
    /// 400 ms momentary blocks every 100 ms.
    pub block_powers: Vec<f64>,
    pub sample_rate: i64,
    pub channels: usize,
    pub duration_seconds: f64,
}

fn finite_or(v: f64) -> f64 {
    if v.is_finite() { v } else { -200.0 }
}

/// Direct-form biquad cascade (shelf, then high-pass) in double precision.
#[derive(Clone)]
struct Biquad {
    // b0 b1 b2 a1 a2 per section
    coeffs: [[f64; 5]; 2],
    state: [[f64; 4]; 2], // x1 x2 y1 y2
}

impl Biquad {
    #[inline]
    fn run(&mut self, mut x: f64) -> f64 {
        for (c, s) in self.coeffs.iter().zip(self.state.iter_mut()) {
            let y = c[0] * x + c[1] * s[0] + c[2] * s[1] - c[3] * s[2] - c[4] * s[3];
            s[1] = s[0];
            s[0] = x;
            s[3] = s[2];
            s[2] = y;
            x = y;
        }
        x
    }
}

pub struct R128Meter {
    pub sample_rate: i64,
    pub channels: usize,
    biquads: Vec<Biquad>,
    weights: Vec<f64>,
    sub_block_frames: usize,
    sub_sum: Vec<f64>,
    sub_count: usize,
    sub_powers: Vec<f64>,
    block_powers: Vec<f64>,
    frames: i64,
    sample_peak: f32,
    true_peak: f32,
    oversample: usize,
    /// oversample × taps; y[n] = Σ_k phase[k]·x[n−k].
    phases: Vec<Vec<f32>>,
    taps: usize,
    /// Per channel: the last taps−1 raw samples.
    history: Vec<Vec<f32>>,
}

impl R128Meter {
    pub fn new(sample_rate: i64, channels: usize) -> Self {
        let fs = sample_rate as f64;
        // Shelf then high-pass (libebur128 constants).
        let (f1, g, q1) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
        let k1 = (PI * f1 / fs).tan();
        let vh = 10f64.powf(g / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a01 = 1.0 + k1 / q1 + k1 * k1;
        let shelf = [
            (vh + vb * k1 / q1 + k1 * k1) / a01,
            2.0 * (k1 * k1 - vh) / a01,
            (vh - vb * k1 / q1 + k1 * k1) / a01,
            2.0 * (k1 * k1 - 1.0) / a01,
            (1.0 - k1 / q1 + k1 * k1) / a01,
        ];
        let (f2, q2) = (38.13547087602444, 0.5003270373238773);
        let k2 = (PI * f2 / fs).tan();
        let a02 = 1.0 + k2 / q2 + k2 * k2;
        // libebur128 keeps the high-pass numerator at 1, -2, 1 (not divided
        // by a0): that +0.04 dB is part of the reference the -0.691 offset
        // was fitted to.
        let hp = [1.0, -2.0, 1.0, 2.0 * (k2 * k2 - 1.0) / a02, (1.0 - k2 / q2 + k2 * k2) / a02];
        let biquad = Biquad { coeffs: [shelf, hp], state: [[0.0; 4]; 2] };

        // Channel weights: L R C 1.0, LFE 0 (5.1 / 7.1), surrounds 1.41.
        let weights = (0..channels)
            .map(|c| {
                if channels >= 6 && c == 3 {
                    0.0
                } else if (channels >= 4 && c >= 3) || (channels == 4 && c >= 2) {
                    1.41
                } else {
                    1.0
                }
            })
            .collect();

        let factor = if sample_rate < 96000 {
            4
        } else if sample_rate < 192000 {
            2
        } else {
            1
        };
        let taps = 12;
        let mut phases = vec![];
        if factor > 1 {
            let n = taps * factor;
            let center = (n - 1) as f64 / 2.0;
            let h: Vec<f64> = (0..n)
                .map(|i| {
                    let x = (i as f64 - center) / factor as f64;
                    let sinc = if x == 0.0 { 1.0 } else { (PI * x).sin() / (PI * x) };
                    let window = 0.5 - 0.5 * (2.0 * PI * i as f64 / (n - 1) as f64).cos();
                    sinc * window
                })
                .collect();
            for p in 0..factor {
                let mut phase: Vec<f64> = (0..taps).map(|k| h[p + factor * k]).collect();
                let sum: f64 = phase.iter().sum();
                if sum != 0.0 {
                    phase.iter_mut().for_each(|v| *v /= sum);
                }
                phases.push(phase.into_iter().map(|v| v as f32).collect());
            }
        }

        R128Meter {
            sample_rate,
            channels,
            biquads: vec![biquad; channels],
            weights,
            sub_block_frames: (sample_rate / 10).max(1) as usize,
            sub_sum: vec![0.0; channels],
            sub_count: 0,
            sub_powers: vec![],
            block_powers: vec![],
            frames: 0,
            sample_peak: 0.0,
            true_peak: 0.0,
            oversample: factor,
            phases,
            taps,
            history: vec![vec![0.0; taps - 1]; channels],
        }
    }

    /// Interleaved samples in any chunking.
    pub fn process(&mut self, samples: &[f32]) {
        let ch = self.channels;
        let frame_count = samples.len() / ch;
        if frame_count == 0 {
            return;
        }
        let taps = self.taps;
        let mut squared_per_channel: Vec<Vec<f64>> = Vec::with_capacity(ch);
        let mut padded = vec![0f32; frame_count + taps - 1];
        for c in 0..ch {
            let chan: Vec<f32> = samples.iter().skip(c).step_by(ch).take(frame_count).copied().collect();
            let peak = chan.iter().fold(0f32, |m, v| m.max(v.abs()));
            if peak > self.sample_peak {
                self.sample_peak = peak;
            }
            if self.oversample > 1 {
                padded[..taps - 1].copy_from_slice(&self.history[c]);
                padded[taps - 1..].copy_from_slice(&chan);
                for phase in &self.phases {
                    let mut p = 0f32;
                    // Accumulated in double: vDSP_conv's own order is not public,
                    // and the Mac's float sums can differ in the last bit; this
                    // is the closer of the two to the exact value.
                    for n in 0..frame_count {
                        let acc =
                            (0..taps).fold(0f64, |a, j| a + padded[n + j] as f64 * phase[taps - 1 - j] as f64) as f32;
                        p = p.max(acc.abs());
                    }
                    if p > self.true_peak {
                        self.true_peak = p;
                    }
                }
                self.history[c] = padded[frame_count..frame_count + taps - 1].to_vec();
            }
            let bq = &mut self.biquads[c];
            squared_per_channel.push(
                chan.iter()
                    .map(|&s| {
                        let y = bq.run(s as f64);
                        y * y
                    })
                    .collect(),
            );
        }

        // Sub-blocks of 100 ms across chunk boundaries.
        let mut offset = 0;
        while offset < frame_count {
            let take = (self.sub_block_frames - self.sub_count).min(frame_count - offset);
            for (c, sq) in squared_per_channel.iter().enumerate() {
                self.sub_sum[c] += sq[offset..offset + take].iter().sum::<f64>();
            }
            self.sub_count += take;
            offset += take;
            if self.sub_count == self.sub_block_frames {
                self.close_sub_block();
            }
        }
        self.frames += frame_count as i64;
    }

    fn close_sub_block(&mut self) {
        let mut power = 0.0;
        for c in 0..self.channels {
            power += self.weights[c] * self.sub_sum[c] / self.sub_block_frames as f64;
            self.sub_sum[c] = 0.0;
        }
        self.sub_powers.push(power);
        self.sub_count = 0;
        let n = self.sub_powers.len();
        if n >= 4 {
            let p = &self.sub_powers;
            self.block_powers.push((p[n - 1] + p[n - 2] + p[n - 3] + p[n - 4]) / 4.0);
        }
    }

    pub fn finish(self) -> LoudnessResult {
        let peak = if self.oversample > 1 { self.true_peak.max(self.sample_peak) } else { self.sample_peak } as f64;
        LoudnessResult {
            integrated_lufs: integrated(&self.block_powers),
            true_peak_dbtp: finite_or(20.0 * peak.log10()),
            sample_peak_dbfs: finite_or(20.0 * (self.sample_peak as f64).log10()),
            block_powers: self.block_powers,
            sample_rate: self.sample_rate,
            channels: self.channels,
            duration_seconds: self.frames as f64 / self.sample_rate as f64,
        }
    }
}

pub fn loudness(power: f64) -> f64 {
    -0.691 + 10.0 * power.log10()
}

/// Gated integration over any set of blocks: one track, or every block of
/// an album for the album loudness.
pub fn integrated(block_powers: &[f64]) -> Option<f64> {
    let absolute: Vec<f64> = block_powers.iter().copied().filter(|p| loudness(*p) > -70.0).collect();
    if absolute.is_empty() {
        return None;
    }
    let relative = loudness(absolute.iter().sum::<f64>() / absolute.len() as f64) - 10.0;
    let gated: Vec<f64> = absolute.into_iter().filter(|p| loudness(*p) > relative).collect();
    if gated.is_empty() {
        return None;
    }
    Some(loudness(gated.iter().sum::<f64>() / gated.len() as f64))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumLoudness {
    pub tracks: Vec<LoudnessResult>,
    pub integrated_lufs: Option<f64>,
    pub true_peak_dbtp: f64,
}

impl AlbumLoudness {
    pub fn new(tracks: Vec<LoudnessResult>) -> Self {
        let blocks: Vec<f64> = tracks.iter().flat_map(|t| t.block_powers.iter().copied()).collect();
        let peak = tracks.iter().map(|t| t.true_peak_dbtp).fold(f64::NEG_INFINITY, f64::max);
        AlbumLoudness {
            integrated_lufs: integrated(&blocks),
            true_peak_dbtp: if tracks.is_empty() { -200.0 } else { peak },
            tracks,
        }
    }
}

/// ReplayGain 2.0 (reference -18 LUFS, true peak as a linear factor) and
/// the Opus-style R128_* integers (Q7.8 dB relative to -23 LUFS) for DSD.
pub mod replaygain {
    use super::{AlbumLoudness, LoudnessResult};

    pub const REFERENCE_LUFS: f64 = -18.0;
    pub const R128_REFERENCE_LUFS: f64 = -23.0;

    pub const FIELD_NAMES: &[&str] = &[
        "REPLAYGAIN_TRACK_GAIN",
        "REPLAYGAIN_TRACK_PEAK",
        "REPLAYGAIN_ALBUM_GAIN",
        "REPLAYGAIN_ALBUM_PEAK",
        "REPLAYGAIN_REFERENCE_LOUDNESS",
        "R128_TRACK_GAIN",
        "R128_ALBUM_GAIN",
    ];

    pub fn gain_string(lufs: f64) -> String {
        format!("{:.2} dB", REFERENCE_LUFS - lufs)
    }

    pub fn peak_string(dbtp: f64) -> String {
        format!("{:.6}", 10f64.powf(dbtp / 20.0))
    }

    /// Swift's `.rounded()`: half away from zero.
    pub fn r128_gain(lufs: f64) -> String {
        (((R128_REFERENCE_LUFS - lufs) * 256.0).round() as i64).to_string()
    }

    /// (name, value) pairs for one track; album fields when `album` is given.
    pub fn tags(track: &LoudnessResult, album: Option<&AlbumLoudness>, dsd: bool) -> Vec<(String, String)> {
        let mut out = vec![];
        if let Some(lufs) = track.integrated_lufs {
            out.push(("REPLAYGAIN_TRACK_GAIN".into(), gain_string(lufs)));
            out.push(("REPLAYGAIN_TRACK_PEAK".into(), peak_string(track.true_peak_dbtp)));
            if dsd {
                out.push(("R128_TRACK_GAIN".into(), r128_gain(lufs)));
            }
        }
        if let Some(lufs) = album.and_then(|a| a.integrated_lufs) {
            let album = album.unwrap();
            out.push(("REPLAYGAIN_ALBUM_GAIN".into(), gain_string(lufs)));
            out.push(("REPLAYGAIN_ALBUM_PEAK".into(), peak_string(album.true_peak_dbtp)));
            if dsd {
                out.push(("R128_ALBUM_GAIN".into(), r128_gain(lufs)));
            }
        }
        if !out.is_empty() {
            out.push(("REPLAYGAIN_REFERENCE_LOUDNESS".into(), format!("{REFERENCE_LUFS:.2} LUFS")));
        }
        out
    }
}

const SCALE: f32 = 1.0 / 2_147_483_648.0;

/// Decodes files with ffmpeg to 32-bit PCM and meters the stream as it
/// arrives (no temporary file).
pub struct LoudnessAnalyzer {
    pub tool: FFmpegTool,
    pub max_concurrent: usize,
}

impl LoudnessAnalyzer {
    pub fn new(tool: FFmpegTool) -> Self {
        LoudnessAnalyzer { tool, max_concurrent: 2 }
    }

    pub fn analyze(&self, path: &Path) -> Result<LoudnessResult, ToolError> {
        let info = self.tool.probe(path)?;
        let is_dsd = info.codec.to_lowercase().starts_with("dsd") || info.sample_rate > 192_000;
        let rate = if is_dsd { 88_200 } else { info.sample_rate };
        let channels = info.channels.clamp(1, 8);
        let format = PCMFormat::new(rate, channels, 32).map_err(|e| ToolError::BadProbeOutput(e.to_string()))?;
        let mut meter = R128Meter::new(rate, channels as usize);
        let frame_bytes = 4 * channels as usize;
        let mut carry: Vec<u8> = vec![];
        let mut floats: Vec<f32> = vec![];
        self.tool.decode_streaming(path, format, &mut |data| {
            carry.extend_from_slice(data);
            let usable = carry.len() - carry.len() % frame_bytes;
            if usable > 0 {
                floats.clear();
                floats.extend(carry[..usable].as_chunks::<4>().0.iter().map(|b| i32::from_le_bytes(*b) as f32 * SCALE));
                meter.process(&floats);
                carry.drain(..usable);
            }
            Ok(())
        })?;
        Ok(meter.finish())
    }

    /// Album loudness gates over every block of every track; tracks are
    /// measured `max_concurrent` at a time. `progress(done, total)`.
    pub fn analyze_album(
        &self,
        paths: &[PathBuf],
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> Result<AlbumLoudness, ToolError> {
        let results: Mutex<Vec<Option<Result<LoudnessResult, ToolError>>>> =
            Mutex::new((0..paths.len()).map(|_| None).collect());
        let next = Mutex::new(0usize);
        let done = Mutex::new(0usize);
        std::thread::scope(|s| {
            for _ in 0..self.max_concurrent.max(1) {
                s.spawn(|| {
                    loop {
                        let i = {
                            let mut n = next.lock().unwrap();
                            if *n >= paths.len() {
                                return;
                            }
                            *n += 1;
                            *n - 1
                        };
                        let r = self.analyze(&paths[i]);
                        let failed = r.is_err();
                        results.lock().unwrap()[i] = Some(r);
                        let mut d = done.lock().unwrap();
                        *d += 1;
                        progress(*d, paths.len());
                        if failed {
                            *next.lock().unwrap() = paths.len(); // stop handing out work
                        }
                    }
                });
            }
        });
        let mut tracks = vec![];
        for r in results.into_inner().unwrap().into_iter().flatten() {
            tracks.push(r?);
        }
        Ok(AlbumLoudness::new(tracks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, dbfs: f64, seconds: f64, rate: i64, channels: usize) -> Vec<f32> {
        let amp = 10f64.powf(dbfs / 20.0) as f32;
        let n = (seconds * rate as f64) as usize;
        let mut out = vec![0f32; n * channels];
        for i in 0..n {
            let v = amp * (2.0 * PI * hz * i as f64 / rate as f64).sin() as f32;
            out[i * channels..(i + 1) * channels].fill(v);
        }
        out
    }

    fn measure(s: &[f32], rate: i64, channels: usize) -> LoudnessResult {
        let mut m = R128Meter::new(rate, channels);
        m.process(s);
        m.finish()
    }

    #[test]
    fn ebu_calibration_tone() {
        // EBU Tech 3341 case 1: stereo 997 Hz at -23 dBFS reads -23.0 LUFS ±0.1.
        for rate in [48000, 44100, 96000] {
            let r = measure(&sine(997.0, -23.0, 5.0, rate, 2), rate, 2);
            assert!((r.integrated_lufs.unwrap() + 23.0).abs() < 0.1, "rate {rate}: {:?}", r.integrated_lufs);
            assert!((r.true_peak_dbtp + 23.0).abs() < 0.1, "true peak {}", r.true_peak_dbtp);
            assert!((r.sample_peak_dbfs + 23.0).abs() < 0.1);
            assert!((r.duration_seconds - 5.0).abs() < 0.01);
        }
    }

    #[test]
    fn relative_gate_drops_quiet_passages() {
        let rate = 48000;
        let mut s = sine(997.0, -36.0, 10.0, rate, 2);
        s.extend(sine(997.0, -23.0, 20.0, rate, 2));
        s.extend(sine(997.0, -36.0, 10.0, rate, 2));
        assert!((measure(&s, rate, 2).integrated_lufs.unwrap() + 23.0).abs() < 0.1);
        assert_eq!(measure(&vec![0.0; rate as usize * 2], rate, 2).integrated_lufs, None, "silence is gated out");
    }

    #[test]
    fn chunking_does_not_change_the_result() {
        let rate = 44100;
        let s = sine(1234.0, -12.0, 3.0, rate, 2);
        let whole = measure(&s, rate, 2);
        let mut m = R128Meter::new(rate, 2);
        for chunk in s.chunks(2 * 1777) {
            m.process(chunk);
        }
        let parts = m.finish();
        assert_eq!(whole.block_powers.len(), parts.block_powers.len());
        assert!((whole.integrated_lufs.unwrap() - parts.integrated_lufs.unwrap()).abs() < 1e-9);
        assert_eq!(whole.true_peak_dbtp, parts.true_peak_dbtp);
    }

    #[test]
    fn album_gating_and_tags() {
        let rate = 48000;
        let loud = measure(&sine(997.0, -20.0, 4.0, rate, 2), rate, 2);
        let quiet = measure(&sine(997.0, -26.0, 4.0, rate, 2), rate, 2);
        let album = AlbumLoudness::new(vec![loud, quiet.clone()]);
        let lufs = album.integrated_lufs.unwrap();
        assert!(lufs > -26.0 && lufs < -20.0 && (lufs + 22.2).abs() < 0.3, "album {lufs}");
        assert!((album.true_peak_dbtp + 20.0).abs() < 0.1);
        let tags: std::collections::HashMap<String, String> =
            replaygain::tags(&quiet, Some(&album), true).into_iter().collect();
        assert_eq!(tags["REPLAYGAIN_TRACK_GAIN"], "8.00 dB");
        assert!((tags["REPLAYGAIN_TRACK_PEAK"].parse::<f64>().unwrap() - 0.0501187).abs() < 0.0005);
        assert_eq!(tags["R128_TRACK_GAIN"], "768");
        assert!(tags["REPLAYGAIN_ALBUM_GAIN"].ends_with(" dB"));
        assert_eq!(tags["REPLAYGAIN_REFERENCE_LOUDNESS"], "-18.00 LUFS");
    }

    #[test]
    fn five_dot_one_weights_ignore_lfe() {
        let rate = 48000;
        let n = rate as usize * 3;
        let mut lfe = vec![0f32; n * 6];
        for i in 0..n {
            lfe[i * 6 + 3] = 0.5 * (2.0 * PI * 60.0 * i as f64 / rate as f64).sin() as f32;
        }
        assert_eq!(measure(&lfe, rate, 6).integrated_lufs, None);
    }
}
