//! Shared helpers for the media tests: fixture generation with real ffmpeg,
//! and measuring what is actually inside an exported file.
#![allow(dead_code)]

use mango_core::*;
use std::path::{Path, PathBuf};
use std::process::Command as Proc;

pub const FFMPEG: &str = "/opt/homebrew/bin/ffmpeg";

pub fn ffmpeg_bin() -> PathBuf {
    mango_media::ffmpeg::ffmpeg_path().expect("ffmpeg is required for these tests")
}
pub fn ffprobe_bin() -> PathBuf {
    mango_media::ffmpeg::ffprobe_path().expect("ffprobe is required for these tests")
}

/// Runs ffmpeg and panics with its stderr if it fails.
pub fn ff(args: &[&str]) {
    let out = Proc::new(ffmpeg_bin()).args(["-hide_banner", "-v", "error", "-nostdin", "-y"]).args(args).output().expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg {:?} failed:\n{}", args, String::from_utf8_lossy(&out.stderr));
}

/// Runs ffmpeg and returns its stdout.
pub fn ff_out(args: &[&str]) -> Vec<u8> {
    let out = Proc::new(ffmpeg_bin()).args(["-hide_banner", "-v", "error", "-nostdin"]).args(args).output().expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg {:?} failed:\n{}", args, String::from_utf8_lossy(&out.stderr));
    out.stdout
}

