//! Decoded-audio cache.
//!
//! Every source file is decoded once to 48 kHz stereo s16le, stored in the
//! per-user cache directory and memory-mapped, so the mixer can read any
//! sample of any source in O(1) without a decoder in the audio path.
//!
//! Sample `n` of a [`Pcm`] is the sound at source time `n / 48000`, on the same
//! clock `ffmpeg -ss` uses for that file (audio that starts late is padded
//! with silence), so audio and [`crate::video`] frames line up.

use anyhow::{Context, Result, anyhow, bail};
use memmap2::Mmap;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Sample rate of all decoded audio and of the mixer.
pub const SAMPLE_RATE: u32 = 48_000;
/// Channels of all decoded audio.
pub const CHANNELS: usize = 2;

const BYTES_PER_FRAME: usize = 2 * CHANNELS;
const HEADER_LEN: usize = 64;
const MAGIC: &[u8; 8] = b"MANGOPCM";
const FORMAT_VERSION: u32 = 1;
const FLAG_NO_AUDIO: u32 = 1;

/// Returned (inside the `anyhow::Error`) when `ensure` was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("audio decoding was cancelled")
    }
}
impl std::error::Error for Cancelled {}

/// Decoded audio for one source file: 48 kHz stereo, memory-mapped.
pub struct Pcm {
    map: Mmap,
    frames: u64,
    file: PathBuf,
}

impl std::fmt::Debug for Pcm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pcm").field("frames", &self.frames).field("file", &self.file).finish()
    }
}

impl Pcm {
    /// Number of stereo frames.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Length in seconds.
    pub fn duration(&self) -> f64 {
        self.frames as f64 / SAMPLE_RATE as f64
    }

    /// The cache file backing this audio.
    pub fn cache_file(&self) -> &Path {
        &self.file
    }

    #[inline]
    fn data(&self) -> &[u8] {
        &self.map[HEADER_LEN..HEADER_LEN + self.frames as usize * BYTES_PER_FRAME]
    }

    /// One frame as raw samples; silence past the end.
    #[inline]
    pub(crate) fn frame_i16(&self, frame: u64) -> [i16; 2] {
        if frame >= self.frames {
            return [0, 0];
        }
        let o = HEADER_LEN + frame as usize * BYTES_PER_FRAME;
        let b = &self.map[o..o + BYTES_PER_FRAME];
        [i16::from_le_bytes([b[0], b[1]]), i16::from_le_bytes([b[2], b[3]])]
    }

    /// One frame as floats in `-1.0..1.0`; silence past the end.
    #[inline]
    pub(crate) fn frame_f32(&self, frame: u64) -> [f32; 2] {
        let s = self.frame_i16(frame);
        [s[0] as f32 * I16_SCALE, s[1] as f32 * I16_SCALE]
    }

    /// Copies frames starting at `start_frame` into `out` as floats in
    /// `-1.0..1.0`. Frames past the end are zero-filled. Returns how many
    /// frames of `out` came from the file.
    pub fn read(&self, start_frame: u64, out: &mut [[f32; 2]]) -> usize {
        let avail = self.frames.saturating_sub(start_frame).min(out.len() as u64) as usize;
        if avail > 0 {
            let o = start_frame as usize * BYTES_PER_FRAME;
            let src = &self.data()[o..o + avail * BYTES_PER_FRAME];
            for (dst, b) in out[..avail].iter_mut().zip(src.as_chunks::<BYTES_PER_FRAME>().0) {
                *dst = [
                    i16::from_le_bytes([b[0], b[1]]) as f32 * I16_SCALE,
                    i16::from_le_bytes([b[2], b[3]]) as f32 * I16_SCALE,
                ];
            }
        }
        out[avail..].fill([0.0, 0.0]);
        avail
    }

