//! Live video preview: decoded frames for the script track and for video
//! layers, straight from the source files.
//!
//! [`FrameStream`] is one running `ffmpeg` decode. [`ScriptVideo`] and
//! [`LayerVideo`] keep such streams running on worker threads, a little ahead
//! of the playhead, and answer "what should be on screen at time t" without
//! ever blocking the caller.

use anyhow::{Context, Result, anyhow, bail};
use mango_core::model::{Composition, MediaKind, Project};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

/// Tolerance when comparing a frame time with a requested time, seconds.
/// Timestamps are exact to the stream's time base; this only absorbs
/// floating-point noise in the times the caller computes.
const EPS: f64 = 1e-6;
/// Two stream start times closer than this are the same request.
const SAME_START: f64 = 1e-3;
/// The stream for the clip after a cut is started this long before the cut.
const PREOPEN_SECS: f64 = 1.0;
/// While playing, a request up to this far ahead of what is decoded is served
/// by decoding forward rather than restarting the decoder.
const SKIP_AHEAD_PLAYING: f64 = 2.0;
/// The same when paused (scrubbing forward a little).
const SKIP_AHEAD_PAUSED: f64 = 1.0;
/// Decoded frames buffered ahead per stream.
const BUFFER_BYTES: usize = 32 << 20;
/// Largest frame the pipe protocol accepts.
const MAX_FRAME_BYTES: usize = 8192 * 8192 * 4;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One decoded picture, top-down BGRA, 4 bytes per pixel, no padding.
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// Presentation time in the source file, seconds (the clock of `ffmpeg -ss`).
    pub pts: f64,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame").field("width", &self.width).field("height", &self.height).field("pts", &self.pts).finish()
    }
}

// ---------------------------------------------------------------------------
// FrameStream
// ---------------------------------------------------------------------------

enum Info {
    /// Start time of the container: the zero of the `-ss` clock.
    ContainerStart(f64),
    TimeBase(i64, i64),
    Frame { pts: Option<i64>, width: u32, height: u32 },
}

/// A running ffmpeg decode of one source from a start time, scaled down to
/// fit `max_w` x `max_h` (never up, aspect preserved, rotation applied),
/// delivering every source frame in order with its exact timestamp.
///
/// Dropping the stream kills the decoder process.
pub struct FrameStream {
    child: Arc<Mutex<Child>>,
    pid: u32,
    stdout: ChildStdout,
    info: Receiver<Info>,
    stderr_thread: Option<JoinHandle<()>>,
    errors: Arc<Mutex<Vec<String>>>,
    path: PathBuf,
    /// Requested start; earlier frames are skipped.
    start: f64,
    /// Timestamp of the container's start, which `-ss` counts from.
    container_start: f64,
    time_base: Option<(i64, i64)>,
    last_pts: Option<f64>,
    delivered: u64,
    done: bool,
}

