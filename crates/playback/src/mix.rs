//! The audible part of a composition as a flat list of segments, and the
//! reference mixer that renders any range of it.
//!
//! [`timeline`] is cheap and is meant to be called after every edit.
//! [`render`] is pure: output frame `n` depends only on the timeline and `n`,
//! so rendering in any block size gives identical samples. It does not lock,
//! allocate or do I/O (beyond touching memory-mapped audio), which makes it
//! safe to call from the realtime audio callback.

use crate::pcm::{Pcm, PcmCache, SAMPLE_RATE};
use mango_core::model::{AudioSettings, Composition, LayerContent, MediaKind, Project};
use std::sync::Arc;

const SR: f64 = SAMPLE_RATE as f64;

/// Fade applied on each side of a cut on the script track; the exporter's value.
pub const MICROFADE: f64 = mango_core::render::MICROFADE;

/// One piece of one source, placed in composition time.
#[derive(Clone, Debug)]
pub struct AudioSegment {
    pub pcm: Arc<Pcm>,
    /// Composition time at which the segment starts, seconds.
    pub comp_start: f64,
    /// Length in composition time, seconds.
    pub duration: f64,
    /// Source time that plays at `comp_start`, seconds.
    pub src_start: f64,
    /// Source seconds consumed per composition second. Values other than 1
    /// are resampled with linear interpolation (the pitch shifts).
    pub speed: f64,
    /// Linear gain.
    pub gain: f32,
    /// -1 = hard left, 0 = centre, 1 = hard right (balance, as in export).
    pub pan: f32,
    /// Linear fade lengths in seconds, inside the segment.
    pub fade_in: f64,
    pub fade_out: f64,
    /// Volume automation: (seconds from the segment start, dB), sorted by
    /// time. Held before the first and after the last point, interpolated
    /// linearly in dB between.
    pub volume_keys: Vec<(f64, f64)>,
    /// At the end of the source, continue from `src_start` again.
    pub looped: bool,
}

/// Everything audible in a composition.
#[derive(Clone, Debug, Default)]
pub struct Timeline {
    pub segments: Vec<AudioSegment>,
    /// Length of the composition in seconds.
    pub duration: f64,
}

impl Timeline {
    /// Length in frames at 48 kHz.
    pub fn frames(&self) -> u64 {
        frame_of(self.duration)
    }
}

/// Nearest frame index of a time in seconds; negative and non-finite times map to 0.
pub fn frame_of(secs: f64) -> u64 {
    if secs.is_finite() && secs > 0.0 { (secs * SR).round() as u64 } else { 0 }
}

fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

fn settings_gain(a: &AudioSettings) -> Option<(f32, f32)> {
    if a.muted {
        return None;
    }
    let gain = db_to_lin(a.gain_db);
    if !gain.is_finite() {
        return None;
    }
    let pan = if a.pan.is_finite() { a.pan.clamp(-1.0, 1.0) } else { 0.0 };
    Some((gain as f32, pan as f32))
}

