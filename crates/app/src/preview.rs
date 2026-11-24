//! Live preview: keeps the playback engine and frame decoders in step with
//! the app model.

use crate::model::AppModel;
use gpui_kit::RenderImage;
use mango_core::{CompositionId, LayerContent, LayerId, Secs};
use mango_playback::audio::{AudioEngine, Engine, NullEngine};
use mango_playback::mix;
use mango_playback::pcm::PcmCache;
use mango_playback::video::{LayerVideo, Picture, ScriptVideo, VideoFrame};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

static HEADLESS: AtomicBool = AtomicBool::new(false);

/// Makes new previews use a silent, manually clocked engine instead of the
/// sound device. For tests and machines without audio output.
pub fn set_headless(on: bool) {
    HEADLESS.store(on, Ordering::Relaxed);
}

enum Clock {
    Device(AudioEngine),
    Null(NullEngine),
}

impl Clock {
    fn engine(&self) -> &dyn Engine {
        match self {
            Clock::Device(e) => e,
            Clock::Null(e) => e,
        }
    }
}

pub const PREVIEW_W: u32 = 960;
pub const PREVIEW_H: u32 = 960;

struct LayerPreview {
    id: LayerId,
    path: PathBuf,
    video: LayerVideo,
    shown: Option<Arc<VideoFrame>>,
}

pub struct Preview {
    pub cache: PcmCache,
    clock: Clock,
    video: ScriptVideo,
    layers: Vec<LayerPreview>,
    /// Set by decoder threads when a new picture is ready.
    pub frame_ready: Arc<AtomicBool>,
    revision: u64,
    comp: Option<CompositionId>,
    /// The playhead value this side last wrote or applied.
    synced_playhead: Secs,
    rate: f32,
    shown: Option<Arc<VideoFrame>>,
    /// Files whose audio has been decoded or is being decoded.
    pub requested: HashSet<PathBuf>,
    /// A message to show once (no audio device, decoder failure).
    pub notice: Option<String>,
}

/// Wraps a decoded frame for drawing.
pub fn to_image(f: &VideoFrame) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(f.width, f.height, f.bgra.clone())?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

impl Default for Preview {
    fn default() -> Self {
        Self::new()
    }
}

impl Preview {
    pub fn new() -> Self {
        let mut notice = None;
        let clock = if HEADLESS.load(Ordering::Relaxed) {
            Clock::Null(NullEngine::new())
        } else {
            match AudioEngine::new() {
                Ok(e) => Clock::Device(e),
                Err(e) => {
                    notice = Some(format!("No sound output is available, so preview is silent: {e:#}"));
                    Clock::Null(NullEngine::new())
                }
            }
        };
        let frame_ready = Arc::new(AtomicBool::new(false));
        let video = ScriptVideo::new(PREVIEW_W, PREVIEW_H);
        let flag = frame_ready.clone();
        video.set_on_frame(Box::new(move || flag.store(true, Ordering::Relaxed)));
        Self {
            cache: PcmCache::new(),
            clock,
            video,
            layers: Vec::new(),
            frame_ready,
            revision: u64::MAX,
            comp: None,
            synced_playhead: 0.0,
            rate: 1.0,
            shown: None,
            requested: HashSet::new(),
            notice,
        }
    }

    /// Forces the timeline to be rebuilt on the next sync (after audio for a
    /// file finishes decoding).
    pub fn invalidate(&mut self) {
        self.revision = u64::MAX;
    }

