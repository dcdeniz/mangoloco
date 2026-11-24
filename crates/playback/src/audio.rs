//! Realtime audio output.
//!
//! [`AudioEngine`] plays a [`Timeline`] through the default output device and
//! is the master clock of the preview: [`AudioEngine::position`] is the
//! composition time that has actually been handed to the device.
//! [`NullEngine`] is the same transport without a device, advanced by an
//! explicit clock; it is used by tests and when the machine has no output.
//!
//! The audio callback never blocks: it reads the timeline through an atomic
//! pointer swap, reads commands from atomics, renders with [`mix::render`]
//! into preallocated buffers, and never frees a timeline (the control side
//! keeps every timeline alive until the callback has let go of it).
//!
//! Every discontinuity (play, pause, seek, a new timeline) is crossfaded over
//! [`XFADE_FRAMES`] so none of them clicks.

use crate::mix::{self, Timeline};
use crate::pcm::SAMPLE_RATE;
use anyhow::{Context, Result, anyhow};
use arc_swap::ArcSwap;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const SR: f64 = SAMPLE_RATE as f64;

/// Length of the crossfade applied at play, pause, seek and timeline swaps: 5 ms.
pub const XFADE_FRAMES: usize = 240;
/// Preview speed limits.
pub const MIN_RATE: f32 = 0.5;
pub const MAX_RATE: f32 = 3.0;

const CHUNK: usize = 256;
/// Input frames available to the resampler for one chunk.
const SCRATCH: usize = CHUNK * 24 + 8;
/// Time for the level meter to fall by a factor of e.
const LEVEL_DECAY_SECS: f32 = 0.25;
/// The output stream is stopped after this long without playback, so an idle
/// editor does not keep the audio hardware (and the machine) awake.
const IDLE_STOP: Duration = Duration::from_millis(1500);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The transport interface shared by [`AudioEngine`] and [`NullEngine`].
pub trait Engine: Send + Sync {
    /// Replaces what is playing. Playback continues from the same composition
    /// time, crossfaded so the swap does not click.
    fn set_timeline(&self, tl: Timeline);
    /// Starts playback. At the end of the timeline it starts over from 0.
    fn play(&self);
    fn pause(&self);
    fn is_playing(&self) -> bool;
    /// Jumps to a composition time in seconds (clamped to the timeline).
    fn seek(&self, secs: f64);
    /// Composition time, in seconds, that has been sent to the output.
    fn position(&self) -> f64;
    /// True once playback has run into the end of the timeline; cleared by `seek` and `play`.
    fn finished(&self) -> bool;
    /// Preview speed, clamped to `0.5..=3.0`. The pitch follows the speed.
    fn set_rate(&self, rate: f32);
    fn rate(&self) -> f32;
    /// Output volume, linear, clamped to `0.0..=2.0`.
    fn set_volume(&self, linear: f32);
    /// Recent peak level (left, right) of the programme before the volume
    /// control, linear, falling back to zero over about a second.
    fn levels(&self) -> (f32, f32);
    /// Length of the current timeline in seconds.
    fn duration(&self) -> f64;
}

// ---------------------------------------------------------------------------
// State shared between the control side and the renderer
// ---------------------------------------------------------------------------

struct Shared {
    timeline: ArcSwap<Timeline>,
    playing: AtomicBool,
    finished: AtomicBool,
    /// Position in frames as `f64` bits; written by the renderer.
    pos: AtomicU64,
    /// Sequence lock for seeks: odd while `seek_target` is being written.
    seek_seq: AtomicU64,
    seek_target: AtomicU64,
    /// Last `seek_seq` the renderer has applied.
    seek_applied: AtomicU64,
    rate: AtomicU32,
    volume: AtomicU32,
    level_l: AtomicU32,
    level_r: AtomicU32,
    /// Control side only; the renderer never takes this.
    control: Mutex<Control>,
}

#[derive(Default)]
struct Control {
    /// Every timeline the renderer may still be holding. Keeping a reference
    /// here means the renderer never drops the last one (which would free
    /// memory and unmap files on the audio thread).
    retained: Vec<Arc<Timeline>>,
}