    /// Asks the OS to page in `[start_frame, start_frame + frames)` so the
    /// audio thread does not fault on it. Never blocks on the read itself.
    pub fn prefetch(&self, start_frame: u64, frames: u64) {
        let start = start_frame.min(self.frames);
        let len = frames.min(self.frames - start);
        if len == 0 {
            return;
        }
        #[cfg(unix)]
        {
            // Advice only: a failure costs a possible page fault later, nothing else.
            let _ = self.map.advise_range(
                memmap2::Advice::WillNeed,
                HEADER_LEN + start as usize * BYTES_PER_FRAME,
                len as usize * BYTES_PER_FRAME,
            );
        }
    }
}

pub(crate) const I16_SCALE: f32 = 1.0 / 32768.0;

/// Identity of a source file's contents, as far as the cache can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    mtime_ns: u128,
}

impl Stamp {
    fn of(meta: &fs::Metadata) -> Self {
        let mtime_ns = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
        Self { size: meta.len(), mtime_ns }
    }
}

#[derive(Clone)]
struct Loaded {
    stamp: Stamp,
    /// `None`: the file has no audio stream.
    pcm: Option<Arc<Pcm>>,
}

struct Inner {
    dir: PathBuf,
    loaded: Mutex<HashMap<PathBuf, Loaded>>,
    /// Cache files currently being produced by some `ensure` call.
    busy: Mutex<HashSet<PathBuf>>,
    busy_changed: Condvar,
}

/// Thread-safe, cloneable handle to the decoded-audio cache.
#[derive(Clone)]
pub struct PcmCache {
    inner: Arc<Inner>,
}

impl Default for PcmCache {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl PcmCache {
    /// A cache in `mango_core::tools::cache_dir()/pcm`.
    pub fn new() -> Self {
        Self::with_dir(mango_core::tools::cache_dir().join("pcm"))
    }