/// Builds the audible timeline of `comp`: the script clips with microfades at
/// every cut and the script track's gain / mute / pan, plus every audible
/// audio or video layer. Sources whose audio is not loaded in `cache` yet are
/// left out; build again when their decode finishes.
///
/// Not previewed: audio effects, ducking and master loudness (export only).
pub fn timeline(project: &Project, comp: &Composition, cache: &PcmCache) -> Timeline {
    let mut segments: Vec<AudioSegment> = Vec::new();

    // --- script track -------------------------------------------------------
    if let Some((gain, pan)) = settings_gain(&comp.script_audio) {
        // (asset, source end, composition end) of the previous playing clip.
        let mut prev: Option<(mango_core::model::AssetId, f64, f64)> = None;
        let mut prev_pushed = false;
        for clip in comp.script_clips() {
            let d = clip.duration();
            if d.is_nan() || d <= 0.0 {
                continue;
            }
            let Some(asset_id) = clip.asset else {
                prev = None;
                continue;
            };
            // Same source, continuing exactly where the previous clip stopped
            // (a scene boundary, not a cut): keep playing without a fade.
            let seamless = prev.is_some_and(|(a, src_end, comp_end)| a == asset_id && (clip.src_start - src_end).abs() < 0.5 / SR && (clip.comp_start - comp_end).abs() < 0.5 / SR);
            prev = Some((asset_id, clip.src_end, clip.comp_start + d));
            if seamless && prev_pushed {
                if let Some(last) = segments.last_mut() {
                    last.duration = clip.comp_start + d - last.comp_start;
                    last.fade_in = MICROFADE.min(last.duration / 2.0);
                    last.fade_out = last.fade_in;
                }
                continue;
            }
            prev_pushed = false;
            let Some(asset) = project.asset(asset_id) else { continue };
            if !asset.has_audio || asset.kind == MediaKind::Image {
                continue;
            }
            let Some(pcm) = cache.get(&asset.path) else { continue };
            let fade = MICROFADE.min(d / 2.0);
            segments.push(AudioSegment {
                pcm,
                comp_start: clip.comp_start,
                duration: d,
                src_start: clip.src_start.max(0.0),
                speed: 1.0,
                gain,
                pan,
                fade_in: fade,
                fade_out: fade,
                volume_keys: Vec::new(),
                looped: false,
            });
            prev_pushed = true;
        }
    }

    // --- layers -------------------------------------------------------------
    for layer in &comp.layers {
        if layer.hidden || layer.duration.is_nan() || layer.duration <= 0.0 || !layer.start.is_finite() {
            continue;
        }
        let (LayerContent::Audio(clip) | LayerContent::Video(clip)) = &layer.content else { continue };
        let Some((gain, pan)) = settings_gain(&clip.audio) else { continue };
        let Some(asset) = project.asset(clip.asset) else { continue };
        if !asset.has_audio {
            continue;
        }
        let Some(pcm) = cache.get(&asset.path) else { continue };
        let half = layer.duration / 2.0;
        let mut keys: Vec<(f64, f64)> = clip.volume_keys.iter().copied().filter(|(t, db)| t.is_finite() && !db.is_nan()).collect();
        keys.sort_by(|a, b| a.0.total_cmp(&b.0));
        segments.push(AudioSegment {
            pcm,
            comp_start: layer.start.max(0.0),
            duration: layer.duration,
            src_start: if clip.src_in.is_finite() { clip.src_in.max(0.0) } else { 0.0 },
            speed: if clip.speed.is_finite() { clip.speed.max(0.05) } else { 1.0 },
            gain,
            pan,
            fade_in: if layer.fade_in > 1e-4 { layer.fade_in.min(half) } else { 0.0 },
            fade_out: if layer.fade_out > 1e-4 { layer.fade_out.min(half) } else { 0.0 },
            volume_keys: keys,
            looped: clip.loop_playback,
        });
    }

    let duration = comp.duration();
    Timeline { segments, duration: if duration.is_finite() { duration.max(0.0) } else { 0.0 } }
}

/// Renders composition frames `[start_frame, start_frame + out.len())` at
/// 48 kHz stereo. The result is the plain sum of all segments (it is not
/// clipped). Realtime-safe.
pub fn render(tl: &Timeline, start_frame: u64, out: &mut [[f32; 2]]) {
    out.fill([0.0, 0.0]);
    if out.is_empty() {
        return;
    }
    for seg in &tl.segments {
        mix_segment(seg, start_frame, out);
    }
}

/// Pages in the source audio needed to render `[start_frame, start_frame + frames)`.
pub fn prefetch(tl: &Timeline, start_frame: u64, frames: u64) {
    let end = start_frame.saturating_add(frames);
    for seg in &tl.segments {
        let s0 = frame_of(seg.comp_start);
        let s1 = frame_of(seg.comp_start + seg.duration);
        let (a, b) = (start_frame.max(s0), end.min(s1));
        if a >= b {
            continue;
        }
        let src0 = frame_of(seg.src_start);
        let speed = sane_speed(seg.speed);
        if seg.looped {
            // The read position wraps; a few seconds from the loop start cover the common case cheaply.
            seg.pcm.prefetch(src0, seg.pcm.frames().saturating_sub(src0).min(((b - a) as f64 * speed) as u64 + 2));
        }
        let from = src0 + ((a - s0) as f64 * speed) as u64;
        seg.pcm.prefetch(from, ((b - a) as f64 * speed) as u64 + 2);
    }
}

fn sane_speed(speed: f64) -> f64 {
    if speed.is_finite() && speed > 0.0 { speed.min(64.0) } else { 1.0 }
}