impl Shared {
    fn new() -> Arc<Self> {
        let empty = Arc::new(Timeline::default());
        Arc::new(Self {
            timeline: ArcSwap::new(empty.clone()),
            playing: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            pos: AtomicU64::new(0f64.to_bits()),
            seek_seq: AtomicU64::new(0),
            seek_target: AtomicU64::new(0f64.to_bits()),
            seek_applied: AtomicU64::new(0),
            rate: AtomicU32::new(1f32.to_bits()),
            volume: AtomicU32::new(1f32.to_bits()),
            level_l: AtomicU32::new(0f32.to_bits()),
            level_r: AtomicU32::new(0f32.to_bits()),
            control: Mutex::new(Control { retained: vec![empty] }),
        })
    }

    fn set_timeline(&self, tl: Timeline) {
        let tl = Arc::new(tl);
        let mut c = lock(&self.control);
        c.retained.push(tl.clone());
        self.timeline.store(tl);
        Self::release_unused(&mut c);
    }

    /// Drops timelines nobody else references any more.
    fn release_unused(c: &mut Control) {
        c.retained.retain(|t| Arc::strong_count(t) > 1);
    }

    fn collect_garbage(&self) {
        Self::release_unused(&mut lock(&self.control));
    }

    /// Position in frames, taking a seek the renderer has not applied yet into account.
    fn position_frames(&self) -> f64 {
        loop {
            let s = self.seek_seq.load(SeqCst);
            if s % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            if s == self.seek_applied.load(SeqCst) {
                let p = f64::from_bits(self.pos.load(SeqCst));
                // A seek may have been requested and applied in between; then `p` is still current.
                return p;
            }
            let t = f64::from_bits(self.seek_target.load(SeqCst));
            if self.seek_seq.load(SeqCst) == s {
                return t;
            }
        }
    }

    fn seek_frames(&self, frame: f64) {
        let _c = lock(&self.control);
        self.seek_seq.fetch_add(1, SeqCst);
        self.seek_target.store(frame.to_bits(), SeqCst);
        self.seek_seq.fetch_add(1, SeqCst);
    }

    fn seek(&self, secs: f64) {
        let end = self.timeline.load().frames() as f64;
        let secs = if secs.is_finite() { secs } else { 0.0 };
        self.seek_frames((secs * SR).round().clamp(0.0, end));
        self.finished.store(false, SeqCst);
    }

    fn play(&self) {
        let end = self.timeline.load().frames() as f64;
        if self.finished.load(SeqCst) || self.position_frames() >= end {
            self.seek_frames(0.0);
        }
        self.finished.store(false, SeqCst);
        self.playing.store(true, SeqCst);
    }

    fn pause(&self) {
        self.playing.store(false, SeqCst);
    }

    fn position(&self) -> f64 {
        let tl = self.timeline.load();
        let p = self.position_frames();
        if p >= tl.frames() as f64 { tl.duration.max(0.0) } else { p / SR }
    }

    fn rate(&self) -> f32 {
        f32::from_bits(self.rate.load(SeqCst))
    }

    fn set_rate(&self, rate: f32) {
        let r = if rate.is_finite() { rate.clamp(MIN_RATE, MAX_RATE) } else { 1.0 };
        self.rate.store(r.to_bits(), SeqCst);
    }

