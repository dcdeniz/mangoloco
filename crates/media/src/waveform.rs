//! Waveform peaks, decoded through ffmpeg and cached on disk.

use crate::ffmpeg::{ChildGuard, describe_exit, drain_tail, error_tail, ffmpeg_path, ffprobe_path, run_capture_raw};
use anyhow::{Context, Result, anyhow, bail};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Min/max sample values per time bucket of a mono mixdown.
#[derive(Clone, Debug, PartialEq)]
pub struct Peaks {
    /// Buckets per second.
    pub per_second: u32,
    /// Lowest sample in each bucket, `-1..=1`.
    pub min: Vec<f32>,
    /// Highest sample in each bucket, `-1..=1`.
    pub max: Vec<f32>,
}

impl Peaks {
    pub fn len(&self) -> usize {
        self.max.len().min(self.min.len())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Length of the audio these peaks describe, in seconds.
    pub fn duration(&self) -> f64 {
        if self.per_second == 0 { 0.0 } else { self.len() as f64 / self.per_second as f64 }
    }
    /// Largest absolute sample value in `[from, to)` seconds; 0 outside the data.
    pub fn amplitude(&self, from: f64, to: f64) -> f32 {
        let n = self.len();
        if n == 0 || self.per_second == 0 || !(to > from) || to <= 0.0 {
            return 0.0;
        }
        let ps = self.per_second as f64;
        let a = (from.max(0.0) * ps).floor() as usize;
        let b = ((to * ps).ceil().max(0.0) as usize).min(n);
        let mut m = 0.0f32;
        for i in a.min(n)..b {
            m = m.max(self.max[i].abs()).max(self.min[i].abs());
        }
        m.min(1.0)
    }
}

const MAGIC: &[u8; 8] = b"MLPEAKS1";
const TARGET_DECODE_RATE: u32 = 16_000;

/// Peaks for the audio in `path`, `per_second` buckets per second, cached
/// under [`mango_core::tools::cache_dir`].
pub fn peaks(path: &Path, per_second: u32) -> Result<Peaks> {
    peaks_with_cache_dir(path, per_second, &mango_core::tools::cache_dir().join("waveforms"))
}

/// As [`peaks`], with an explicit cache directory.
pub fn peaks_with_cache_dir(path: &Path, per_second: u32, cache_dir: &Path) -> Result<Peaks> {
    if per_second == 0 || per_second > 48_000 {
        bail!("waveform resolution must be between 1 and 48000 peaks per second");
    }
    let meta = std::fs::metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    let cache_file = cache_dir.join(cache_name(path, &meta, per_second));
    if let Some(p) = read_cache(&cache_file, per_second) {
        return Ok(p);
    }
    let p = decode_peaks(path, per_second)?;
    // The cache is an optimisation: failing to write it must not fail the call.
    let _ = write_cache(&cache_file, &p);
    Ok(p)
}

fn fnv1a(bytes: &[u8], mut h: u64) -> u64 {
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn cache_name(path: &Path, meta: &std::fs::Metadata, per_second: u32) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = fnv1a(canonical.to_string_lossy().as_bytes(), 0xcbf2_9ce4_8422_2325);
    h = fnv1a(&meta.len().to_le_bytes(), h);
    h = fnv1a(&mtime.to_le_bytes(), h);
    h = fnv1a(&per_second.to_le_bytes(), h);
    format!("{h:016x}-{per_second}.peaks")
}

fn read_cache(file: &Path, per_second: u32) -> Option<Peaks> {
    let data = std::fs::read(file).ok()?;
    if data.len() < 20 || &data[..8] != MAGIC {
        return None;
    }
    let ps = u32::from_le_bytes(data[8..12].try_into().ok()?);
    let n = u64::from_le_bytes(data[12..20].try_into().ok()?) as usize;
    if ps != per_second || data.len() != 20 + n.checked_mul(8)? {
        return None;
    }
    let floats = |bytes: &[u8]| -> Vec<f32> { bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect() };
    let min = floats(&data[20..20 + n * 4]);
    let max = floats(&data[20 + n * 4..]);
    if min.iter().chain(max.iter()).any(|v| !v.is_finite()) {
        return None;
    }
    Some(Peaks { per_second, min, max })
}

fn write_cache(file: &Path, p: &Peaks) -> Result<()> {
    let dir = file.parent().ok_or_else(|| anyhow!("cache file has no directory"))?;
    std::fs::create_dir_all(dir)?;
    let n = p.len();
    let mut data = Vec::with_capacity(20 + n * 8);
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&p.per_second.to_le_bytes());
    data.extend_from_slice(&(n as u64).to_le_bytes());
    for v in &p.min[..n] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    for v in &p.max[..n] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    // Write-then-rename so a concurrent reader never sees a partial file.
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, &data)?;
    tmp.persist(file).map_err(|e| e.error)?;
    Ok(())
}