/// Volume automation evaluated at increasing times without searching from
/// the start for every sample.
struct KeyCursor<'a> {
    keys: &'a [(f64, f64)],
    /// Index of the first key with time > the last evaluated time.
    next: usize,
}

impl<'a> KeyCursor<'a> {
    fn new(keys: &'a [(f64, f64)]) -> Self {
        Self { keys, next: 0 }
    }

    /// Linear gain at `t` seconds from the segment start. `t` must not decrease between calls.
    #[inline]
    fn gain(&mut self, t: f64) -> f32 {
        let k = self.keys;
        while self.next < k.len() && k[self.next].0 <= t {
            self.next += 1;
        }
        let db = if self.next == 0 {
            k[0].1
        } else if self.next == k.len() {
            k[k.len() - 1].1
        } else {
            let (t0, d0) = k[self.next - 1];
            let (t1, d1) = k[self.next];
            let span = t1 - t0;
            if span < 1e-9 { d1 } else { d0 + (d1 - d0) * (t - t0) / span }
        };
        // 10^(dB/20); nonsense keys mute rather than poison the mix.
        let g = (db * (std::f64::consts::LN_10 / 20.0)).exp() as f32;
        if g.is_finite() { g } else { 0.0 }
    }
}

fn mix_segment(seg: &AudioSegment, start_frame: u64, out: &mut [[f32; 2]]) {
    let s0 = frame_of(seg.comp_start);
    let s1 = frame_of(seg.comp_start + seg.duration);
    let total = s1.saturating_sub(s0);
    let end_frame = start_frame + out.len() as u64;
    let (a, b) = (start_frame.max(s0), end_frame.min(s1));
    if a >= b || !seg.gain.is_finite() {
        return;
    }
    let pcm = &*seg.pcm;
    let src_len = pcm.frames();
    let src0 = frame_of(seg.src_start);
    if src0 >= src_len {
        return;
    }
    // Frames available from the segment's start position to the end of the file.
    let avail = src_len - src0;
    let speed = sane_speed(seg.speed);
    let unit_speed = speed == 1.0;

    let pan = if seg.pan.is_finite() { seg.pan.clamp(-1.0, 1.0) } else { 0.0 };
    let gl = seg.gain * if pan > 0.0 { 1.0 - pan } else { 1.0 };
    let gr = seg.gain * if pan < 0.0 { 1.0 + pan } else { 1.0 };

    let total_f = total as f64;
    let fade_in = if seg.fade_in.is_finite() { (seg.fade_in * SR).clamp(0.0, total_f) } else { 0.0 };
    let fade_out = if seg.fade_out.is_finite() { (seg.fade_out * SR).clamp(0.0, total_f) } else { 0.0 };
    let fade_out_from = total_f - fade_out;
    let mut keys = (!seg.volume_keys.is_empty()).then(|| KeyCursor::new(&seg.volume_keys));

    for n in a..b {
        let k = n - s0;
        // Source sample.
        let s = if unit_speed {
            let i = if seg.looped {
                k % avail
            } else if k >= avail {
                // Past the end of the file: silence from here on.
                break;
            } else {
                k
            };
            pcm.frame_f32(src0 + i)
        } else {
            let p = k as f64 * speed;
            let i = p.floor();
            let frac = (p - i) as f32;
            let i = i as u64;
            let (i0, i1) = if seg.looped {
                (i % avail, (i + 1) % avail)
            } else if i >= avail {
                break;
            } else {
                (i, i + 1)
            };
            let x0 = pcm.frame_f32(src0 + i0);
            // `frame_f32` is silent past the end, so the last sample fades to zero.
            let x1 = pcm.frame_f32(src0 + i1);
            [x0[0] + (x1[0] - x0[0]) * frac, x0[1] + (x1[1] - x0[1]) * frac]
        };
        // Envelope.
        let kf = k as f64;
        let mut env = 1.0f32;
        if kf < fade_in {
            env *= (kf / fade_in) as f32;
        }
        if kf >= fade_out_from && fade_out > 0.0 {
            env *= ((total_f - kf) / fade_out) as f32;
        }
        if let Some(keys) = keys.as_mut() {
            env *= keys.gain(kf / SR);
        }
        let o = &mut out[(n - start_frame) as usize];
        o[0] += s[0] * gl * env;
        o[1] += s[1] * gr * env;
    }
}
