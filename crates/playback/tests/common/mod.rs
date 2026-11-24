//! Shared helpers: real media generated with ffmpeg, measurement tools, and
//! projects built through `mango_core::Editor`.
#![allow(dead_code)]

use mango_core::*;
use mango_playback::video::VideoFrame;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

pub const SR: usize = 48_000;

pub fn tool(name: &str) -> PathBuf {
    mango_core::tools::find_tool(name).unwrap_or_else(|| panic!("{name} is required for these tests"))
}

/// Runs ffmpeg with `args`, panicking with its output on failure.
pub fn ffmpeg(args: &[&str]) {
    let out = Process::new(tool("ffmpeg")).args(["-v", "error", "-nostdin", "-y"]).args(args).output().expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// Directory for generated media, shared by the tests of one test binary and kept between runs.
pub fn media_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mango-playback-media-v3");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Generates `name` once (also safe when several test processes race).
pub fn generated(name: &str, make: impl FnOnce(&Path)) -> PathBuf {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = media_dir().join(name);
    if !path.exists() {
        let ext = path.extension().unwrap().to_str().unwrap().to_string();
        let tmp = media_dir().join(format!("tmp-{}-{name}.{ext}", std::process::id()));
        make(&tmp);
        std::fs::rename(&tmp, &path).unwrap();
    }
    path
}

/// Expression for a tone whose frequency is `300 + 100 * floor(t)` Hz at amplitude 0.5.
/// The phase is continuous across the steps.
pub const STEP_TONE: &str = "0.5*sin(2*PI*(300+100*floor(t))*t)";

/// Frequency of [`STEP_TONE`] during source second `sec`.
pub fn step_freq(sec: usize) -> f64 {
    300.0 + 100.0 * sec as f64
}

/// Stereo 48 kHz 16-bit WAV of the stepped tone.
pub fn tone_wav(secs: u32) -> PathBuf {
    generated(&format!("tone{secs}.wav"), |p| {
        ffmpeg(&["-f", "lavfi", "-i", &format!("aevalsrc=exprs='{STEP_TONE}|{STEP_TONE}':s=48000:d={secs}"), "-c:a", "pcm_s16le", p.to_str().unwrap()]);
    })
}

/// A steady stereo sine WAV.
pub fn sine_wav(name: &str, hz: u32, secs: f64, rate: u32, channels: u32) -> PathBuf {
    generated(name, |p| {
        ffmpeg(&["-f", "lavfi", "-i", &format!("sine=f={hz}:r={rate}:d={secs}"), "-ac", &channels.to_string(), "-c:a", "pcm_s16le", p.to_str().unwrap()]);
    })
}

pub const VIDEO_W: u32 = 320;
pub const VIDEO_H: u32 = 180;
pub const VIDEO_FPS: f64 = 30.0;

fn frame_number_source(secs: u32) -> String {
    // Eight vertical bars; bar b is white when bit b of the frame number is set.
    format!("color=black:s={VIDEO_W}x{VIDEO_H}:r=30:d={secs},geq=lum='255*mod(floor(N/pow(2,floor(X*8/W))),2)':cb=128:cr=128")
}

/// H.264 video in which every frame shows its own number, 30 fps, a keyframe
/// every second and B-frames; with the stepped tone as AAC audio if `audio`.
pub fn numbered_video(secs: u32, audio: bool) -> PathBuf {
    generated(&format!("numbered{secs}{}.mp4", if audio { "a" } else { "" }), |p| {
        let v = frame_number_source(secs);
        let a = format!("aevalsrc=exprs='{STEP_TONE}|{STEP_TONE}':s=48000:d={secs}");
        let mut args = vec!["-f", "lavfi", "-i", v.as_str()];
        if audio {
            args.extend(["-f", "lavfi", "-i", a.as_str(), "-c:a", "aac", "-b:a", "128k"]);
        }
        args.extend(["-c:v", "libx264", "-g", "30", "-bf", "2", "-pix_fmt", "yuv420p", p.to_str().unwrap()]);
        ffmpeg(&args);
    })
}

/// The number encoded in a frame of [`numbered_video`].
pub fn frame_number(f: &VideoFrame) -> u32 {
    read_bars(f, 8, 0.5)
}

/// 20 s of 1920x1080 30 fps H.264 with the encoder's default long GOP (keyframes
/// 8.33 s apart), moving test-pattern content, and the frame number in ten bars
/// across the top fifth of the picture.
pub fn hd_video() -> PathBuf {
    generated("hd20.mp4", |p| {
        let mut graph = String::from("testsrc2=s=1920x1080:r=30:d=20,drawbox=x=0:y=0:w=iw:h=216:c=black:t=fill");
        for bit in 0..10 {
            graph += &format!(",drawbox=x={}:y=0:w=192:h=216:c=white:t=fill:enable='mod(floor(n/{}),2)'", bit * 192, 1 << bit);
        }
        ffmpeg(&["-f", "lavfi", "-i", &graph, "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", p.to_str().unwrap()]);
    })
}

/// The number encoded in a frame of [`hd_video`].
pub fn hd_frame_number(f: &VideoFrame) -> u32 {
    read_bars(f, 10, 0.1)
}

/// Reads `bits` equal-width vertical bars (white = 1, least significant on the left) at height `y_frac`.
fn read_bars(f: &VideoFrame, bits: u32, y_frac: f64) -> u32 {
    assert_eq!(f.bgra.len(), (f.width * f.height * 4) as usize, "frame buffer has the wrong size");
    let mut n = 0;
    for bit in 0..bits {
        // Centre of the bar, averaged over a small patch (green channel).
        let cx = (f.width * (2 * bit + 1)) / (2 * bits);
        let cy = (f.height as f64 * y_frac) as u32;
        let mut sum = 0u32;
        let mut count = 0u32;
        for y in cy.saturating_sub(2)..(cy + 3).min(f.height) {
            for x in cx.saturating_sub(2)..(cx + 3).min(f.width) {
                sum += f.bgra[((y * f.width + x) * 4 + 1) as usize] as u32;
                count += 1;
            }
        }
        if sum / count > 127 {
            n |= 1 << bit;
        }
    }
    n
}

/// Duration reported by ffprobe for the container.
pub fn probe_duration(path: &Path) -> f64 {
    let out = Process::new(tool("ffprobe"))
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1"])
        .arg(path)
        .output()
        .expect("run ffprobe");
    String::from_utf8_lossy(&out.stdout).trim().parse().expect("ffprobe duration")
}

/// Signal power at `hz` (Goertzel) in one channel.
pub fn power_at(samples: &[[f32; 2]], ch: usize, hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / SR as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for s in samples {
        let s0 = s[ch] as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2) / (samples.len() as f64 * samples.len() as f64 / 4.0)
}

/// The strongest frequency among multiples of 50 Hz from 100 to 2400 Hz.
pub fn dominant_freq(samples: &[[f32; 2]], ch: usize) -> f64 {
    let mut best = (0.0, -1.0);
    for i in 2..=48 {
        let hz = i as f64 * 50.0;
        let p = power_at(samples, ch, hz);
        if p > best.1 {
            best = (hz, p);
        }
    }
    assert!(best.1 > 1e-4, "no tone found (strongest power {})", best.1);
    best.0
}

pub fn peak(samples: &[[f32; 2]], ch: usize) -> f32 {
    samples.iter().map(|s| s[ch].abs()).fold(0.0, f32::max)
}

pub fn rms(samples: &[[f32; 2]], ch: usize) -> f64 {
    (samples.iter().map(|s| (s[ch] as f64).powi(2)).sum::<f64>() / samples.len().max(1) as f64).sqrt()
}

/// Frames `[from, to)` seconds of a rendered buffer that starts at time 0.
pub fn span(buf: &[[f32; 2]], from: f64, to: f64) -> &[[f32; 2]] {
    &buf[(from * SR as f64).round() as usize..(to * SR as f64).round() as usize]
}

/// An asset for `path` whose transcript has one word per second: "w0" covers
/// source `[0, 1)`, "w1" `[1, 2)` and so on.
pub fn asset_per_second(path: &Path, kind: MediaKind, secs: u32) -> MediaAsset {
    let words = (0..secs).map(|i| TranscriptWord { text: format!("w{i}"), start: i as f64, end: i as f64 + 1.0, speaker: None, confidence: 1.0 }).collect();
    asset_with_words(path, kind, secs as f64, words)
}

pub fn asset_with_words(path: &Path, kind: MediaKind, duration: f64, words: Vec<TranscriptWord>) -> MediaAsset {
    let video = kind == MediaKind::Video;
    MediaAsset {
        id: AssetId::new(),
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path: path.to_path_buf(),
        kind,
        duration,
        width: if video { VIDEO_W } else { 0 },
        height: if video { VIDEO_H } else { 0 },
        fps: if video { VIDEO_FPS } else { 0.0 },
        has_audio: true,
        has_video: video,
        sample_rate: 48_000,
        channels: 2,
        folder: None,
        transcript: Some(Transcript { language: "en".into(), words }),
    }
}

/// A project whose script is the whole of `asset`.
pub fn editor_with_script(asset: MediaAsset) -> (Editor, AssetId) {
    let id = asset.id;
    let mut e = Editor::new_project("Test");
    e.apply(Command::AddAsset { asset }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    (e, id)
}

pub fn word_index(e: &Editor, w: &str) -> usize {
    e.comp().unwrap().script.iter().position(|t| t.is_word() && t.text == w).unwrap_or_else(|| panic!("no word {w}"))
}

/// Number of live child processes of this test process whose command line mentions `needle`.
pub fn children_mentioning(needle: &str) -> usize {
    let out = Process::new("pgrep").args(["-P", &std::process::id().to_string(), "-f", needle]).output().expect("run pgrep");
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.trim().is_empty()).count()
}

pub fn process_alive(pid: u32) -> bool {
    Process::new("kill").args(["-0", &pid.to_string()]).output().map(|o| o.status.success()).unwrap_or(false)
}