impl FrameStream {
    /// Starts decoding `path` at `start` seconds. The first frame delivered
    /// is the first one whose timestamp is not before `start` (within a
    /// millisecond). Returns as soon as the decoder process is running;
    /// problems with the file's contents surface from [`FrameStream::next_frame`].
    pub fn open(path: &Path, start: f64, max_w: u32, max_h: u32) -> Result<Self> {
        if max_w == 0 || max_h == 0 {
            bail!("preview size must not be zero ({max_w}x{max_h})");
        }
        if !start.is_finite() {
            bail!("invalid start time for video preview");
        }
        match std::fs::metadata(path) {
            Ok(m) if m.is_file() => {}
            Ok(_) => bail!("not a video file: {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!("video file not found: {}", path.display()),
            Err(e) => bail!("cannot read video file {}: {e}", path.display()),
        }
        let ffmpeg = mango_core::tools::find_tool("ffmpeg").ok_or_else(|| anyhow!("ffmpeg was not found; it is required to preview video"))?;
        let start = start.max(0.0);
        // A millisecond early so a start exactly on a frame keeps that frame
        // despite rounding inside ffmpeg; `next_frame` drops what is too early.
        let seek_arg = format!("{:.6}", (start - 0.001).max(0.0));
        // Display size: square pixels, shrunk to fit, never enlarged.
        let k = format!("min(1,min({max_w}/(iw*sar),{max_h}/ih))");
        let filter = format!(
            "scale=w='max(1,round(iw*sar*{k}))':h='max(1,round(ih*{k}))':flags=fast_bilinear,setsar=1,format=bgra,showinfo=checksum=0"
        );
        let mut child = Command::new(ffmpeg)
            .args(["-hide_banner", "-nostats", "-nostdin", "-loglevel", "level+info"])
            // `-copyts` keeps the source's own timestamps, so a frame's time does
            // not depend on where the decode was started.
            .args(["-ss", &seek_arg, "-copyts", "-i"])
            .arg(path)
            .args(["-map", "0:v:0", "-an", "-sn", "-dn", "-fps_mode", "passthrough"])
            .args(["-vf", &filter, "-f", "rawvideo", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("cannot run ffmpeg")?;
        let pid = child.id();
        let (stdout, stderr) = match (child.stdout.take(), child.stderr.take()) {
            (Some(o), Some(e)) => (o, e),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("ffmpeg started without pipes");
            }
        };
        let errors = Arc::new(Mutex::new(Vec::new()));
        let (tx, info) = channel();
        let stderr_thread = {
            let errors = errors.clone();
            std::thread::Builder::new().name("mango-video-info".into()).spawn(move || read_info(stderr, tx, errors))
        };
        let stderr_thread = match stderr_thread {
            Ok(t) => t,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("cannot start the video decoder thread: {e}");
            }
        };
        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            pid,
            stdout,
            info,
            stderr_thread: Some(stderr_thread),
            errors,
            path: path.to_path_buf(),
            start,
            container_start: 0.0,
            time_base: None,
            last_pts: None,
            delivered: 0,
            done: false,
        })
    }

    /// Process id of the decoder.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The next frame, blocking until it is decoded. `Ok(None)` at the end of
    /// the file (and from then on).
    pub fn next_frame(&mut self) -> Result<Option<VideoFrame>> {
        if self.done {
            return Ok(None);
        }
        loop {
            let (pts_ticks, width, height) = loop {
                match self.info.recv() {
                    Ok(Info::ContainerStart(t)) => self.container_start = t,
                    Ok(Info::TimeBase(n, d)) => self.time_base = Some((n, d)),
                    Ok(Info::Frame { pts, width, height }) => break (pts, width, height),
                    Err(_) => {
                        // The decoder closed its log: it has exited.
                        self.done = true;
                        let status = lock(&self.child).wait().context("waiting for ffmpeg failed")?;
                        if status.success() {
                            return Ok(None);
                        }
                        bail!("cannot decode video of {}: {}", self.path.display(), self.error_text());
                    }
                }
            };
            let bytes = width as usize * height as usize * 4;
            if bytes == 0 || bytes > MAX_FRAME_BYTES {
                self.done = true;
                bail!("video decoder reported an unusable frame size {width}x{height} for {}", self.path.display());
            }
            let mut bgra = vec![0u8; bytes];
            if let Err(e) = self.stdout.read_exact(&mut bgra) {
                self.done = true;
                bail!("video stream of {} ended unexpectedly: {e}", self.path.display());
            }
            let pts = match (pts_ticks, self.time_base) {
                (Some(p), Some((n, d))) if d != 0 => p as f64 * n as f64 / d as f64 - self.container_start,
                // No timestamp on this frame: place it right after the previous one.
                _ => self.last_pts.map(|p| p + 1.0 / 30.0).unwrap_or(self.start),
            };
            if pts < self.start - EPS && self.delivered == 0 {
                // Before the requested time (the decode starts a moment early).
                continue;
            }
            self.last_pts = Some(pts);
            self.delivered += 1;
            return Ok(Some(VideoFrame { width, height, bgra, pts }));
        }
    }

    fn error_text(&self) -> String {
        let e = lock(&self.errors);
        if e.is_empty() { "ffmpeg failed".to_string() } else { e.join("; ") }
    }

    /// A handle that can kill the decoder from another thread, which makes a
    /// blocked `next_frame` return.
    fn killer(&self) -> Arc<Mutex<Child>> {
        self.child.clone()
    }
}