    fn set_volume(&self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 2.0) } else { 0.0 };
        self.volume.store(v.to_bits(), SeqCst);
    }

    fn levels(&self) -> (f32, f32) {
        (f32::from_bits(self.level_l.load(SeqCst)), f32::from_bits(self.level_r.load(SeqCst)))
    }

    fn clear_levels(&self) {
        self.level_l.store(0f32.to_bits(), SeqCst);
        self.level_r.store(0f32.to_bits(), SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Renderer: runs on the audio thread (or under NullEngine's clock)
// ---------------------------------------------------------------------------

struct Renderer {
    shared: Arc<Shared>,
    tl: Arc<Timeline>,
    /// Position in composition frames. Whole numbers while the step is 1.
    pos: f64,
    was_playing: bool,
    seek_applied: u64,
    volume: f32,
    level: [f32; 2],
    scratch: Box<[[f32; 2]]>,
    xfade: Box<[[f32; 2]]>,
}

impl Renderer {
    fn new(shared: Arc<Shared>) -> Self {
        let tl = shared.timeline.load_full();
        let volume = f32::from_bits(shared.volume.load(SeqCst));
        Self {
            shared,
            tl,
            pos: 0.0,
            was_playing: false,
            seek_applied: 0,
            volume,
            level: [0.0; 2],
            scratch: vec![[0.0; 2]; SCRATCH].into_boxed_slice(),
            xfade: vec![[0.0; 2]; XFADE_FRAMES].into_boxed_slice(),
        }
    }

    /// Produces the next `out.len()` output frames. `device_step` is the
    /// number of 48 kHz frames per output frame (1.0 for a 48 kHz device).
    fn process(&mut self, out: &mut [[f32; 2]], device_step: f64) {
        if out.is_empty() {
            return;
        }
        // An atomic increment; the renderer never holds the last reference.
        let shared = Arc::clone(&self.shared);

        // --- what should be playing now --------------------------------------
        let new_tl = {
            let cur = shared.timeline.load();
            if Arc::ptr_eq(&cur, &self.tl) { None } else { Some(Arc::clone(&cur)) }
        };
        let mut seek = None;
        let seq = shared.seek_seq.load(SeqCst);
        if seq != self.seek_applied && seq.is_multiple_of(2) {
            let target = f64::from_bits(shared.seek_target.load(SeqCst));
            if shared.seek_seq.load(SeqCst) == seq {
                seek = Some(target);
            }
        }
        let playing = shared.playing.load(SeqCst);
        let rate = f32::from_bits(shared.rate.load(SeqCst)).clamp(MIN_RATE, MAX_RATE) as f64;
        let step = rate * device_step;

        // --- the tail of what was playing, to fade out -----------------------
        let discontinuity = new_tl.is_some() || seek.is_some() || playing != self.was_playing;
        let x = if discontinuity { XFADE_FRAMES.min(out.len()) } else { 0 };
        if x > 0 {
            if self.was_playing {
                let mut p = self.pos;
                render_stream(&self.tl, &mut p, step, &mut self.xfade[..x], &mut self.scratch);
            } else {
                self.xfade[..x].fill([0.0, 0.0]);
            }
        }
        if let Some(tl) = new_tl {
            // The control side still holds the old timeline, so this is never the last reference.
            self.tl = tl;
        }
        if let Some(p) = seek {
            self.pos = p;
            self.seek_applied = seq;
        }

        // --- the new stream ---------------------------------------------------
        let mut ended = false;
        if playing {
            ended = render_stream(&self.tl, &mut self.pos, step, out, &mut self.scratch);
        } else {
            out.fill([0.0, 0.0]);
        }
        for i in 0..x {
            let w = i as f32 / x as f32;
            let old = self.xfade[i];
            out[i][0] = out[i][0] * w + old[0] * (1.0 - w);
            out[i][1] = out[i][1] * w + old[1] * (1.0 - w);
        }
        self.was_playing = playing && !ended;

        // --- publish ----------------------------------------------------------
        shared.pos.store(self.pos.to_bits(), SeqCst);
        shared.seek_applied.store(self.seek_applied, SeqCst);
        if ended && shared.seek_seq.load(SeqCst) == seq {
            shared.finished.store(true, SeqCst);
            shared.playing.store(false, SeqCst);
        }

        // --- meter, volume, clip ----------------------------------------------
        let mut peak = [0.0f32; 2];
        let target = f32::from_bits(shared.volume.load(SeqCst));
        let dv = (target - self.volume) / out.len() as f32;
        let mut v = self.volume;
        for o in out.iter_mut() {
            peak[0] = peak[0].max(o[0].abs());
            peak[1] = peak[1].max(o[1].abs());
            v += dv;
            o[0] = limit(o[0] * v);
            o[1] = limit(o[1] * v);
        }
        self.volume = target;
        let decay = (-(out.len() as f32 * device_step as f32) / (LEVEL_DECAY_SECS * SR as f32)).exp();
        for c in 0..2 {
            let l = (self.level[c] * decay).max(peak[c]);
            self.level[c] = if l < 1e-5 { 0.0 } else { l };
        }
        shared.level_l.store(self.level[0].to_bits(), SeqCst);
        shared.level_r.store(self.level[1].to_bits(), SeqCst);
    }
}

/// Renders `out.len()` frames of `tl` starting at `*pos`, advancing `step`
/// timeline frames per output frame, and moves `*pos`. Returns true when the
/// end of the timeline was reached (the rest of `out` is silence).
fn render_stream(tl: &Timeline, pos: &mut f64, step: f64, out: &mut [[f32; 2]], scratch: &mut [[f32; 2]]) -> bool {
    let end_frames = tl.frames();
    let end = end_frames as f64;
    if step == 1.0 {
        let start = pos.floor().max(0.0) as u64;
        let n = (out.len() as u64).min(end_frames.saturating_sub(start)) as usize;
        mix::render(tl, start, &mut out[..n]);
        out[n..].fill([0.0, 0.0]);
        let reached = start + n as u64;
        *pos = if reached >= end_frames { end } else { reached as f64 };
        return reached >= end_frames;
    }
    let mut done = 0;
    while done < out.len() {
        if *pos >= end {
            break;
        }
        let fit = (((scratch.len() - 8) as f64) / step) as usize;
        let left = ((end - *pos) / step).ceil() as usize;
        let m = (out.len() - done).min(CHUNK).min(fit.max(1)).min(left.max(1));
        // `buf` holds timeline frames `base..`, one before and two after every read position.
        let i0 = pos.floor() as u64;
        let last = (*pos + (m - 1) as f64 * step).floor() as u64;
        let len = ((last - i0) as usize + 4).min(scratch.len());
        let buf = &mut scratch[..len];
        let base = if i0 > 0 {
            mix::render(tl, i0 - 1, buf);
            (i0 - 1) as f64
        } else {
            // Nothing precedes the start of the timeline.
            buf[0] = [0.0, 0.0];
            mix::render(tl, 0, &mut buf[1..]);
            -1.0
        };
        for (j, o) in out[done..done + m].iter_mut().enumerate() {
            let rel = *pos + j as f64 * step - base;
            let i = (rel.floor() as usize).clamp(1, len - 3);
            let t = (rel - i as f64).clamp(0.0, 1.0) as f32;
            *o = hermite(buf[i - 1], buf[i], buf[i + 1], buf[i + 2], t);
        }
        *pos += m as f64 * step;
        done += m;
    }
    out[done..].fill([0.0, 0.0]);
    if *pos >= end {
        *pos = end;
        return true;
    }
    false
}

/// Hard limit to full scale; anything that is not a number becomes silence.
#[inline]
fn limit(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.clamp(-1.0, 1.0) }
}

/// Four-point, third-order Hermite interpolation between `b` and `c`.
#[inline]
fn hermite(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2], t: f32) -> [f32; 2] {
    let mut o = [0.0f32; 2];
    for ch in 0..2 {
        let (y0, y1, y2, y3) = (a[ch], b[ch], c[ch], d[ch]);
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        o[ch] = ((c3 * t + c2) * t + c1) * t + y1;
    }
    o
}

macro_rules! delegate_engine {
    ($ty:ty) => {
        impl Engine for $ty {
            fn set_timeline(&self, tl: Timeline) {
                <$ty>::set_timeline(self, tl)
            }
            fn play(&self) {
                <$ty>::play(self)
            }
            fn pause(&self) {
                <$ty>::pause(self)
            }
            fn is_playing(&self) -> bool {
                <$ty>::is_playing(self)
            }
            fn seek(&self, secs: f64) {
                <$ty>::seek(self, secs)
            }
            fn position(&self) -> f64 {
                <$ty>::position(self)
            }
            fn finished(&self) -> bool {
                <$ty>::finished(self)
            }
            fn set_rate(&self, rate: f32) {
                <$ty>::set_rate(self, rate)
            }
            fn rate(&self) -> f32 {
                <$ty>::rate(self)
            }
            fn set_volume(&self, linear: f32) {
                <$ty>::set_volume(self, linear)
            }
            fn levels(&self) -> (f32, f32) {
                <$ty>::levels(self)
            }
            fn duration(&self) -> f64 {
                <$ty>::duration(self)
            }
        }
    };
}

// ---------------------------------------------------------------------------
// NullEngine
// ---------------------------------------------------------------------------

/// A transport without an output device. Time moves only when
/// [`NullEngine::advance`] or [`NullEngine::render`] is called.
pub struct NullEngine {
    shared: Arc<Shared>,
    state: Mutex<NullState>,
}

struct NullState {
    renderer: Renderer,
    /// Fraction of a frame left over from previous `advance` calls.
    carry: f64,
    buf: Vec<[f32; 2]>,
}

impl Default for NullEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl NullEngine {
    pub fn new() -> Self {
        let shared = Shared::new();
        let renderer = Renderer::new(shared.clone());
        Self { shared, state: Mutex::new(NullState { renderer, carry: 0.0, buf: vec![[0.0; 2]; 1024] }) }
    }

    /// Runs the engine for `secs` seconds of output time, discarding the audio.
    pub fn advance(&self, secs: f64) {
        if !(secs.is_finite() && secs > 0.0) {
            return;
        }
        let mut st = lock(&self.state);
        let exact = secs * SR + st.carry;
        let mut frames = exact.floor() as u64;
        st.carry = exact - frames as f64;
        let NullState { renderer, buf, .. } = &mut *st;
        while frames > 0 {
            let n = frames.min(buf.len() as u64) as usize;
            renderer.process(&mut buf[..n], 1.0);
            frames -= n as u64;
        }
    }

    /// Runs the engine for exactly `out.len()` frames at 48 kHz and returns
    /// what an output device would have received.
    pub fn render(&self, out: &mut [[f32; 2]]) {
        lock(&self.state).renderer.process(out, 1.0);
    }

    pub fn set_timeline(&self, tl: Timeline) {
        self.shared.set_timeline(tl);
    }
    /// Starts playback. At the end of the timeline it starts over from 0.
    pub fn play(&self) {
        self.shared.play();
    }
    pub fn pause(&self) {
        self.shared.pause();
    }
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(SeqCst)
    }
    pub fn seek(&self, secs: f64) {
        self.shared.seek(secs);
    }
    pub fn position(&self) -> f64 {
        self.shared.position()
    }
    pub fn finished(&self) -> bool {
        self.shared.finished.load(SeqCst)
    }
    pub fn set_rate(&self, rate: f32) {
        self.shared.set_rate(rate);
    }
    pub fn rate(&self) -> f32 {
        self.shared.rate()
    }
    pub fn set_volume(&self, linear: f32) {
        self.shared.set_volume(linear);
    }
    pub fn levels(&self) -> (f32, f32) {
        self.shared.levels()
    }
    pub fn duration(&self) -> f64 {
        self.shared.timeline.load().duration
    }
}

