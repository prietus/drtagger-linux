//! Cross-check against ffmpeg's own ebur128 filter (port of LoudnessKit's
//! "Analyzer vs ffmpeg" test). Runs when the bundled ffmpeg is built
//! (scripts/build-ffmpeg.sh).

use loudnesskit::LoudnessAnalyzer;
use splitkit::FFmpegTool;
use std::f64::consts::PI;
use std::path::PathBuf;
use std::process::Command;

fn tool() -> Option<FFmpegTool> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Vendor/ffmpeg");
    dir.join("ffmpeg").exists().then(|| FFmpegTool::new(dir.join("ffmpeg"), dir.join("ffprobe")))
}

fn wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let pcm: Vec<u8> =
        samples.iter().flat_map(|s| ((s * 32767.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes()).collect();
    let mut d = b"RIFF".to_vec();
    d.extend((36 + pcm.len() as u32).to_le_bytes());
    d.extend(b"WAVEfmt ");
    d.extend(16u32.to_le_bytes());
    d.extend([1, 0, 2, 0]);
    d.extend(rate.to_le_bytes());
    d.extend((rate * 4).to_le_bytes());
    d.extend([4, 0, 16, 0]);
    d.extend(b"data");
    d.extend((pcm.len() as u32).to_le_bytes());
    d.extend(pcm);
    d
}

fn sine(hz: f64, dbfs: f64, seconds: f64, rate: u32) -> Vec<f32> {
    let amp = 10f64.powf(dbfs / 20.0) as f32;
    (0..(seconds * rate as f64) as usize)
        .flat_map(|i| {
            let v = amp * (2.0 * PI * hz * i as f64 / rate as f64).sin() as f32;
            [v, v]
        })
        .collect()
}

#[test]
fn matches_ffmpeg_ebur128() {
    let Some(tool) = tool() else { return };
    let rate = 44100;
    let mut s = sine(997.0, -23.0, 6.0, rate);
    s.extend(sine(3000.0, -14.0, 3.0, rate)); // shelf region: K-weighting matters
    s.extend(sine(60.0, -30.0, 3.0, rate)); // high-pass region
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.wav");
    std::fs::write(&path, wav(&s, rate)).unwrap();

    let ours = LoudnessAnalyzer::new(tool.clone()).analyze(&path).unwrap();
    let out = Command::new(&tool.ffmpeg)
        .args(["-nostats", "-i"])
        .arg(&path)
        .args(["-af", "ebur128=peak=true", "-f", "null", "-"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stderr);
    let summary = text.rsplit("Summary:").next().unwrap();
    let value = |label: &str| -> f64 {
        let rest = &summary[summary.find(label).unwrap() + label.len()..];
        rest.split_whitespace().next().unwrap().parse().unwrap()
    };
    let (theirs, their_peak) = (value("I:"), value("Peak:"));
    assert!(
        (ours.integrated_lufs.unwrap() - theirs).abs() < 0.15,
        "ours {:?} vs ffmpeg {theirs}",
        ours.integrated_lufs
    );
    assert!((ours.true_peak_dbtp - their_peak).abs() < 0.2, "peak {} vs {their_peak}", ours.true_peak_dbtp);
    let album = LoudnessAnalyzer::new(tool).analyze_album(&[path.clone(), path], &|_, _| {}).unwrap();
    assert_eq!(album.tracks.len(), 2);
    assert!((album.integrated_lufs.unwrap() - theirs).abs() < 0.15);
}
