//! Drives the ffmpeg / ffprobe executables. Every call spawns one process;
//! stdin is fed and stdout/stderr drained on their own threads so pipes
//! never deadlock, and a `Cancel` token kills running children.

use crate::pcm::{PCMFormat, crc32_of_file};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolError {
    LaunchFailed(String),
    Failed { command: String, status: i32, stderr: String },
    BadProbeOutput(String),
    NoAudioStream(PathBuf),
    OutputMisaligned { bytes: i64, bytes_per_frame: i64 },
    Cancelled,
    Io(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolError::LaunchFailed(m) => write!(f, "Could not launch ffmpeg: {m}"),
            ToolError::Failed { command, status, stderr } => {
                let lines: Vec<&str> = stderr.lines().collect();
                let tail = lines[lines.len().saturating_sub(3)..].join(" ");
                write!(f, "{command} failed (exit {status}): {tail}")
            }
            ToolError::BadProbeOutput(m) => write!(f, "ffprobe output could not be parsed: {m}"),
            ToolError::NoAudioStream(p) => {
                write!(f, "{} has no audio stream.", p.file_name().unwrap_or_default().to_string_lossy())
            }
            ToolError::OutputMisaligned { bytes, bytes_per_frame } => {
                write!(f, "Decoded {bytes} bytes, not a multiple of {bytes_per_frame} bytes per frame.")
            }
            ToolError::Cancelled => write!(f, "Cancelled."),
            ToolError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(e: std::io::Error) -> Self {
        ToolError::Io(e.to_string())
    }
}

/// Stops the work it is attached to: running children are terminated and
/// later calls fail with `Cancelled`.
#[derive(Default)]
pub struct Cancel {
    flag: AtomicBool,
    children: Mutex<HashSet<u32>>,
}

impl Cancel {
    pub fn new() -> Arc<Cancel> {
        Arc::new(Cancel::default())
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        for pid in self.children.lock().unwrap().iter() {
            // SAFETY: plain kill(2) on a child we spawned and still track.
            unsafe { libc::kill(*pid as i32, libc::SIGTERM) };
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub codec: String,
    pub sample_rate: i64,
    pub channels: i64,
    pub bits_per_sample: i64,
    pub duration_seconds: Option<f64>,
}

impl StreamInfo {
    /// The PCM the splitter decodes to: 16, 24 or 32 bits.
    pub fn pcm_format(&self) -> Result<PCMFormat, crate::pcm::FormatError> {
        let bits = match self.bits_per_sample {
            ..=16 => 16,
            17..=24 => 24,
            _ => 32,
        };
        PCMFormat::new(self.sample_rate, self.channels, bits)
    }
}

/// What was fed to an encoder: byte count, MD5 (comparable with FLAC's
/// STREAMINFO signature) and CRC-32.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedDigest {
    pub byte_count: i64,
    pub md5: [u8; 16],
    pub crc32: u32,
}

impl FeedDigest {
    pub fn md5_hex(&self) -> String {
        self.md5.iter().map(|b| format!("{b:02x}")).collect()
    }
}

struct Output {
    status: i32,
    stdout: Vec<u8>,
    stderr: String,
}

type Feeder = Box<dyn FnOnce(&mut dyn Write) -> std::io::Result<()> + Send>;

#[derive(Clone)]
pub struct FFmpegTool {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    cancel: Option<Arc<Cancel>>,
}

fn path_arg(p: &Path) -> std::ffi::OsString {
    p.as_os_str().to_os_string()
}

impl FFmpegTool {
    pub fn new(ffmpeg: PathBuf, ffprobe: PathBuf) -> Self {
        FFmpegTool { ffmpeg, ffprobe, cancel: None }
    }

    pub fn with_cancel(mut self, cancel: Arc<Cancel>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    fn check_cancel(&self) -> Result<(), ToolError> {
        if self.cancel.as_ref().is_some_and(|c| c.is_cancelled()) { Err(ToolError::Cancelled) } else { Ok(()) }
    }

    fn command(&self, exe: &Path, args: &[std::ffi::OsString], low_priority: bool) -> Command {
        let mut cmd = Command::new(exe);
        cmd.args(args);
        if low_priority {
            // Background work: like the Mac app's utility QoS.
            // SAFETY: setpriority is async-signal-safe.
            unsafe {
                cmd.pre_exec(|| {
                    libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                    Ok(())
                });
            }
        }
        cmd
    }

    fn track(&self, pid: u32, add: bool) {
        if let Some(c) = &self.cancel {
            let mut set = c.children.lock().unwrap();
            if add {
                set.insert(pid);
            } else {
                set.remove(&pid);
            }
        }
    }

    fn run(&self, exe: &Path, args: &[std::ffi::OsString], feeder: Option<Feeder>) -> Result<Output, ToolError> {
        self.check_cancel()?;
        let mut cmd = self.command(exe, args, false);
        cmd.stdin(if feeder.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| ToolError::LaunchFailed(e.to_string()))?;
        self.track(child.id(), true);
        let feed = match (feeder, child.stdin.take()) {
            (Some(f), Some(mut stdin)) => Some(std::thread::spawn(move || f(&mut stdin))),
            _ => None,
        };
        let mut stderr = child.stderr.take().unwrap();
        let err = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = stderr.read_to_end(&mut s);
            s
        });
        let mut stdout = Vec::new();
        let _ = child.stdout.take().unwrap().read_to_end(&mut stdout);
        let status = child.wait()?;
        self.track(child.id(), false);
        let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
        // EPIPE when ffmpeg exits early is reported through its exit status.
        let feed_result = feed.map(|h| h.join().unwrap_or(Ok(())));
        self.check_cancel()?;
        if status.success()
            && let Some(Err(e)) = feed_result
        {
            return Err(e.into());
        }
        Ok(Output { status: status.code().unwrap_or(-1), stdout, stderr })
    }

    fn failed(command: &str, out: &Output) -> ToolError {
        ToolError::Failed { command: command.into(), status: out.status, stderr: out.stderr.clone() }
    }

    pub fn probe(&self, url: &Path) -> Result<StreamInfo, ToolError> {
        let args: Vec<std::ffi::OsString> = [
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,sample_rate,channels,bits_per_sample,bits_per_raw_sample,duration",
            "-of",
            "json",
        ]
        .iter()
        .map(Into::into)
        .chain([path_arg(url)])
        .collect();
        let out = self.run(&self.ffprobe, &args, None)?;
        if out.status != 0 {
            return Err(Self::failed("ffprobe", &out));
        }
        Self::parse_probe(&out.stdout, url)
    }

    pub fn parse_probe(data: &[u8], url: &Path) -> Result<StreamInfo, ToolError> {
        let bad = || ToolError::BadProbeOutput(String::from_utf8_lossy(&data[..data.len().min(200)]).into_owned());
        let root: Value = serde_json::from_slice(data).map_err(|_| bad())?;
        let streams = root.get("streams").and_then(Value::as_array).ok_or_else(bad)?;
        let s = streams.first().ok_or_else(|| ToolError::NoAudioStream(url.to_path_buf()))?;
        let int = |k: &str| match s.get(k) {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(v)) => v.parse().ok(),
            _ => None,
        };
        let double = |k: &str| match s.get(k) {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(v)) => v.parse().ok(),
            _ => None,
        };
        let (Some(rate), Some(channels)) = (int("sample_rate").filter(|r| *r > 0), int("channels").filter(|c| *c > 0))
        else {
            return Err(ToolError::BadProbeOutput("missing sample_rate / channels".into()));
        };
        let raw = int("bits_per_raw_sample").unwrap_or(0);
        let bps = int("bits_per_sample").unwrap_or(0);
        Ok(StreamInfo {
            codec: s.get("codec_name").and_then(Value::as_str).unwrap_or("").to_string(),
            sample_rate: rate,
            channels,
            bits_per_sample: if raw > 0 {
                raw
            } else if bps > 0 {
                bps
            } else {
                16
            },
            duration_seconds: double("duration"),
        })
    }

    /// Container tags as ffprobe reports them (format-level over the first
    /// audio stream's), keys upper-cased.
    pub fn probe_tags(&self, url: &Path) -> Result<BTreeMap<String, String>, ToolError> {
        let args: Vec<std::ffi::OsString> =
            ["-v", "error", "-select_streams", "a:0", "-show_entries", "format_tags:stream_tags", "-of", "json"]
                .iter()
                .map(Into::into)
                .chain([path_arg(url)])
                .collect();
        let out = self.run(&self.ffprobe, &args, None)?;
        if out.status != 0 {
            return Err(Self::failed("ffprobe tags", &out));
        }
        let root: Value = serde_json::from_slice(&out.stdout).map_err(|_| ToolError::BadProbeOutput("tags".into()))?;
        let mut tags = BTreeMap::new();
        let mut take = |obj: Option<&Value>| {
            if let Some(map) = obj.and_then(Value::as_object) {
                for (k, v) in map {
                    let s = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                    tags.insert(k.to_uppercase(), s);
                }
            }
        };
        take(root.get("streams").and_then(|s| s.get(0)).and_then(|s| s.get("tags")));
        take(root.get("format").and_then(|f| f.get("tags")));
        Ok(tags)
    }

    /// `seconds` of 16-bit PCM from `start`, down-mixed to at most two
    /// channels: chromaprint's input.
    pub fn decode_snippet(&self, url: &Path, start: f64, seconds: f64) -> Result<(Vec<i16>, i64, i64), ToolError> {
        let info = self.probe(url)?;
        let channels = info.channels.clamp(1, 2);
        let mut args: Vec<std::ffi::OsString> = vec!["-v".into(), "error".into(), "-nostdin".into()];
        if start > 0.0 {
            args.extend(["-ss".into(), format!("{start:.3}").into()]);
        }
        args.extend(["-i".into(), path_arg(url)]);
        for a in ["-t", &format!("{seconds:.3}"), "-map", "0:a:0", "-vn", "-ac", &channels.to_string()] {
            args.push(a.into());
        }
        for a in ["-f", "s16le", "-acodec", "pcm_s16le", "pipe:1"] {
            args.push(a.into());
        }
        let out = self.run(&self.ffmpeg, &args, None)?;
        if out.status != 0 {
            return Err(Self::failed("ffmpeg snippet", &out));
        }
        let samples = out.stdout.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect();
        Ok((samples, info.sample_rate, channels))
    }

    fn decode_args(
        url: &Path,
        format: PCMFormat,
        output: std::ffi::OsString,
        overwrite: bool,
    ) -> Vec<std::ffi::OsString> {
        let mut args: Vec<std::ffi::OsString> = vec!["-v".into(), "error".into(), "-nostdin".into()];
        if overwrite {
            args.push("-y".into());
        }
        args.extend(["-i".into(), path_arg(url)]);
        for a in [
            "-map".to_string(),
            "0:a:0".into(),
            "-vn".into(),
            "-f".into(),
            format.ffmpeg_format_name(),
            "-acodec".into(),
            format.ffmpeg_codec_name(),
            "-ar".into(),
            format.sample_rate.to_string(),
            "-ac".into(),
            format.channels.to_string(),
        ] {
            args.push(a.into());
        }
        args.push(output);
        args
    }

    /// Decodes the whole file to raw interleaved PCM; returns the frame count.
    pub fn decode_to_raw(&self, url: &Path, format: PCMFormat, output: &Path) -> Result<i64, ToolError> {
        let out = self.run(&self.ffmpeg, &Self::decode_args(url, format, path_arg(output), true), None)?;
        if out.status != 0 {
            return Err(Self::failed("ffmpeg decode", &out));
        }
        let size = std::fs::metadata(output).map_or(0, |m| m.len() as i64);
        if size % format.bytes_per_frame() != 0 {
            return Err(ToolError::OutputMisaligned { bytes: size, bytes_per_frame: format.bytes_per_frame() });
        }
        Ok(size / format.bytes_per_frame())
    }

    /// Decodes the whole file and hands the PCM over in chunks as ffmpeg
    /// produces it (nothing goes to disk); ffmpeg runs at low priority. A
    /// failing consumer stops the process. Returns the byte count.
    pub fn decode_streaming(
        &self,
        url: &Path,
        format: PCMFormat,
        consume: &mut dyn FnMut(&[u8]) -> Result<(), ToolError>,
    ) -> Result<i64, ToolError> {
        self.check_cancel()?;
        let mut cmd = self.command(&self.ffmpeg, &Self::decode_args(url, format, "pipe:1".into(), false), true);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| ToolError::LaunchFailed(e.to_string()))?;
        self.track(child.id(), true);
        let mut stderr = child.stderr.take().unwrap();
        let err = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = stderr.read_to_end(&mut s);
            s
        });
        let mut stdout = child.stdout.take().unwrap();
        let mut buf = vec![0u8; 1 << 20];
        let mut bytes = 0i64;
        let mut failure = None;
        loop {
            let n = stdout.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if failure.is_some() {
                continue; // drain so the child can exit
            }
            bytes += n as i64;
            if let Err(e) = consume(&buf[..n]) {
                failure = Some(e);
                let _ = child.kill();
            }
        }
        let status = child.wait()?;
        self.track(child.id(), false);
        let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
        self.check_cancel()?;
        if let Some(e) = failure {
            return Err(e);
        }
        if !status.success() {
            return Err(ToolError::Failed {
                command: "ffmpeg decode".into(),
                status: status.code().unwrap_or(-1),
                stderr,
            });
        }
        Ok(bytes)
    }

    /// Encodes a byte range of a raw PCM file to FLAC through ffmpeg's
    /// stdin; the digest covers exactly the bytes fed.
    pub fn encode_flac(
        &self,
        raw_pcm: &Path,
        start: i64,
        end: i64,
        format: PCMFormat,
        compression_level: i64,
        output: &Path,
    ) -> Result<FeedDigest, ToolError> {
        let mut args: Vec<std::ffi::OsString> = vec![];
        for a in [
            "-v".to_string(),
            "error".into(),
            "-y".into(),
            "-f".into(),
            format.ffmpeg_format_name(),
            "-ar".into(),
            format.sample_rate.to_string(),
            "-ac".into(),
            format.channels.to_string(),
            "-i".into(),
            "pipe:0".into(),
            "-map".into(),
            "0:a:0".into(),
            "-c:a".into(),
            "flac".into(),
            "-compression_level".into(),
            compression_level.clamp(0, 12).to_string(),
            "-f".into(),
            "flac".into(),
        ] {
            args.push(a.into());
        }
        args.push(path_arg(output));
        let digest = Arc::new(Mutex::new(None));
        let sink = digest.clone();
        let raw = raw_pcm.to_path_buf();
        let feeder: Feeder = Box::new(move |stdin| {
            let mut file = File::open(&raw)?;
            file.seek(SeekFrom::Start(start as u64))?;
            let mut remaining = (end - start).max(0) as u64;
            let (mut md5, mut crc, mut fed) = (Md5::new(), crc32fast::Hasher::new(), 0i64);
            let mut buf = vec![0u8; 1 << 20];
            while remaining > 0 {
                let n = {
                    let take = remaining.min(buf.len() as u64) as usize;
                    file.read(&mut buf[..take])?
                };
                if n == 0 {
                    break;
                }
                md5.update(&buf[..n]);
                crc.update(&buf[..n]);
                stdin.write_all(&buf[..n])?;
                fed += n as i64;
                remaining -= n as u64;
            }
            *sink.lock().unwrap() =
                Some(FeedDigest { byte_count: fed, md5: md5.finalize().into(), crc32: crc.finalize() });
            Ok(())
        });
        let out = self.run(&self.ffmpeg, &args, Some(feeder))?;
        if out.status != 0 {
            return Err(Self::failed("ffmpeg flac", &out));
        }
        digest.lock().unwrap().take().ok_or_else(|| ToolError::Failed {
            command: "ffmpeg flac".into(),
            status: -1,
            stderr: "stdin feeder produced no digest".into(),
        })
    }

    /// CRC-32 and frame count of any audio file's PCM in `format`: the
    /// fallback verification for encoded tracks. `work` holds the temporary
    /// decode.
    pub fn raw_checksum(&self, url: &Path, format: PCMFormat, work: &Path) -> Result<(u32, i64), ToolError> {
        let tmp = tempfile::Builder::new().prefix("drtagger-verify-").suffix(".raw").tempfile_in(work)?;
        let samples = self.decode_to_raw(url, format, tmp.path())?;
        let crc = crc32_of_file(tmp.path(), 0, samples * format.bytes_per_frame())?;
        Ok((crc, samples))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_probe_json() {
        let json = br#"{"streams":[{"codec_name":"flac","sample_rate":"44100","channels":2,"bits_per_raw_sample":"24","duration":"12.5"}]}"#;
        let info = FFmpegTool::parse_probe(json, Path::new("x.flac")).unwrap();
        assert_eq!((info.sample_rate, info.channels, info.bits_per_sample), (44100, 2, 24));
        assert_eq!(info.duration_seconds, Some(12.5));
        assert_eq!(info.pcm_format().unwrap().bits_per_sample, 24);
        assert!(matches!(
            FFmpegTool::parse_probe(br#"{"streams":[]}"#, Path::new("x.flac")),
            Err(ToolError::NoAudioStream(_))
        ));
    }
}
