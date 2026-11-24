//! Shared helpers: real speech generated with macOS `say`, assembled into clips with known timing.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub const SR: usize = 16_000;

pub fn ffmpeg() -> PathBuf {
    mango_core::tools::find_tool("ffmpeg").expect("ffmpeg is required for the tests")
}

pub fn ffprobe() -> PathBuf {
    mango_core::tools::find_tool("ffprobe").expect("ffprobe is required for the tests")
}

pub fn model(name: &str) -> PathBuf {
    let dir = std::env::var_os("MANGO_TEST_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"));
    let p = dir.join(name);
    assert!(p.is_file(), "test model {} is missing (set MANGO_TEST_MODELS_DIR)", p.display());
    p
}

pub fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap_or_else(|e| panic!("could not run {:?}: {e}", cmd.get_program()));
    assert!(
        out.status.success(),
        "{:?} failed: {}",
        cmd.get_program(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Decodes any audio file to 16 kHz mono f32.
pub fn decode_pcm(path: &Path) -> Vec<f32> {
    let out = Command::new(ffmpeg())
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-ac", "1", "-ar", "16000", "-f", "f32le", "pipe:1"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "ffmpeg: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

/// Speaks `text` with the system synthesiser and returns the speech with leading and
/// trailing silence removed, so sample 0 is the true onset.
pub fn say_pcm(dir: &Path, text: &str, voice: Option<&str>) -> Vec<f32> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let aiff = dir.join(format!("say-{}-{n}.aiff", std::process::id()));
    let mut cmd = Command::new("/usr/bin/say");
    if let Some(v) = voice {
        cmd.args(["-v", v]);
    }
    cmd.arg("-o").arg(&aiff).arg(text);
    run(&mut cmd);
    let pcm = decode_pcm(&aiff);
    let _ = std::fs::remove_file(&aiff);
    trim_silence(&pcm)
}

pub fn trim_silence(pcm: &[f32]) -> Vec<f32> {
    let thr = 0.004;
    let first = pcm.iter().position(|s| s.abs() > thr).expect("say produced only silence");
    let last = pcm.iter().rposition(|s| s.abs() > thr).unwrap();
    pcm[first..=last].to_vec()
}

pub struct Clip {
    pub samples: Vec<f32>,
    /// (start, end) of each phrase in seconds.
    pub phrases: Vec<(f64, f64)>,
}

impl Clip {
    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / SR as f64
    }
}

/// Lays out phrases as: lead-in silence, phrase, gap, phrase, ..., tail silence.
pub fn build_clip(dir: &Path, phrases: &[&str], lead_in: f64, gaps: &[f64], tail: f64) -> Clip {
    assert_eq!(gaps.len() + 1, phrases.len());
    let mut samples = vec![0.0f32; (lead_in * SR as f64) as usize];
    let mut spans = Vec::new();
    for (i, text) in phrases.iter().enumerate() {
        let pcm = say_pcm(dir, text, Some("Samantha"));
        let start = samples.len() as f64 / SR as f64;
        samples.extend_from_slice(&pcm);
        spans.push((start, samples.len() as f64 / SR as f64));
        let silence = if i < gaps.len() { gaps[i] } else { tail };
        samples.extend(std::iter::repeat_n(0.0f32, (silence * SR as f64) as usize));
    }
    Clip { samples, phrases: spans }
}

pub fn write_wav(path: &Path, samples: &[f32], rate: u32) {
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write wav");
}

pub fn probe_duration(path: &Path) -> f64 {
    let out = run(Command::new(ffprobe())
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path));
    out.trim().parse().unwrap_or_else(|_| panic!("ffprobe gave no duration for {}: {out:?}", path.display()))
}

/// Lower-case words with punctuation stripped.
pub fn norm_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Word error rate: edit distance over reference length.
pub fn wer(reference: &[String], hypothesis: &[String]) -> f64 {
    let (n, m) = (reference.len(), hypothesis.len());
    let mut prev: Vec<usize> = (0..=m).collect();
    for i in 1..=n {
        let mut cur = vec![i; m + 1];
        for j in 1..=m {
            let sub = prev[j - 1] + usize::from(reference[i - 1] != hypothesis[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    prev[m] as f64 / n.max(1) as f64
}

pub const PHRASES: [&str; 6] = [
    "The quick brown fox jumps over the lazy dog near the river bank.",
    "Yesterday we recorded a long interview about renewable energy.",
    "Please remember to charge the camera batteries before the trip.",
    "Seven people waited quietly outside the small wooden door.",
    "Editing video by changing the text feels almost like magic.",
    "Thank you for listening, and see you again next week.",
];
pub const GAPS: [f64; 5] = [1.0, 0.7, 1.6, 0.9, 1.2];
pub const LEAD_IN: f64 = 0.8;
pub const TAIL: f64 = 1.0;

pub fn standard_clip(dir: &Path) -> Clip {
    build_clip(dir, &PHRASES, LEAD_IN, &GAPS, TAIL)
}

/// For each reference word, the index of the hypothesis word it aligns to (equal words only).
pub fn align_indices(reference: &[String], hypothesis: &[String]) -> Vec<Option<usize>> {
    let (n, m) = (reference.len(), hypothesis.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..=n {
        d[i][0] = i;
    }
    for j in 0..=m {
        d[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let sub = d[i - 1][j - 1] + usize::from(reference[i - 1] != hypothesis[j - 1]);
            d[i][j] = sub.min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
        }
    }
    let mut out = vec![None; n];
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        if d[i][j] == d[i - 1][j - 1] + usize::from(reference[i - 1] != hypothesis[j - 1]) {
            if reference[i - 1] == hypothesis[j - 1] {
                out[i - 1] = Some(j - 1);
            }
            i -= 1;
            j -= 1;
        } else if d[i][j] == d[i - 1][j] + 1 {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    out
}

pub struct Stats {
    pub mean: f64,
    pub max: f64,
    pub n: usize,
}

pub fn stats(values: &[f64]) -> Stats {
    let n = values.len();
    Stats {
        mean: values.iter().map(|v| v.abs()).sum::<f64>() / n.max(1) as f64,
        max: values.iter().map(|v| v.abs()).fold(0.0, f64::max),
        n,
    }
}

/// A clip of separately synthesised words with 40 ms between them: every word boundary is
/// known exactly, and the pauses are too short for pause detection to do the work.
pub fn isolated_words_clip(dir: &Path, words: &[&str]) -> (Vec<f32>, Vec<(f64, f64)>) {
    let mut samples = vec![0.0f32; SR / 2];
    let mut truth = Vec::new();
    for w in words {
        let pcm = say_pcm(dir, w, Some("Samantha"));
        let s = samples.len() as f64 / SR as f64;
        samples.extend_from_slice(&pcm);
        truth.push((s, samples.len() as f64 / SR as f64));
        samples.extend(std::iter::repeat_n(0.0f32, SR * 4 / 100));
    }
    samples.extend(std::iter::repeat_n(0.0f32, SR / 2));
    (samples, truth)
}

pub const ISOLATED: &str = "we must finish this difficult project before Friday evening because nobody wants another delay";