/// Channel count of the first audio stream.
fn audio_channels(path: &Path) -> Result<usize> {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let args: Vec<String> = ["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=channels", "-of", "csv=p=0"]
        .iter()
        .map(|s| s.to_string())
        .chain([path.to_string_lossy().into_owned()])
        .collect();
    let (status, out, err) = run_capture_raw(&ffprobe_path()?, &args)?;
    if !status.success() {
        bail!("could not read audio from {name}:\n{}", error_tail(&err));
    }
    let text = String::from_utf8_lossy(&out);
    match text.lines().find_map(|l| l.trim().trim_end_matches(',').parse::<usize>().ok()) {
        Some(n) if (1..=64).contains(&n) => Ok(n),
        _ => bail!("{name} has no audio to draw a waveform from"),
    }
}

fn decode_peaks(path: &Path, per_second: u32) -> Result<Peaks> {
    // All channels are decoded and averaged here: ffmpeg's own downmix is
    // power-preserving and would clip (or scale) correlated channels.
    let channels = audio_channels(path)?;
    // A decode rate that is a whole multiple of the bucket rate, so every
    // bucket holds the same number of samples and never drifts.
    let per_bucket = TARGET_DECODE_RATE.div_ceil(per_second).max(1);
    let rate = per_bucket * per_second;
    let program = ffmpeg_path()?;
    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-i".into(),
        path.to_string_lossy().into_owned(),
        "-map".into(),
        "0:a:0".into(),
        "-vn".into(),
        "-ar".into(),
        rate.to_string(),
        "-f".into(),
        "f32le".into(),
        "pipe:1".into(),
    ];
    let child = Command::new(&program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not start {}", program.display()))?;
    let mut child = ChildGuard(child);
    let stderr = child.0.stderr.take().ok_or_else(|| anyhow!("no stderr pipe"))?;
    let mut stdout = child.0.stdout.take().ok_or_else(|| anyhow!("no stdout pipe"))?;
    let err_thread = drain_tail(stderr, 8192);

    let mut min: Vec<f32> = Vec::new();
    let mut max: Vec<f32> = Vec::new();
    let (mut lo, mut hi, mut count) = (f32::INFINITY, f32::NEG_INFINITY, 0u32);
    let (mut frame_sum, mut in_frame) = (0f32, 0usize);
    let mut buf = vec![0u8; 64 * 1024];
    let mut carry = 0usize; // bytes of an incomplete sample at the start of buf
    loop {
        let n = stdout.read(&mut buf[carry..]).context("reading decoded audio")?;
        if n == 0 {
            break;
        }
        let avail = carry + n;
        let whole = avail - avail % 4;
        for c in buf[..whole].chunks_exact(4) {
            let mut s = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            if !s.is_finite() {
                s = 0.0;
            }
            frame_sum += s;
            in_frame += 1;
            if in_frame < channels {
                continue;
            }
            let s = (frame_sum / channels as f32).clamp(-1.0, 1.0);
            (frame_sum, in_frame) = (0.0, 0);
            lo = lo.min(s);
            hi = hi.max(s);
            count += 1;
            if count == per_bucket {
                min.push(lo);
                max.push(hi);
                (lo, hi, count) = (f32::INFINITY, f32::NEG_INFINITY, 0);
            }
        }
        buf.copy_within(whole..avail, 0);
        carry = avail - whole;
    }
    // A trailing partial bucket counts only if it is at least half full, so
    // the length rounds to the duration instead of always rounding up.
    if count * 2 >= per_bucket {
        min.push(lo);
        max.push(hi);
    }
    let status = child.0.wait().context("waiting for ffmpeg")?;
    let err = err_thread.join().unwrap_or_default();
    if !status.success() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        bail!("could not read audio from {name} ({}):\n{}", describe_exit(status), error_tail(&err));
    }
    Ok(Peaks { per_second, min, max })
}

/// Where the default cache for [`peaks`] lives.
pub fn default_cache_dir() -> PathBuf {
    mango_core::tools::cache_dir().join("waveforms")
}