impl Drop for FrameStream {
    fn drop(&mut self) {
        {
            let mut child = lock(&self.child);
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
    }
}

/// Reads ffmpeg's log: frame descriptions from `showinfo` go to `tx`, error
/// lines are kept for messages.
fn read_info(stderr: std::process::ChildStderr, tx: Sender<Info>, errors: Arc<Mutex<Vec<String>>>) {
    let mut reader = BufReader::new(stderr);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end();
        if !line.contains("Parsed_showinfo") && line.contains("Duration:") {
            // "Duration: 00:00:04.00, start: 0.000000, bitrate: ..." of the input.
            if let Some(v) = after(line, "start:").and_then(|r| r.trim_start().split(',').next()).and_then(|v| v.trim().parse::<f64>().ok()) {
                let _ = tx.send(Info::ContainerStart(v));
            }
        } else if line.contains("Parsed_showinfo") {
            if let Some(rest) = after(line, "config in time_base:") {
                let tb = rest.trim_start().split(',').next().unwrap_or("");
                if let Some((n, d)) = tb.split_once('/')
                    && let (Ok(n), Ok(d)) = (n.trim().parse::<i64>(), d.trim().parse::<i64>()) {
                        // A closed receiver only means the stream was dropped; keep draining the pipe.
                        let _ = tx.send(Info::TimeBase(n, d));
                    }
            } else if let (Some(_), Some(pts), Some(size)) = (after(line, " n:"), after(line, " pts:"), after(line, " s:")) {
                let pts = pts.split_whitespace().next().and_then(|v| v.parse::<i64>().ok());
                let size = size.split_whitespace().next().unwrap_or("");
                if let Some((w, h)) = size.split_once('x')
                    && let (Ok(width), Ok(height)) = (w.parse::<u32>(), h.parse::<u32>()) {
                        let _ = tx.send(Info::Frame { pts, width, height });
                    }
            }
        } else if line.contains("[error]") || line.contains("[fatal]") {
            let mut e = lock(&errors);
            if e.len() < 6 {
                // Drop the "[component @ 0x...] [error] " prefix.
                let msg = line.rsplit_once("] ").map(|(_, m)| m).unwrap_or(line).trim();
                if !msg.is_empty() {
                    e.push(msg.to_string());
                }
            }
        }
    }
}

fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.find(key).map(|i| &line[i + key.len()..])
}

// ---------------------------------------------------------------------------
// Feed: a FrameStream on its own thread with a small read-ahead buffer
// ---------------------------------------------------------------------------

/// What the preview should show.
#[derive(Clone, Debug)]
pub enum Picture {
    /// The frame for the requested time.
    Frame(Arc<VideoFrame>),
    /// Nothing to show: a gap clip, an audio-only source, or a source that cannot be decoded.
    Blank,
    /// The decoder is still catching up; keep showing the previous picture.
    Pending,
}

type Callback = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Notifier {
    callback: Mutex<Option<Callback>>,
}

impl Notifier {
    fn fire(&self) {
        let cb = lock(&self.callback).clone();
        if let Some(cb) = cb {
            cb();
        }
    }
}

struct FeedShared {
    buf: Mutex<FeedBuf>,
    /// Signalled when the consumer makes room or the feed is stopped.
    space: Condvar,
    stop: AtomicBool,
    /// The consumer was told "pending" and wants to hear about progress.
    waiting: AtomicBool,
}

struct FeedBuf {
    frames: VecDeque<Arc<VideoFrame>>,
    capacity: usize,
    /// Frames the consumer has already moved past.
    popped: u64,
    /// Timestamp of the newest decoded frame.
    decoded_until: Option<f64>,
    eof: bool,
    error: Option<String>,
    killer: Option<Arc<Mutex<Child>>>,
}

struct Feed {
    path: PathBuf,
    /// Requested start time.
    start: f64,
    shared: Arc<FeedShared>,
    thread: Option<JoinHandle<()>>,
}

enum Lookup {
    Frame(Arc<VideoFrame>),
    Pending,
    Failed(String),
    /// The source has no frame at or after the requested time.
    Empty,
}