    /// Files used by the project whose audio has not been requested yet.
    pub fn missing_audio(&mut self, model: &AppModel) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for a in &model.editor.project.assets {
            if a.has_audio && !self.requested.contains(&a.path) {
                self.requested.insert(a.path.clone());
                out.push(a.path.clone());
            }
        }
        out
    }

    pub fn levels(&self) -> (f32, f32) {
        self.clock.engine().levels()
    }

    /// Brings the engine in line with the model, and the model's playhead in
    /// line with the engine's clock while playing. `dt` is the wall time
    /// since the last call. Returns true if anything visible changed.
    pub fn sync(&mut self, model: &mut AppModel, dt: Secs) -> bool {
        let mut changed = false;
        let rebuild = model.editor.revision != self.revision || model.editor.active != self.comp;
        if rebuild {
            self.sync_layer_decoders(model);
        }
        let engine = self.clock.engine();
        if rebuild {
            self.revision = model.editor.revision;
            self.comp = model.editor.active;
            match model.comp() {
                Some(c) => {
                    engine.set_timeline(mix::timeline(&model.editor.project, c, &self.cache));
                    self.video.set_clips(&model.editor.project, c);
                }
                None => engine.set_timeline(mix::Timeline { segments: Vec::new(), duration: 0.0 }),
            }
            changed = true;
        }
        if (model.rate - self.rate).abs() > 1e-6 {
            self.rate = model.rate;
            engine.set_rate(model.rate);
        }
        // The user moved the playhead (click, arrow keys, scrub).
        if (model.playhead - self.synced_playhead).abs() > 1e-6 {
            engine.seek(model.playhead);
            self.synced_playhead = model.playhead;
            changed = true;
        }
        if model.playing != engine.is_playing() {
            if model.playing {
                engine.seek(model.playhead);
                engine.play();
            } else {
                engine.pause();
            }
            changed = true;
        }
        if model.playing {
            if let Clock::Null(n) = &self.clock {
                n.advance(dt);
            }
            let pos = engine.position();
            if engine.finished() {
                model.set_playhead_from_clock(model.duration());
                engine.pause();
                model.playing = false;
            } else {
                model.set_playhead_from_clock(pos);
            }
            self.synced_playhead = model.playhead;
            changed = true;
        }
        if let Clock::Device(e) = &self.clock {
            if let Some(err) = e.take_error() {
                self.notice = Some(format!("Sound output stopped: {err}"));
                model.playing = false;
            }
        }
        if let Some(err) = self.video.take_error() {
            self.notice = Some(format!("Could not show the video: {err}"));
        }
        changed
    }

    fn sync_layer_decoders(&mut self, model: &AppModel) {
        let Some(c) = model.comp() else {
            self.layers.clear();
            return;
        };
        let wanted: Vec<(LayerId, PathBuf)> = c
            .layers
            .iter()
            .filter_map(|l| match &l.content {
                LayerContent::Video(m) => model.editor.project.asset(m.asset).filter(|a| a.has_video).map(|a| (l.id, a.path.clone())),
                _ => None,
            })
            .collect();
        self.layers.retain(|lp| wanted.iter().any(|(id, p)| *id == lp.id && *p == lp.path));
        for (id, path) in wanted {
            if !self.layers.iter().any(|lp| lp.id == id) {
                let video = LayerVideo::new(&path, PREVIEW_W / 2, PREVIEW_H / 2);
                let flag = self.frame_ready.clone();
                video.set_on_frame(Box::new(move || flag.store(true, Ordering::Relaxed)));
                self.layers.push(LayerPreview { id, path, video, shown: None });
            }
        }
    }

    /// The script track's picture at the playhead. `Some(None)` means "show
    /// nothing" (gap clip or audio-only); `None` means "keep what is shown".
    pub fn script_picture(&mut self, model: &AppModel) -> Option<Option<Arc<VideoFrame>>> {
        match self.video.request(model.playhead, model.playing) {
            Picture::Frame(f) => {
                if self.shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, &f)) {
                    return None;
                }
                self.shown = Some(f.clone());
                Some(Some(f))
            }
            Picture::Blank => {
                if self.shown.take().is_some() {
                    Some(None)
                } else {
                    None
                }
            }
            Picture::Pending => None,
        }
    }

    /// New pictures for video layers showing at the playhead.
    pub fn layer_pictures(&mut self, model: &AppModel) -> Vec<(LayerId, Option<Arc<VideoFrame>>)> {
        let Some(c) = model.comp() else { return Vec::new() };
        let t = model.playhead;
        let mut out = Vec::new();
        for lp in &mut self.layers {
            let Some(l) = c.layer(lp.id) else { continue };
            let LayerContent::Video(m) = &l.content else { continue };
            if l.hidden || t < l.start || t >= l.end() {
                if lp.shown.take().is_some() {
                    out.push((lp.id, None));
                }
                continue;
            }
            let mut s = m.src_in + (t - l.start) * m.speed;
            if m.loop_playback {
                if let Some(a) = model.editor.project.asset(m.asset) {
                    let span = (a.duration - m.src_in).max(0.05);
                    s = m.src_in + (s - m.src_in).rem_euclid(span);
                }
            }
            match lp.video.request(s, model.playing) {
                Picture::Frame(f) => {
                    if !lp.shown.as_ref().is_some_and(|x| Arc::ptr_eq(x, &f)) {
                        lp.shown = Some(f.clone());
                        out.push((lp.id, Some(f)));
                    }
                }
                Picture::Blank => {
                    if lp.shown.take().is_some() {
                        out.push((lp.id, None));
                    }
                }
                Picture::Pending => {}
            }
        }
        out
    }
}