    /// A cache in a specific directory (created on first use).
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(Inner {
                dir: dir.into(),
                loaded: Mutex::new(HashMap::new()),
                busy: Mutex::new(HashSet::new()),
                busy_changed: Condvar::new(),
            }),
        }
    }

    /// Directory holding the cache files.
    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    /// Audio for `path` if it has already been loaded by `ensure` on this
    /// handle (or a clone). Never blocks on decoding or touches the disk.
    /// `None` also for files known to have no audio.
    pub fn get(&self, path: &Path) -> Option<Arc<Pcm>> {
        lock(&self.inner.loaded).get(path).and_then(|l| l.pcm.clone())
    }

    /// Whether `ensure` has completed for `path` on this handle, including
    /// for files without audio.
    pub fn is_loaded(&self, path: &Path) -> bool {
        lock(&self.inner.loaded).contains_key(path)
    }

    /// Decoded audio for `path`, decoding it with ffmpeg if it is not in the
    /// cache yet. `on_progress` receives values in `0.0..=1.0` (and always a
    /// final `1.0` on success). Setting `cancel` aborts a running decode with
    /// a [`Cancelled`] error and leaves nothing behind. Files without an
    /// audio stream give `Ok(None)`.
    pub fn ensure(&self, path: &Path, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<Option<Arc<Pcm>>> {
        let meta = fs::metadata(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => anyhow!("media file not found: {}", path.display()),
            _ => anyhow!("cannot read media file {}: {e}", path.display()),
        })?;
        if !meta.is_file() {
            bail!("not a media file: {}", path.display());
        }
        let stamp = Stamp::of(&meta);
        if let Some(l) = lock(&self.inner.loaded).get(path)
            && l.stamp == stamp {
                on_progress(1.0);
                return Ok(l.pcm.clone());
            }
        fs::create_dir_all(&self.inner.dir).with_context(|| format!("cannot create audio cache folder {}", self.inner.dir.display()))?;
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let path_hash = fnv1a(canonical.as_os_str().as_encoded_bytes());
        let cache_file = self.inner.dir.join(format!("{path_hash:016x}-{:x}-{:x}.pcm", stamp.size, stamp.mtime_ns));

        // One decode per cache file at a time; later callers wait and then hit the cache.
        let _busy = BusyGuard::acquire(&self.inner, &cache_file, cancel)?;
        if let Some(l) = lock(&self.inner.loaded).get(path)
            && l.stamp == stamp {
                on_progress(1.0);
                return Ok(l.pcm.clone());
            }
        let pcm = match open_cache_file(&cache_file) {
            Ok(Some(hit)) => {
                // Mark as recently used for `prune`.
                if let Ok(f) = File::options().write(true).open(&cache_file) {
                    let _ = f.set_modified(SystemTime::now());
                }
                hit
            }
            Ok(None) | Err(_) => {
                // Missing, or unreadable/corrupt: decode again.
                let _ = fs::remove_file(&cache_file);
                decode_to_cache(path, &cache_file, stamp, on_progress, cancel)?;
                self.remove_stale_versions(path_hash, &cache_file);
                open_cache_file(&cache_file)?.ok_or_else(|| anyhow!("audio cache file {} vanished after decoding", cache_file.display()))?
            }
        };
        let pcm = pcm.map(Arc::new);
        lock(&self.inner.loaded).insert(path.to_path_buf(), Loaded { stamp, pcm: pcm.clone() });
        on_progress(1.0);
        Ok(pcm)
    }

    /// Forgets the in-memory entry for `path` (the cache file stays).
    pub fn forget(&self, path: &Path) {
        lock(&self.inner.loaded).remove(path);
    }

    /// Bytes used by cache files on disk.
    pub fn disk_usage(&self) -> u64 {
        self.cache_files().iter().map(|(_, len, _)| len).sum()
    }

    /// Deletes least-recently-used cache files until at most `max_bytes`
    /// remain. Files loaded through this handle are kept. Also removes
    /// leftovers of decodes that were interrupted by a crash. Returns the
    /// number of bytes freed.
    pub fn prune(&self, max_bytes: u64) -> u64 {
        let mut freed = 0;
        let now = SystemTime::now();
        if let Ok(rd) = fs::read_dir(&self.inner.dir) {
            for e in rd.flatten() {
                let p = e.path();
                let is_tmp = p.extension().is_some_and(|x| x == "tmp");
                let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| now.duration_since(t).ok()).is_some_and(|d| d > Duration::from_secs(24 * 3600));
                if is_tmp && old {
                    let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                    if fs::remove_file(&p).is_ok() {
                        freed += len;
                    }
                }
            }
        }
        let in_use: HashSet<PathBuf> = lock(&self.inner.loaded).values().filter_map(|l| l.pcm.as_ref().map(|p| p.file.clone())).collect();
        let mut files = self.cache_files();
        files.sort_by_key(|(_, _, used)| *used);
        let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
        for (p, len, _) in files {
            if total <= max_bytes {
                break;
            }
            if in_use.contains(&p) || lock(&self.inner.busy).contains(&p) {
                continue;
            }
            if fs::remove_file(&p).is_ok() {
                total -= len;
                freed += len;
            }
        }
        freed
    }

    fn cache_files(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut v = Vec::new();
        if let Ok(rd) = fs::read_dir(&self.inner.dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "pcm")
                    && let Ok(m) = e.metadata() {
                        v.push((p, m.len(), m.modified().unwrap_or(UNIX_EPOCH)));
                    }
            }
        }
        v
    }

    /// Removes cache files for older versions of the same source path.
    fn remove_stale_versions(&self, path_hash: u64, keep: &Path) {
        let prefix = format!("{path_hash:016x}-");
        let in_use: HashSet<PathBuf> = lock(&self.inner.loaded).values().filter_map(|l| l.pcm.as_ref().map(|p| p.file.clone())).collect();
        for (p, _, _) in self.cache_files() {
            let same_source = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix));
            // Unlinking a file that is still mapped elsewhere is safe on unix, but keep it simple: skip those.
            if same_source && p != keep && (cfg!(unix) || !in_use.contains(&p)) {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

/// Holds the "being decoded" mark for one cache file.
struct BusyGuard<'a> {
    inner: &'a Inner,
    file: PathBuf,
}