impl Feed {
    fn spawn(path: &Path, start: f64, max_w: u32, max_h: u32, notifier: Arc<Notifier>) -> Feed {
        let shared = Arc::new(FeedShared {
            buf: Mutex::new(FeedBuf { frames: VecDeque::new(), capacity: 3, popped: 0, decoded_until: None, eof: false, error: None, killer: None }),
            space: Condvar::new(),
            stop: AtomicBool::new(false),
            waiting: AtomicBool::new(false),
        });
        let thread = {
            let (shared, path) = (shared.clone(), path.to_path_buf());
            std::thread::Builder::new().name("mango-video-decode".into()).spawn(move || feed_loop(&shared, &path, start, max_w, max_h, &notifier))
        };
        let thread = match thread {
            Ok(t) => Some(t),
            Err(e) => {
                lock(&shared.buf).error = Some(format!("cannot start a video decoder thread: {e}"));
                None
            }
        };
        Feed { path: path.to_path_buf(), start, shared, thread }
    }

    /// Whether this feed can produce the frame for source time `s` by reading on.
    fn serves(&self, s: f64, playing: bool) -> bool {
        let b = lock(&self.shared.buf);
        let lo = match b.frames.front() {
            Some(f) if b.popped > 0 => f.pts - EPS,
            _ => self.start - EPS,
        };
        if s < lo {
            return false;
        }
        if b.eof || b.error.is_some() {
            // Nothing more will come; a restart would not change the answer
            // for later times, only for ones outside what was decoded.
            return b.error.is_none() || s <= self.start + SKIP_AHEAD_PAUSED;
        }
        let window = if playing { SKIP_AHEAD_PLAYING } else { SKIP_AHEAD_PAUSED };
        s <= b.decoded_until.unwrap_or(self.start).max(self.start) + window
    }

    /// The frame to show at source time `s`; frames before `floor` are never shown.
    fn frame_at(&self, s: f64, floor: f64, playing: bool) -> Lookup {
        let mut b = lock(&self.shared.buf);
        if let Some(e) = &b.error {
            return Lookup::Failed(e.clone());
        }
        let before = b.frames.len();
        while b.frames.len() >= 2 && (b.frames[0].pts < floor - EPS || b.frames[1].pts <= s + EPS) {
            b.frames.pop_front();
            b.popped += 1;
        }
        if b.frames.len() != before {
            self.shared.space.notify_all();
        }
        let more = b.frames.len() >= 2;
        let result = match b.frames.front() {
            None if b.eof => Lookup::Empty,
            None => Lookup::Pending,
            Some(f) => {
                if f.pts < floor - EPS && !b.eof {
                    // Still before the clip; the first usable frame is on its way.
                    Lookup::Pending
                } else if f.pts > s + EPS || more || b.eof || playing {
                    // Either the first frame of the stream, or the latest one
                    // not after `s` (known for sure once its successor exists;
                    // while playing the newest decoded frame is good enough).
                    Lookup::Frame(f.clone())
                } else {
                    Lookup::Pending
                }
            }
        };
        if matches!(result, Lookup::Pending) {
            self.shared.waiting.store(true, SeqCst);
        }
        result
    }

    /// Stops decoding. The returned handle finishes promptly.
    fn stop(mut self) -> Option<JoinHandle<()>> {
        self.signal_stop();
        self.thread.take()
    }