delegate_engine!(NullEngine);

// ---------------------------------------------------------------------------
// AudioEngine
// ---------------------------------------------------------------------------

/// Realtime output through the system's default output device.
pub struct AudioEngine {
    shared: Arc<Shared>,
    output: Arc<Output>,
    helper: Option<std::thread::JoinHandle<()>>,
    sample_rate: u32,
    channels: u16,
}

struct Output {
    stream: cpal::Stream,
    state: Mutex<OutputState>,
    wake: Condvar,
    stop: AtomicBool,
    xruns: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
    /// Output latency in microseconds, as last reported by the device.
    latency_us: Arc<AtomicU64>,
}

struct OutputState {
    /// Whether we believe the device stream is running.
    running: bool,
    idle_since: Option<Instant>,
}

/// Errors reported asynchronously by the device.
struct ErrorSink {
    shared: Arc<Shared>,
    xruns: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
}

impl AudioEngine {
    /// Opens the default output device. Fails with a clear message when the
    /// machine has no usable output; use [`NullEngine`] then.
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow!("no audio output device is available"))?;
        // The device's own format: asking for another sample rate would
        // reconfigure the hardware for every application. The renderer
        // resamples when the device is not at 48 kHz.
        let supported = device.default_output_config().map_err(|e| anyhow!("the audio output device cannot be used: {e}"))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        if config.channels == 0 || config.sample_rate == 0 {
            return Err(anyhow!("the audio output device reports an unusable format ({} channels at {} Hz)", config.channels, config.sample_rate));
        }

        let shared = Shared::new();
        let xruns = Arc::new(AtomicU64::new(0));
        let error = Arc::new(Mutex::new(None));
        let latency_us = Arc::new(AtomicU64::new(0));
        let renderer = (Renderer::new(shared.clone()), latency_us.clone());
        let sink = ErrorSink { shared: shared.clone(), xruns: xruns.clone(), error: error.clone() };
        use cpal::SampleFormat as F;
        let stream = match format {
            F::F32 => build::<f32>(&device, config, renderer, sink),
            F::F64 => build::<f64>(&device, config, renderer, sink),
            F::I8 => build::<i8>(&device, config, renderer, sink),
            F::I16 => build::<i16>(&device, config, renderer, sink),
            F::I24 => build::<cpal::I24>(&device, config, renderer, sink),
            F::I32 => build::<i32>(&device, config, renderer, sink),
            F::I64 => build::<i64>(&device, config, renderer, sink),
            F::U8 => build::<u8>(&device, config, renderer, sink),
            F::U16 => build::<u16>(&device, config, renderer, sink),
            F::U32 => build::<u32>(&device, config, renderer, sink),
            F::U64 => build::<u64>(&device, config, renderer, sink),
            other => return Err(anyhow!("the audio output device uses an unsupported sample format ({other})")),
        }
        .context("cannot open the audio output device")?;
        // Stay silent and stopped until `play`.
        stream.pause().map_err(|e| anyhow!("cannot control the audio output device: {e}"))?;

        let output = Arc::new(Output {
            stream,
            state: Mutex::new(OutputState { running: false, idle_since: None }),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            xruns,
            error,
            latency_us,
        });
        let helper = {
            let (output, shared) = (output.clone(), shared.clone());
            std::thread::Builder::new()
                .name("mango-audio-helper".into())
                .spawn(move || helper_loop(&output, &shared))
                .context("cannot start the audio helper thread")?
        };
        Ok(Self { shared, output, helper: Some(helper), sample_rate: config.sample_rate, channels: config.channels })
    }

    /// Sample rate of the output device.
    pub fn device_sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channel count of the output device.
    pub fn device_channels(&self) -> u16 {
        self.channels
    }

    /// The most recent error reported by the output device, if any (for
    /// example the device was unplugged and there is no other output).
    /// Reading it clears it.
    pub fn take_error(&self) -> Option<String> {
        lock(&self.output.error).take()
    }

    /// Time between audio being handed to the device (what [`AudioEngine::position`]
    /// counts) and it being heard: the device's buffer and hardware latency,
    /// in seconds. Zero until playback has started. Show the picture for
    /// `position() - output_latency() * rate()` to keep lips in sync on
    /// high-latency outputs such as Bluetooth.
    pub fn output_latency(&self) -> f64 {
        self.output.latency_us.load(SeqCst) as f64 / 1e6
    }

    /// Number of buffer underruns the device has reported.
    pub fn xruns(&self) -> u64 {
        self.output.xruns.load(SeqCst)
    }

    pub fn set_timeline(&self, tl: Timeline) {
        self.shared.set_timeline(tl);
    }

    /// Starts playback. At the end of the timeline it starts over from 0.
    /// If the device cannot be started, playback stays paused and
    /// [`AudioEngine::take_error`] says why.
    pub fn play(&self) {
        let mut st = lock(&self.output.state);
        self.shared.play();
        st.idle_since = None;
        // Always ask: the device layer may have stopped the stream on its own
        // (default device removed). Starting a running stream is a no-op.
        match self.output.stream.play() {
            Ok(()) => st.running = true,
            Err(e) => {
                st.running = false;
                self.shared.pause();
                *lock(&self.output.error) = Some(format!("cannot start audio playback: {e}"));
            }
        }
        drop(st);
        self.output.wake.notify_all();
    }

    pub fn pause(&self) {
        self.shared.pause();
    }
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(SeqCst)
    }
    pub fn seek(&self, secs: f64) {
        self.shared.seek(secs);
        self.output.wake.notify_all();
    }
    pub fn position(&self) -> f64 {
        self.shared.position()
    }
    pub fn finished(&self) -> bool {
        self.shared.finished.load(SeqCst)
    }
    pub fn set_rate(&self, rate: f32) {
        self.shared.set_rate(rate);
    }
    pub fn rate(&self) -> f32 {
        self.shared.rate()
    }
    pub fn set_volume(&self, linear: f32) {
        self.shared.set_volume(linear);
    }
    pub fn levels(&self) -> (f32, f32) {
        self.shared.levels()
    }
    pub fn duration(&self) -> f64 {
        self.shared.timeline.load().duration
    }
}