impl<'a> BusyGuard<'a> {
    fn acquire(inner: &'a Inner, file: &Path, cancel: &AtomicBool) -> Result<Self> {
        let mut busy = lock(&inner.busy);
        while busy.contains(file) {
            if cancel.load(Ordering::Relaxed) {
                return Err(Cancelled.into());
            }
            busy = inner.busy_changed.wait_timeout(busy, Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner()).0;
        }
        busy.insert(file.to_path_buf());
        Ok(Self { inner, file: file.to_path_buf() })
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        lock(&self.inner.busy).remove(&self.file);
        self.inner.busy_changed.notify_all();
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn header(flags: u32, frames: u64, stamp: Stamp) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[0..8].copy_from_slice(MAGIC);
    h[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    h[12..16].copy_from_slice(&flags.to_le_bytes());
    h[16..24].copy_from_slice(&frames.to_le_bytes());
    h[24..32].copy_from_slice(&stamp.size.to_le_bytes());
    h[32..48].copy_from_slice(&stamp.mtime_ns.to_le_bytes());
    h[48..52].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
    h[52..56].copy_from_slice(&(CHANNELS as u32).to_le_bytes());
    h
}

/// `Ok(None)`: no such file. `Ok(Some(None))`: source has no audio.
/// `Err`: the file exists but is not a complete, valid cache file.
fn open_cache_file(file: &Path) -> Result<Option<Option<Pcm>>> {
    let f = match File::open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot open audio cache file {}", file.display())),
    };
    let len = f.metadata()?.len();
    if len < HEADER_LEN as u64 {
        bail!("audio cache file {} is truncated", file.display());
    }
    // SAFETY: cache files are written to a temporary name and renamed into
    // place only when complete, and are never modified afterwards (only
    // unlinked, which keeps existing mappings valid).
    let map = unsafe { Mmap::map(&f) }.with_context(|| format!("cannot map audio cache file {}", file.display()))?;
    if &map[0..8] != MAGIC || u32::from_le_bytes(map[8..12].try_into().unwrap()) != FORMAT_VERSION {
        bail!("audio cache file {} has an unknown format", file.display());
    }
    let flags = u32::from_le_bytes(map[12..16].try_into().unwrap());
    let frames = u64::from_le_bytes(map[16..24].try_into().unwrap());
    let rate = u32::from_le_bytes(map[48..52].try_into().unwrap());
    let channels = u32::from_le_bytes(map[52..56].try_into().unwrap());
    let expected = frames.checked_mul(BYTES_PER_FRAME as u64).and_then(|b| b.checked_add(HEADER_LEN as u64));
    if rate != SAMPLE_RATE || channels as usize != CHANNELS || expected != Some(len) || map.len() as u64 != len {
        bail!("audio cache file {} is damaged", file.display());
    }
    if flags & FLAG_NO_AUDIO != 0 {
        return Ok(Some(None));
    }
    Ok(Some(Some(Pcm { map, frames, file: file.to_path_buf() })))
}

/// Removes the temporary file unless the decode completed.
struct TmpFile {
    path: PathBuf,
    keep: bool,
}

impl Drop for TmpFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Kills and reaps the child on every exit path.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn tool(name: &str) -> Result<PathBuf> {
    mango_core::tools::find_tool(name).ok_or_else(|| anyhow!("{name} was not found; it is required to read media files"))
}

/// What ffprobe says about the first audio stream.
struct AudioProbe {
    has_audio: bool,
    duration: Option<f64>,
}

fn probe_audio(path: &Path) -> Result<AudioProbe> {
    let out = Command::new(tool("ffprobe")?)
        .args(["-v", "error", "-show_entries", "stream=codec_type:format=duration", "-of", "default=nw=1"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .context("cannot run ffprobe")?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        bail!("cannot read {}: {}", path.display(), last_lines(&msg));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let has_audio = text.lines().any(|l| l.trim() == "codec_type=audio");
    let duration = text.lines().find_map(|l| l.trim().strip_prefix("duration=")).and_then(|v| v.parse::<f64>().ok()).filter(|d| d.is_finite() && *d > 0.0);
    Ok(AudioProbe { has_audio, duration })
}

fn last_lines(s: &str) -> String {
    let lines: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let start = lines.len().saturating_sub(3);
    let joined = lines[start..].join("; ");
    if joined.is_empty() { "unknown error".into() } else { joined }
}

fn decode_to_cache(path: &Path, cache_file: &Path, stamp: Stamp, on_progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled.into());
    }
    on_progress(0.0);
    let probe = probe_audio(path)?;
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = cache_file.with_extension(format!("{}-{n}.tmp", std::process::id()));
    let mut tmp = TmpFile { path: tmp_path.clone(), keep: false };
    let file = File::create(&tmp_path).with_context(|| format!("cannot write to the audio cache at {}", tmp_path.display()))?;
    let mut w = BufWriter::with_capacity(1 << 20, file);

    let mut frames: u64 = 0;
    let mut flags = 0;
    w.write_all(&header(flags, 0, stamp))?;
    if !probe.has_audio {
        flags = FLAG_NO_AUDIO;
    } else {
        // `first_pts=0` pads (or trims) so sample 0 is source time 0 even when
        // the audio stream starts late; the channel conversion is the one the
        // exporter uses, so levels match.
        let mut child = ChildGuard(
            Command::new(tool("ffmpeg")?)
                .args(["-v", "error", "-nostdin", "-i"])
                .arg(path)
                .args(["-map", "0:a:0", "-vn", "-sn", "-dn"])
                .args(["-af", &format!("aresample={SAMPLE_RATE}:first_pts=0,aformat=sample_fmts=s16:channel_layouts=stereo")])
                .args(["-c:a", "pcm_s16le", "-f", "s16le", "pipe:1"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("cannot run ffmpeg")?,
        );
        let mut stdout = child.0.stdout.take().ok_or_else(|| anyhow!("ffmpeg has no output pipe"))?;
        let mut stderr = child.0.stderr.take().ok_or_else(|| anyhow!("ffmpeg has no error pipe"))?;
        let err_reader = std::thread::spawn(move || {
            let mut s = Vec::new();
            let _ = (&mut stderr).take(64 * 1024).read_to_end(&mut s);
            // Keep draining so ffmpeg can never block on a full pipe.
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            String::from_utf8_lossy(&s).into_owned()
        });
        let expected_bytes = probe.duration.map(|d| d * SAMPLE_RATE as f64 * BYTES_PER_FRAME as f64);
        let mut buf = vec![0u8; 256 * 1024];
        let mut total: u64 = 0;
        let mut last_reported = 0.0f32;
        let io_result: Result<()> = loop {
            if cancel.load(Ordering::Relaxed) {
                break Err(Cancelled.into());
            }
            match stdout.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    if let Err(e) = w.write_all(&buf[..n]) {
                        break Err(anyhow!("cannot write to the audio cache at {}: {e}", tmp_path.display()));
                    }
                    total += n as u64;
                    if let Some(exp) = expected_bytes {
                        let p = ((total as f64 / exp) as f32).clamp(0.0, 0.99);
                        if p - last_reported >= 0.005 {
                            last_reported = p;
                            on_progress(p);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => break Err(anyhow!("reading decoded audio failed: {e}")),
            }
        };
        if io_result.is_err() {
            let _ = child.0.kill();
        }
        drop(stdout);
        let status = child.0.wait().context("waiting for ffmpeg failed")?;
        let err_text = err_reader.join().unwrap_or_default();
        io_result?;
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        if !status.success() {
            bail!("cannot decode audio of {}: {}", path.display(), last_lines(&err_text));
        }
        frames = total / BYTES_PER_FRAME as u64;
    }
    let mut file = w.into_inner().map_err(|e| anyhow!("cannot write to the audio cache: {}", e.error()))?;
    file.set_len(HEADER_LEN as u64 + frames * BYTES_PER_FRAME as u64)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header(flags, frames, stamp))?;
    file.sync_data().ok();
    drop(file);
    fs::rename(&tmp_path, cache_file).with_context(|| format!("cannot move decoded audio into the cache at {}", cache_file.display()))?;
    tmp.keep = true;
    Ok(())
}