    fn signal_stop(&self) {
        self.shared.stop.store(true, SeqCst);
        let b = lock(&self.shared.buf);
        if let Some(k) = &b.killer {
            // Unblocks a read in progress; the feed thread reaps the process.
            let _ = lock(k).kill();
        }
        drop(b);
        self.shared.space.notify_all();
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.signal_stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn feed_loop(shared: &FeedShared, path: &Path, start: f64, max_w: u32, max_h: u32, notifier: &Notifier) {
    let fail = |msg: String| {
        lock(&shared.buf).error = Some(msg);
        shared.waiting.store(false, SeqCst);
        notifier.fire();
    };
    let mut stream = match FrameStream::open(path, start, max_w, max_h) {
        Ok(s) => s,
        Err(e) => return fail(format!("{e:#}")),
    };
    {
        let mut b = lock(&shared.buf);
        if shared.stop.load(SeqCst) {
            return;
        }
        b.killer = Some(stream.killer());
    }
    loop {
        if shared.stop.load(SeqCst) {
            return;
        }
        match stream.next_frame() {
            Ok(Some(frame)) => {
                let mut b = lock(&shared.buf);
                if b.decoded_until.is_none() {
                    b.capacity = (BUFFER_BYTES / frame.bgra.len().max(1)).clamp(3, 8);
                }
                while b.frames.len() >= b.capacity && !shared.stop.load(SeqCst) {
                    b = shared.space.wait(b).unwrap_or_else(|e| e.into_inner());
                }
                if shared.stop.load(SeqCst) {
                    return;
                }
                b.decoded_until = Some(frame.pts);
                b.frames.push_back(Arc::new(frame));
                drop(b);
                if shared.waiting.swap(false, SeqCst) {
                    notifier.fire();
                }
            }
            Ok(None) => {
                lock(&shared.buf).eof = true;
                shared.waiting.store(false, SeqCst);
                notifier.fire();
                return;
            }
            Err(e) => {
                if !shared.stop.load(SeqCst) {
                    fail(format!("{e:#}"));
                }
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Track: the current stream, a pre-opened next one, and retired ones
// ---------------------------------------------------------------------------

struct Track {
    max_w: u32,
    max_h: u32,
    notifier: Arc<Notifier>,
    cur: Option<Feed>,
    next: Option<Feed>,
    retired: Vec<JoinHandle<()>>,
    last_error: Option<String>,
    last_frame: Option<Arc<VideoFrame>>,
}

impl Track {
    fn new(max_w: u32, max_h: u32, notifier: Arc<Notifier>) -> Self {
        Self { max_w: max_w.max(1), max_h: max_h.max(1), notifier, cur: None, next: None, retired: Vec::new(), last_error: None, last_frame: None }
    }

    fn retire(&mut self, feed: Option<Feed>) {
        if let Some(h) = feed.and_then(Feed::stop) {
            self.retired.push(h);
        }
    }

    /// Joins decoder threads that have finished stopping.
    fn reap(&mut self) {
        let mut i = 0;
        while i < self.retired.len() {
            if self.retired[i].is_finished() {
                let _ = self.retired.swap_remove(i).join();
            } else {
                i += 1;
            }
        }
    }

    /// Makes sure a stream starting at `start` of `path` is being prepared.
    fn preopen(&mut self, path: &Path, start: f64) {
        if self.next.as_ref().is_some_and(|f| f.path == path && (f.start - start).abs() < SAME_START) {
            return;
        }
        let old = self.next.take();
        self.retire(old);
        self.next = Some(Feed::spawn(path, start, self.max_w, self.max_h, self.notifier.clone()));
    }

    fn cancel_preopen(&mut self) {
        let old = self.next.take();
        self.retire(old);
    }

    /// The picture for source time `s` of `path`. `floor` is the earliest
    /// source time that may be shown; `lead` is how much earlier than `s` a
    /// new stream starts so the frame covering `s` is found. `entering`: the
    /// playhead has just entered a clip that starts at this source time, so a
    /// stream that was pre-opened for exactly that takes over.
    fn picture(&mut self, path: &Path, s: f64, floor: f64, lead: f64, playing: bool, entering: Option<f64>) -> Picture {
        let fits = |f: &Option<Feed>| f.as_ref().is_some_and(|f| f.path == path && f.serves(s, playing));
        let prepared = entering.is_some_and(|start| self.next.as_ref().is_some_and(|f| (f.start - start).abs() < SAME_START));
        if prepared && fits(&self.next) || !fits(&self.cur) && fits(&self.next) {
            let old = self.cur.take();
            self.retire(old);
            self.cur = self.next.take();
        } else if !fits(&self.cur) {
            let old = self.cur.take();
            self.retire(old);
            let start = (s - lead).max(floor).max(0.0);
            self.cur = Some(Feed::spawn(path, start, self.max_w, self.max_h, self.notifier.clone()));
        }
        let Some(cur) = &self.cur else { return Picture::Pending };
        match cur.frame_at(s, floor, playing) {
            Lookup::Frame(f) => {
                self.last_frame = Some(f.clone());
                Picture::Frame(f)
            }
            Lookup::Pending => Picture::Pending,
            Lookup::Empty => Picture::Blank,
            Lookup::Failed(e) => {
                self.last_error = Some(e);
                Picture::Blank
            }
        }
    }

    fn shutdown(&mut self) {
        let (cur, next) = (self.cur.take(), self.next.take());
        self.retire(cur);
        self.retire(next);
        for h in self.retired.drain(..) {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// ScriptVideo
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
struct VideoClip {
    /// `None`: nothing to show (gap clip, audio-only or hidden source).
    path: Option<PathBuf>,
    src_start: f64,
    src_end: f64,
    comp_start: f64,
    /// Duration of one source frame, or a safe guess.
    frame: f64,
}

struct ScriptState {
    clips: Vec<VideoClip>,
    total: f64,
    track: Track,
    /// Index of the clip shown by the previous request.
    last_clip: Option<usize>,
}

/// The script track's picture at any composition time.
///
/// Call [`ScriptVideo::set_clips`] after every edit and
/// [`ScriptVideo::request`] every UI frame with the playhead. Decoding runs
/// on worker threads; `request` never blocks. Dropping stops all decoders.
pub struct ScriptVideo {
    state: Mutex<ScriptState>,
    notifier: Arc<Notifier>,
}

impl ScriptVideo {
    /// Frames are scaled down to fit `max_w` x `max_h`.
    pub fn new(max_w: u32, max_h: u32) -> Self {
        let notifier = Arc::new(Notifier::default());
        Self { state: Mutex::new(ScriptState { clips: Vec::new(), total: 0.0, track: Track::new(max_w, max_h, notifier.clone()), last_clip: None }), notifier }
    }

    /// Takes the script track of `comp` as it is now. Streams that still fit
    /// keep running; the rest are replaced at the next `request`.
    pub fn set_clips(&self, project: &Project, comp: &Composition) {
        let mut clips = Vec::new();
        let mut total: f64 = 0.0;
        for c in comp.script_clips() {
            let d = c.duration();
            if d.is_nan() || d <= 0.0 || !c.comp_start.is_finite() {
                continue;
            }
            let placement = comp.scenes.get(c.scene).and_then(|s| s.script_video.as_ref()).unwrap_or(&comp.script_video);
            let asset = c.asset.and_then(|id| project.asset(id)).filter(|a| a.has_video && a.kind == MediaKind::Video && !placement.hidden);
            let frame = asset.map(|a| a.fps).filter(|f| f.is_finite() && *f >= 1.0 && *f <= 1000.0).map(|f| 1.0 / f).unwrap_or(1.0 / 15.0);
            clips.push(VideoClip { path: asset.map(|a| a.path.clone()), src_start: c.src_start, src_end: c.src_end, comp_start: c.comp_start, frame });
            total = total.max(c.comp_start + d);
        }
        let mut st = lock(&self.state);
        if st.clips != clips {
            st.clips = clips;
            st.total = total;
            st.last_clip = None;
        }
    }

    /// The picture for composition time `t`. `playing` says whether the
    /// playhead is moving (read ahead, prepare upcoming cuts, prefer a
    /// slightly late frame over none) or parked (deliver exactly the frame
    /// at `t`). Returns immediately.
    pub fn request(&self, t: f64, playing: bool) -> Picture {
        let mut guard = lock(&self.state);
        let st = &mut *guard;
        st.track.reap();
        if st.clips.is_empty() {
            return Picture::Blank;
        }
        // Not-a-number is treated as the start; anything past the end as the end.
        let t = if t.is_nan() { 0.0 } else { t.max(0.0).min(st.total) };
        // The last clip that starts at or before `t`.
        let idx = st.clips.partition_point(|c| c.comp_start <= t).saturating_sub(1);
        let clip = st.clips[idx].clone();
        // At (or past) the very end, keep showing the last frame.
        let s = (clip.src_start + (t - clip.comp_start).max(0.0)).min(clip.src_end - 0.001).max(clip.src_start);
        let changed_clip = st.last_clip != Some(idx);
        st.last_clip = Some(idx);

        // The picture first: entering a clip takes over the stream that was prepared for it.
        let pic = match &clip.path {
            Some(path) => st.track.picture(path, s, clip.src_start, clip.frame * 1.5, playing, changed_clip.then_some(clip.src_start)),
            None => Picture::Blank,
        };

        // Shortly before a cut, start decoding what comes after it, so the cut does not stall.
        let comp_end = clip.comp_start + (clip.src_end - clip.src_start);
        let upcoming = st.clips.get(idx + 1).filter(|_| playing && comp_end - t <= PREOPEN_SECS);
        match upcoming {
            Some(VideoClip { path: Some(next_path), src_start, .. })
                if !(clip.path.as_ref() == Some(next_path) && (src_start - clip.src_end).abs() < clip.frame / 2.0) =>
            {
                st.track.preopen(next_path, *src_start);
            }
            // Nothing to prepare: the source simply continues, a gap follows, or the playhead is parked.
            _ => st.track.cancel_preopen(),
        }
        pic
    }

    /// Registers a function that is called from a worker thread when a
    /// picture that was [`Picture::Pending`] may now be available, so the UI
    /// can redraw and call `request` again. Keep it cheap and non-blocking.
    pub fn set_on_frame(&self, f: Box<dyn Fn() + Send + Sync>) {
        *lock(&self.notifier.callback) = Some(Arc::from(f));
    }

    /// Stops all decoders and frees their buffers, for when the preview is
    /// not visible. The next `request` starts them again.
    pub fn release(&self) {
        let mut st = lock(&self.state);
        st.track.shutdown();
        st.last_clip = None;
    }

    /// The most recently shown frame. While scrubbing returns `Pending`, this
    /// is what should stay on screen.
    pub fn last_frame(&self) -> Option<Arc<VideoFrame>> {
        lock(&self.state).track.last_frame.clone()
    }

    /// The last decoder error (for example an unreadable source), if any. Reading clears it.
    pub fn take_error(&self) -> Option<String> {
        lock(&self.state).track.last_error.take()
    }
}

impl Drop for ScriptVideo {
    fn drop(&mut self) {
        *lock(&self.notifier.callback) = None;
        lock(&self.state).track.shutdown();
    }
}

// ---------------------------------------------------------------------------
// LayerVideo
// ---------------------------------------------------------------------------

/// The picture of one video file at any source time, for video layers.
///
/// The caller maps composition time to source time
/// (`src_in + (t - layer.start) * speed`, wrapped for loops).
pub struct LayerVideo {
    path: PathBuf,
    track: Mutex<Track>,
    notifier: Arc<Notifier>,
}

impl LayerVideo {
    pub fn new(path: &Path, max_w: u32, max_h: u32) -> Self {
        let notifier = Arc::new(Notifier::default());
        Self { path: path.to_path_buf(), track: Mutex::new(Track::new(max_w, max_h, notifier.clone())), notifier }
    }

    /// The picture at `source_time` seconds of the file. Returns immediately.
    /// Past the last frame of the file the picture is [`Picture::Blank`].
    pub fn request(&self, source_time: f64, playing: bool) -> Picture {
        let s = if source_time.is_finite() { source_time.max(0.0) } else { 0.0 };
        let mut track = lock(&self.track);
        track.reap();
        // The frame rate is not known here: start a tenth of a second early
        // and let the stream run up to the frame that covers `s`.
        track.picture(&self.path, s, 0.0, 0.1, playing, None)
    }

    /// See [`ScriptVideo::release`].
    pub fn release(&self) {
        lock(&self.track).shutdown();
    }

    /// See [`ScriptVideo::set_on_frame`].
    pub fn set_on_frame(&self, f: Box<dyn Fn() + Send + Sync>) {
        *lock(&self.notifier.callback) = Some(Arc::from(f));
    }

    pub fn last_frame(&self) -> Option<Arc<VideoFrame>> {
        lock(&self.track).last_frame.clone()
    }

    pub fn take_error(&self) -> Option<String> {
        lock(&self.track).last_error.take()
    }
}

impl Drop for LayerVideo {
    fn drop(&mut self) {
        *lock(&self.notifier.callback) = None;
        lock(&self.track).shutdown();
    }
}