delegate_engine!(AudioEngine);

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.shared.pause();
        self.output.stop.store(true, SeqCst);
        self.output.wake.notify_all();
        if let Some(h) = self.helper.take() {
            let _ = h.join();
        }
        // Stops the device; the stream (and the renderer inside its callback)
        // is destroyed when `output` drops right after.
        let _ = self.output.stream.pause();
    }
}

/// Housekeeping that must not happen on the audio thread: paging in upcoming
/// audio, releasing replaced timelines, stopping an idle device, and
/// collecting device errors.
fn helper_loop(output: &Output, shared: &Shared) {
    let mut last_prefetch: Option<(usize, u64)> = None;
    loop {
        if output.stop.load(SeqCst) {
            return;
        }
        shared.collect_garbage();

        // Page in the next few seconds whenever the playhead or the timeline moved.
        let tl = shared.timeline.load_full();
        let pos = shared.position_frames().max(0.0) as u64;
        let key = (Arc::as_ptr(&tl) as usize, pos / (SAMPLE_RATE as u64 / 2));
        if last_prefetch != Some(key) {
            last_prefetch = Some(key);
            mix::prefetch(&tl, pos, 4 * SAMPLE_RATE as u64);
        }
        drop(tl);

        let mut st = lock(&output.state);
        if shared.playing.load(SeqCst) {
            st.idle_since = None;
        } else if st.running {
            let since = *st.idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= IDLE_STOP {
                // `play` takes the same lock, so this cannot stop a stream that was just started.
                if let Err(e) = output.stream.pause() {
                    *lock(&output.error) = Some(format!("cannot stop the audio output device: {e}"));
                }
                st.running = false;
                st.idle_since = None;
                shared.clear_levels();
            }
        }
        if output.stop.load(SeqCst) {
            return;
        }
        let (guard, _) = output.wake.wait_timeout(st, Duration::from_millis(100)).unwrap_or_else(|e| e.into_inner());
        drop(guard);
    }
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, renderer: (Renderer, Arc<AtomicU64>), sink: ErrorSink) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let device_step = SR / config.sample_rate as f64;
    let mut block = [[0.0f32; 2]; 512];
    let (mut renderer, latency_us) = renderer;
    let data = move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
        let ts = info.timestamp();
        latency_us.store(ts.playback.saturating_duration_since(ts.callback).as_micros() as u64, SeqCst);
        for frames in data.chunks_mut(channels * block.len()) {
            let n = frames.len() / channels;
            let block = &mut block[..n];
            renderer.process(block, device_step);
            for (frame, s) in frames.chunks_mut(channels).zip(block.iter()) {
                if channels == 1 {
                    frame[0] = T::from_sample((s[0] + s[1]) * 0.5);
                } else {
                    frame[0] = T::from_sample(s[0]);
                    frame[1] = T::from_sample(s[1]);
                    for extra in &mut frame[2..] {
                        *extra = T::EQUILIBRIUM;
                    }
                }
            }
        }
    };
    let on_error = move |e: cpal::Error| match e.kind() {
        cpal::ErrorKind::Xrun => {
            sink.xruns.fetch_add(1, SeqCst);
        }
        // The stream keeps running (on the new default device, or without realtime priority).
        cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied => {}
        _ => {
            // The stream is dead or stopped: do not pretend to be playing.
            sink.shared.pause();
            *lock(&sink.error) = Some(format!("audio output stopped: {e}"));
        }
    };
    device.build_output_stream(config, data, on_error, None).map_err(|e| anyhow!("{e}"))
}