pub fn ffprobe_json(path: &Path, extra: &[&str]) -> serde_json::Value {
    let out = Proc::new(ffprobe_bin())
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .args(extra)
        .arg(path)
        .output()
        .expect("run ffprobe");
    assert!(out.status.success(), "ffprobe failed on {}:\n{}", path.display(), String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("ffprobe json")
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Directory for generated fixtures, shared by all test binaries and kept
/// between runs (inside the cargo target directory).
pub fn fixture_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fixtures-v3");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Returns `fixture_dir()/name`, generating it with `make(tmp_path)` if it
/// does not exist yet. Generation writes to a temporary name and renames,
/// so parallel tests and test binaries never see a half-written file.
pub fn fixture(name: &str, make: impl FnOnce(&Path)) -> PathBuf {
    let path = fixture_dir().join(name);
    if path.is_file() {
        return path;
    }
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = fixture_dir().join(format!(".tmp-{}-{:?}-{name}.{ext}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
    make(&tmp);
    assert!(tmp.is_file(), "fixture generator did not write {}", tmp.display());
    std::fs::rename(&tmp, &path).unwrap();
    path
}

pub const CODED_W: u32 = 64;
pub const CODED_H: u32 = 36;
pub const CODED_FPS: f64 = 30.0;
/// Length of each coded unit ("word") in the coded source.
pub const UNIT: f64 = 0.5;
const LEVELS: [u8; 5] = [30, 80, 130, 180, 230];

/// The solid colour shown during unit `k` of the coded source.
pub fn unit_color(k: usize) -> [u8; 3] {
    let k = k % 125;
    [LEVELS[k % 5], LEVELS[(k / 5) % 5], LEVELS[(k / 25) % 5]]
}

/// The tone frequency (Hz) heard during unit `k` of the coded source.
pub fn unit_freq(k: usize) -> f64 {
    300.0 + 40.0 * k as f64
}

/// Which unit a decoded colour belongs to, if it is close to one.
pub fn unit_of_color(rgb: [u8; 3]) -> Option<usize> {
    let mut idx = [0usize; 3];
    for c in 0..3 {
        let (best, dist) = LEVELS.iter().enumerate().map(|(i, l)| (i, (*l as i32 - rgb[c] as i32).abs())).min_by_key(|x| x.1).unwrap();
        if dist > 14 {
            return None;
        }
        idx[c] = best;
    }
    Some(idx[0] + idx[1] * 5 + idx[2] * 25)
}

/// Which unit a measured frequency belongs to, if it is close to one.
pub fn unit_of_freq(f: f64) -> Option<usize> {
    let k = ((f - 300.0) / 40.0).round();
    if k < 0.0 || (unit_freq(k as usize) - f).abs() > 12.0 { None } else { Some(k as usize) }
}

fn write_wav_mono(path: &Path, rate: u32, samples: &[f32]) {
    let mut data = Vec::with_capacity(44 + samples.len() * 2);
    let bytes = (samples.len() * 2) as u32;
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36 + bytes).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&rate.to_le_bytes());
    data.extend_from_slice(&(rate * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&bytes.to_le_bytes());
    for s in samples {
        data.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, data).unwrap();
}

/// A wav holding `units` tones of `UNIT` seconds each, unit k at `unit_freq(k)`.
fn coded_wav(path: &Path, units: usize) {
    let rate = 48_000u32;
    let per = (UNIT * rate as f64) as usize;
    let mut samples = Vec::with_capacity(units * per);
    for k in 0..units {
        let f = unit_freq(k);
        for i in 0..per {
            samples.push((0.5 * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin()) as f32);
        }
    }
    write_wav_mono(path, rate, &samples);
}

/// The coded source: `units` half-second units; during unit k every pixel is
/// `unit_color(k)` and the audio is a `unit_freq(k)` sine. Looking at any
/// output frame or listening to any output moment therefore tells exactly
/// which part of the source is playing. H.264 + AAC in MP4, 64x36 @ 30 fps.
pub fn coded_source(units: usize) -> PathBuf {
    fixture(&format!("coded_{units}.mp4"), |out| {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("v.rgb");
        let wav = dir.path().join("a.wav");
        let frames_per_unit = (UNIT * CODED_FPS) as usize;
        let mut bytes = Vec::with_capacity(units * frames_per_unit * (CODED_W * CODED_H * 3) as usize);
        for k in 0..units {
            let c = unit_color(k);
            for _ in 0..frames_per_unit {
                for _ in 0..CODED_W * CODED_H {
                    bytes.extend_from_slice(&c);
                }
            }
        }
        std::fs::write(&raw, bytes).unwrap();
        coded_wav(&wav, units);
        ff(&[
            "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &format!("{CODED_W}x{CODED_H}"), "-r", "30", "-i", raw.to_str().unwrap(),
            "-i", wav.to_str().unwrap(),
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "10", "-g", "15", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-shortest", "-f", "mp4", out.to_str().unwrap(),
        ]);
    })
}

/// The coded source re-encoded with other container/codec settings:
/// `ffmpeg -i coded_N.mp4 <args> out`.
pub fn coded_variant(units: usize, name: &str, args: &[&str]) -> PathBuf {
    let src = coded_source(units);
    fixture(name, |out| {
        let mut a: Vec<&str> = vec!["-i", src.to_str().unwrap()];
        a.extend_from_slice(args);
        let o = out.to_str().unwrap().to_string();
        a.push(&o);
        ff(&a);
    })
}

/// Audio-only version of the coded source (wav).
pub fn coded_audio(units: usize) -> PathBuf {
    fixture(&format!("coded_{units}.wav"), |out| coded_wav(out, units))
}

/// A lavfi-generated file: `ffmpeg <args> out`.
pub fn lavfi(name: &str, args: &[&str]) -> PathBuf {
    fixture(name, |out| {
        let mut a: Vec<&str> = args.to_vec();
        let o = out.to_str().unwrap().to_string();
        a.push(&o);
        ff(&a);
    })
}

/// A solid-colour video clip with a steady tone.
pub fn solid_clip(name: &str, color: &str, w: u32, h: u32, secs: f64, tone_hz: u32) -> PathBuf {
    let v = format!("color=c={color}:s={w}x{h}:r=30:d={secs}");
    let a = format!("sine=f={tone_hz}:d={secs}:r=48000");
    lavfi(name, &["-f", "lavfi", "-i", &v, "-f", "lavfi", "-i", &a, "-c:v", "libx264", "-crf", "12", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k", "-shortest", "-f", "mp4"])
}

/// A solid-colour PNG.
pub fn solid_png(name: &str, color: &str, w: u32, h: u32) -> PathBuf {
    let v = format!("color=c={color}:s={w}x{h}");
    lavfi(name, &["-f", "lavfi", "-i", &v, "-frames:v", "1", "-update", "1", "-f", "image2", "-c:v", "png"])
}

/// A steady tone as a wav file.
pub fn tone_wav(name: &str, hz: u32, secs: f64, volume: f64) -> PathBuf {
    let a = format!("sine=f={hz}:d={secs}:r=48000,volume={volume}");
    lavfi(name, &["-f", "lavfi", "-i", &a, "-c:a", "pcm_s16le", "-f", "wav"])
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

/// A transcript with one word per coded unit: "w0", "w1", …
pub fn unit_transcript(units: usize) -> Transcript {
    Transcript {
        language: "en".into(),
        words: (0..units)
            .map(|k| TranscriptWord { text: format!("w{k}"), start: k as f64 * UNIT, end: (k + 1) as f64 * UNIT, speaker: None, confidence: 1.0 })
            .collect(),
    }
}

/// Probes `path`, gives it `transcript`, and returns an editor with the asset
/// on the script track of a fresh composition.
pub fn editor_with_source(path: &Path, transcript: Option<Transcript>) -> (Editor, AssetId) {
    let mut asset = mango_media::probe::probe(path).expect("probe source");
    asset.transcript = transcript;
    let id = asset.id;
    let mut e = Editor::new_project("Test");
    e.apply(Command::AddAsset { asset }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    (e, id)
}

/// The coded source with `units` units on the script track, one word each.
pub fn coded_editor(units: usize) -> (Editor, AssetId) {
    editor_with_source(&coded_source(units), Some(unit_transcript(units)))
}

/// Adds a file to the project and returns its asset id.
pub fn add_asset(e: &mut Editor, path: &Path) -> AssetId {
    let asset = mango_media::probe::probe(path).expect("probe asset");
    let id = asset.id;
    e.apply(Command::AddAsset { asset }).unwrap();
    id
}

pub fn word_index(e: &Editor, text: &str) -> usize {
    e.comp().unwrap().script.iter().position(|t| t.is_word() && t.text == text).unwrap_or_else(|| panic!("no word {text}"))
}

pub fn settings(e: &Editor) -> mango_core::render::ExportSettings {
    mango_core::render::ExportSettings::for_composition(e.comp().unwrap())
}

/// Runs an export, panicking with the full error chain on failure.
pub fn export_to(e: &Editor, s: &mango_core::render::ExportSettings, out: &Path) -> mango_media::export::Report {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    match mango_media::export::export(&e.project, e.comp().unwrap(), s, out, &mut |_| {}, &cancel) {
        Ok(r) => r,
        Err(err) => panic!("export failed: {err:#}"),
    }
}

// ---------------------------------------------------------------------------
// Measuring outputs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Info {
    pub format: String,
    pub duration: f64,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frames: u64,
    pub pix_fmt: String,
    pub audio_duration: f64,
    pub sample_rate: u32,
    pub channels: u32,
    pub chapters: Vec<(f64, f64, String)>,
    pub title: Option<String>,
}

fn num(v: &serde_json::Value) -> f64 {
    v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()).unwrap_or(0.0)
}

fn rate(v: &serde_json::Value) -> f64 {
    let s = v.as_str().unwrap_or("0/1");
    let (a, b) = s.split_once('/').unwrap_or((s, "1"));
    let (a, b) = (a.parse::<f64>().unwrap_or(0.0), b.parse::<f64>().unwrap_or(1.0));
    if b > 0.0 { a / b } else { 0.0 }
}

/// Everything ffprobe says about a file, with decoded frame counts.
pub fn info(path: &Path) -> Info {
    let j = ffprobe_json(path, &["-count_frames", "-show_chapters"]);
    let streams = j["streams"].as_array().cloned().unwrap_or_default();
    let v = streams.iter().find(|s| s["codec_type"] == "video");
    let a = streams.iter().find(|s| s["codec_type"] == "audio");
    let chapters = j["chapters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| (num(&c["start_time"]), num(&c["end_time"]), c["tags"]["title"].as_str().unwrap_or("").to_string()))
        .collect();
    Info {
        format: j["format"]["format_name"].as_str().unwrap_or("").to_string(),
        duration: num(&j["format"]["duration"]),
        vcodec: v.map(|s| s["codec_name"].as_str().unwrap_or("").to_string()),
        acodec: a.map(|s| s["codec_name"].as_str().unwrap_or("").to_string()),
        width: v.map(|s| s["width"].as_u64().unwrap_or(0) as u32).unwrap_or(0),
        height: v.map(|s| s["height"].as_u64().unwrap_or(0) as u32).unwrap_or(0),
        fps: v.map(|s| rate(&s["avg_frame_rate"])).unwrap_or(0.0),
        frames: v.map(|s| num(&s["nb_read_frames"]) as u64).unwrap_or(0),
        pix_fmt: v.map(|s| s["pix_fmt"].as_str().unwrap_or("").to_string()).unwrap_or_default(),
        audio_duration: a.map(|s| num(&s["duration"])).unwrap_or(0.0),
        sample_rate: a.map(|s| num(&s["sample_rate"]) as u32).unwrap_or(0),
        channels: a.map(|s| s["channels"].as_u64().unwrap_or(0) as u32).unwrap_or(0),
        chapters,
        title: j["format"]["tags"]["title"].as_str().map(|s| s.to_string()),
    }
}

/// All frames of a video decoded to RGB24 at its own size.
pub struct Video {
    pub w: usize,
    pub h: usize,
    pub frames: Vec<Vec<u8>>,
}

impl Video {
    pub fn pixel(&self, frame: usize, x: usize, y: usize) -> [u8; 3] {
        let f = &self.frames[frame];
        let i = (y * self.w + x) * 3;
        [f[i], f[i + 1], f[i + 2]]
    }
    /// Pixel at fractional canvas coordinates.
    pub fn at(&self, frame: usize, fx: f64, fy: f64) -> [u8; 3] {
        let x = ((fx * self.w as f64) as usize).min(self.w - 1);
        let y = ((fy * self.h as f64) as usize).min(self.h - 1);
        self.pixel(frame, x, y)
    }
    /// Mean colour of a frame.
    pub fn mean(&self, frame: usize) -> [u8; 3] {
        let f = &self.frames[frame];
        let mut s = [0u64; 3];
        for p in f.chunks_exact(3) {
            for c in 0..3 {
                s[c] += p[c] as u64;
            }
        }
        let n = (f.len() / 3) as u64;
        [(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8]
    }
    /// Number of pixels inside the fractional rectangle for which `pred` holds.
    pub fn count_in(&self, frame: usize, x0: f64, y0: f64, x1: f64, y1: f64, pred: impl Fn([u8; 3]) -> bool) -> usize {
        let (xa, xb) = ((x0 * self.w as f64).round() as usize, ((x1 * self.w as f64).round() as usize).min(self.w));
        let (ya, yb) = ((y0 * self.h as f64).round() as usize, ((y1 * self.h as f64).round() as usize).min(self.h));
        let mut n = 0;
        for y in ya..yb {
            for x in xa..xb {
                if pred(self.pixel(frame, x, y)) {
                    n += 1;
                }
            }
        }
        n
    }
    /// Bounding box (x0, y0, x1, y1 in pixels, inclusive) of pixels matching `pred`.
    pub fn bbox(&self, frame: usize, pred: impl Fn([u8; 3]) -> bool) -> Option<(usize, usize, usize, usize)> {
        let mut b: Option<(usize, usize, usize, usize)> = None;
        for y in 0..self.h {
            for x in 0..self.w {
                if pred(self.pixel(frame, x, y)) {
                    b = Some(match b {
                        None => (x, y, x, y),
                        Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(x), e.max(y)),
                    });
                }
            }
        }
        b
    }
}

/// Decodes every frame of the first video stream.
pub fn decode_video(path: &Path) -> Video {
    let i = info(path);
    let (w, h) = (i.width as usize, i.height as usize);
    assert!(w > 0 && h > 0, "{} has no video", path.display());
    let raw = ff_out(&["-i", path.to_str().unwrap(), "-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"]);
    let size = w * h * 3;
    assert_eq!(raw.len() % size, 0, "decoded video is not a whole number of frames");
    Video { w, h, frames: raw.chunks_exact(size).map(|c| c.to_vec()).collect() }
}

pub const RATE: usize = 48_000;

/// Decodes the first audio stream to mono f32 at 48 kHz.
pub fn decode_audio(path: &Path) -> Vec<f32> {
    // The plain average of left and right (ffmpeg's own downmix is not);
    // mono files are read as they are.
    let j = ffprobe_json(path, &[]);
    let channels = j["streams"].as_array().and_then(|s| s.iter().find(|s| s["codec_type"] == "audio")).and_then(|s| s["channels"].as_u64()).unwrap_or(2);
    if channels == 1 {
        let raw = ff_out(&["-i", path.to_str().unwrap(), "-map", "0:a:0", "-ar", "48000", "-f", "f32le", "pipe:1"]);
        return raw.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    }
    let raw = ff_out(&["-i", path.to_str().unwrap(), "-map", "0:a:0", "-ac", "2", "-ar", "48000", "-f", "f32le", "pipe:1"]);
    raw.chunks_exact(8).map(|c| (f32::from_le_bytes([c[0], c[1], c[2], c[3]]) + f32::from_le_bytes([c[4], c[5], c[6], c[7]])) / 2.0).collect()
}

/// Decodes one channel (0 = left) of the first audio stream at 48 kHz.
pub fn decode_audio_channel(path: &Path, channel: usize) -> Vec<f32> {
    let raw = ff_out(&["-i", path.to_str().unwrap(), "-map", "0:a:0", "-ac", "2", "-ar", "48000", "-f", "f32le", "pipe:1"]);
    raw.chunks_exact(8).map(|c| { let o = channel * 4; f32::from_le_bytes([c[o], c[o + 1], c[o + 2], c[o + 3]]) }).collect()
}

fn span(samples: &[f32], t0: f64, t1: f64) -> &[f32] {
    let a = ((t0 * RATE as f64) as usize).min(samples.len());
    let b = ((t1 * RATE as f64) as usize).min(samples.len());
    &samples[a..b]
}

pub fn rms(samples: &[f32], t0: f64, t1: f64) -> f64 {
    let s = span(samples, t0, t1);
    if s.is_empty() {
        return 0.0;
    }
    (s.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
}

/// Amplitude of the `hz` component in `[t0, t1)` (Goertzel-style correlation);
/// a pure sine of peak amplitude A gives A.
pub fn tone_level(samples: &[f32], hz: f64, t0: f64, t1: f64) -> f64 {
    let s = span(samples, t0, t1);
    if s.is_empty() {
        return 0.0;
    }
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, v) in s.iter().enumerate() {
        let ph = 2.0 * std::f64::consts::PI * hz * i as f64 / RATE as f64;
        re += *v as f64 * ph.cos();
        im += *v as f64 * ph.sin();
    }
    2.0 * (re * re + im * im).sqrt() / s.len() as f64
}

/// Dominant frequency in `[t0, t1)` from zero crossings (for clean tones).
pub fn freq(samples: &[f32], t0: f64, t1: f64) -> f64 {
    let s = span(samples, t0, t1);
    let mut first = None;
    let mut last = None;
    let mut n = 0usize;
    for i in 1..s.len() {
        if s[i - 1] <= 0.0 && s[i] > 0.0 {
            // Linear interpolation of the crossing position.
            let frac = -s[i - 1] as f64 / (s[i] - s[i - 1]) as f64;
            let pos = (i - 1) as f64 + frac;
            if first.is_none() {
                first = Some(pos);
            } else {
                n += 1;
            }
            last = Some(pos);
        }
    }
    match (first, last) {
        (Some(a), Some(b)) if n > 0 && b > a => n as f64 * RATE as f64 / (b - a),
        _ => 0.0,
    }
}

/// The coded unit heard in `[t0, t1)`.
pub fn unit_heard(samples: &[f32], t0: f64, t1: f64) -> Option<usize> {
    unit_of_freq(freq(samples, t0, t1))
}

pub fn close(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= tol)
}

pub fn assert_color(actual: [u8; 3], expected: [u8; 3], tol: i32, what: &str) {
    assert!(close(actual, expected, tol), "{what}: expected about {expected:?}, got {actual:?}");
}

/// Files and folders in `dir` (names only), sorted.
pub fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}
