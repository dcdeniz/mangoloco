//! Render planning: turns a composition into ffmpeg invocations.
//!
//! Export runs in two stages.
//!
//! 1. **Chunks.** The script track is cut into chunks (never straddling a
//!    scene, at most [`MAX_CLIPS_PER_CHUNK`] clips each). Each chunk is one
//!    ffmpeg run that seeks every clip, places the video on the canvas and
//!    concatenates, producing a lossless-ish intermediate with exactly the
//!    right number of frames and samples.
//! 2. **Final.** The chunks are joined (with scene transitions), layers and
//!    pre-rasterised overlays are composited on top, audio is mixed and
//!    mastered, and the result is encoded.
//!
//! This module only builds argument lists; running ffmpeg and rasterising
//! text/shape/caption overlays is done by the media crate.

use crate::model::*;
use crate::script::{CONTIGUITY_EPS, ScriptClip};
use std::path::{Path, PathBuf};

pub const MAX_CLIPS_PER_CHUNK: usize = 32;
pub const AUDIO_RATE: u32 = 48_000;
/// Fade applied at every cut to avoid clicks.
pub const MICROFADE: Secs = 0.004;
/// Length of layer in/out animations.
pub const ANIM_SECS: Secs = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VideoCodec {
    H264,
    Hevc,
    ProRes,
    Vp9,
    Gif,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AudioCodec {
    Aac,
    Mp3,
    Wav,
    Flac,
    Opus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Quality {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Normalize {
    Off,
    /// Peak-normalise to this dBFS ceiling (approximated with a limiter-safe gain).
    Peak(f64),
    /// Integrated loudness target in LUFS.
    Lufs(f64),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExportSettings {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// `None` exports audio only.
    pub video: Option<VideoCodec>,
    /// `None` exports video without sound.
    pub audio: Option<AudioCodec>,
    pub quality: Quality,
    /// Use the platform hardware encoder when one exists for the codec.
    pub hardware: bool,
    pub audio_bitrate_kbps: u32,
    pub sample_rate: u32,
    pub channels: u32,
    pub normalize: Normalize,
    /// Composition time range to export; `None` = everything.
    pub range: Option<(Secs, Secs)>,
    pub embed_chapters: bool,
    #[serde(default)]
    pub title: Option<String>,
}

impl ExportSettings {
    pub fn for_composition(comp: &Composition) -> Self {
        Self {
            width: comp.width,
            height: comp.height,
            fps: comp.fps,
            video: Some(VideoCodec::H264),
            audio: Some(AudioCodec::Aac),
            quality: Quality::High,
            hardware: false,
            audio_bitrate_kbps: 192,
            sample_rate: AUDIO_RATE,
            channels: 2,
            normalize: comp.master_loudness.map(Normalize::Lufs).unwrap_or(Normalize::Off),
            range: None,
            embed_chapters: true,
            title: None,
        }
    }

    /// Scales the composition to the given output height, keeping aspect.
    pub fn with_height(mut self, comp: &Composition, height: u32) -> Self {
        let h = height.max(2) & !1;
        let w = ((comp.width as f64 * h as f64 / comp.height as f64).round() as u32).max(2) & !1;
        self.width = w;
        self.height = h;
        self
    }

    /// Conventional file extension for these settings.
    pub fn extension(&self) -> &'static str {
        match (self.video, self.audio) {
            (Some(VideoCodec::Gif), _) => "gif",
            (Some(VideoCodec::Vp9), _) => "webm",
            (Some(VideoCodec::ProRes), _) => "mov",
            (Some(_), _) => "mp4",
            (None, Some(AudioCodec::Mp3)) => "mp3",
            (None, Some(AudioCodec::Wav)) => "wav",
            (None, Some(AudioCodec::Flac)) => "flac",
            (None, Some(AudioCodec::Opus)) => "opus",
            (None, _) => "m4a",
        }
    }
}

/// One stage-1 job.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub index: usize,
    pub scene: usize,
    pub clips: Vec<ScriptClip>,
    /// Composition time of the chunk start and end.
    pub start: Secs,
    pub end: Secs,
    /// Exact video frame count of the chunk at the export frame rate.
    pub frames: u64,
}

impl Chunk {
    pub fn duration(&self) -> Secs {
        self.end - self.start
    }
}

/// A pre-rasterised full-canvas RGBA overlay (text, shape, captions …).
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayInput {
    /// A PNG, or an ffconcat list of PNGs with durations when `sequence`.
    pub path: PathBuf,
    pub sequence: bool,
    pub start: Secs,
    pub end: Secs,
    pub track: i32,
    pub fade_in: Secs,
    pub fade_out: Secs,
    pub animation_in: Animation,
    pub animation_out: Animation,
}

fn f(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" { "0".into() } else { s.to_string() }
}

fn frame_at(t: Secs, fps: f64) -> u64 {
    (t * fps).round().max(0.0) as u64
}

fn db_to_lin(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

fn path_arg(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// ffmpeg audio filters for one effect.
pub fn audio_effect_filters(e: &AudioEffect) -> Vec<String> {
    match *e {
        AudioEffect::SpeechEnhance { intensity } => {
            let i = intensity.clamp(0.0, 1.0);
            vec![
                "highpass=f=80".into(),
                format!("afftdn=nr={}:nf=-28:tn=1", f(6.0 + 22.0 * i)),
                format!("deesser=i={}", f(0.2 + 0.3 * i)),
                format!("acompressor=threshold={}:ratio={}:attack=8:release=160:makeup={}", f(db_to_lin(-20.0)), f(2.0 + 2.0 * i), f(db_to_lin(3.0 * i + 1.0))),
                "equalizer=f=3200:t=q:w=1.2:g=2".into(),
                "alimiter=limit=0.89:level=disabled".into(),
            ]
        }
        AudioEffect::Denoise { strength_db } => vec![format!("afftdn=nr={}:nf=-28:tn=1", f(strength_db.clamp(0.01, 97.0)))],
        AudioEffect::HighPass { hz } => vec![format!("highpass=f={}", f(hz.max(1.0)))],
        AudioEffect::LowPass { hz } => vec![format!("lowpass=f={}", f(hz.max(1.0)))],
        AudioEffect::Eq { hz, gain_db, q } => vec![format!("equalizer=f={}:t=q:w={}:g={}", f(hz.max(1.0)), f(q.max(0.05)), f(gain_db))],
        AudioEffect::LowShelf { hz, gain_db } => vec![format!("lowshelf=f={}:g={}", f(hz.max(1.0)), f(gain_db))],
        AudioEffect::HighShelf { hz, gain_db } => vec![format!("highshelf=f={}:g={}", f(hz.max(1.0)), f(gain_db))],
        AudioEffect::Compressor { threshold_db, ratio, attack_ms, release_ms, makeup_db } => vec![format!(
            "acompressor=threshold={}:ratio={}:attack={}:release={}:makeup={}",
            f(db_to_lin(threshold_db).clamp(0.000976563, 1.0)),
            f(ratio.clamp(1.0, 20.0)),
            f(attack_ms.clamp(0.01, 2000.0)),
            f(release_ms.clamp(0.01, 9000.0)),
            f(db_to_lin(makeup_db).clamp(1.0, 64.0))
        )],
        AudioEffect::Limiter { ceiling_db } => vec![format!("alimiter=limit={}:level=disabled", f(db_to_lin(ceiling_db).clamp(0.0625, 1.0)))],
        AudioEffect::DeEsser { intensity } => vec![format!("deesser=i={}", f(intensity.clamp(0.0, 1.0)))],
        AudioEffect::Gate { threshold_db } => vec![format!("agate=threshold={}", f(db_to_lin(threshold_db).clamp(0.0, 1.0)))],
        AudioEffect::Loudness { target_lufs } => vec![format!("loudnorm=I={}:TP=-1.5:LRA=11", f(target_lufs.clamp(-70.0, -5.0))), format!("aresample={AUDIO_RATE}")],
        AudioEffect::Reverb { mix, room } => {
            let m = mix.clamp(0.0, 1.0);
            let r = room.clamp(0.0, 1.0);
            let d1 = 20.0 + 60.0 * r;
            vec![format!(
                "aecho=1:{}:{}|{}|{}:{}|{}|{}",
                f(0.2 + 0.7 * m),
                f(d1),
                f(d1 * 1.7),
                f(d1 * 2.9),
                f(0.25 + 0.3 * r),
                f(0.18 + 0.25 * r),
                f(0.1 + 0.2 * r)
            )]
        }
        AudioEffect::Distortion { drive } => {
            let d = drive.clamp(0.0, 1.0) * 30.0;
            vec![format!("volume={}dB", f(d)), "alimiter=limit=0.5:level=disabled:attack=0.1:release=1".into(), format!("volume={}dB", f(6.0 - d * 0.3))]
        }
        AudioEffect::Bitcrusher { bits } => vec![format!("acrusher=bits={}:mix=1:mode=lin", bits.clamp(1, 16))],
        AudioEffect::Flanger { depth, rate_hz } => vec![format!("flanger=depth={}:speed={}", f(depth.clamp(0.0, 10.0)), f(rate_hz.clamp(0.1, 10.0)))],
    }
}

/// Gain, pan and effects for an [`AudioSettings`].
pub fn audio_settings_filters(a: &AudioSettings) -> Vec<String> {
    let mut v = Vec::new();
    if a.muted {
        v.push("volume=0".into());
        return v;
    }
    if a.gain_db.abs() > 1e-6 {
        v.push(format!("volume={}dB", f(a.gain_db)));
    }
    for e in &a.effects {
        v.extend(audio_effect_filters(e));
    }
    if a.pan.abs() > 1e-6 {
        let p = a.pan.clamp(-1.0, 1.0);
        let (l, r) = (if p > 0.0 { 1.0 - p } else { 1.0 }, if p < 0.0 { 1.0 + p } else { 1.0 });
        v.push(format!("pan=stereo|c0={}*c0|c1={}*c1", f(l), f(r)));
    }
    v
}

/// ffmpeg video filters for one effect. Input and output are RGBA.
pub fn video_effect_filters(e: &VideoEffect) -> Vec<String> {
    match *e {
        VideoEffect::ChromaKey { color, similarity, blend } => vec![format!(
            "chromakey=color=0x{:02X}{:02X}{:02X}:similarity={}:blend={}",
            color.r,
            color.g,
            color.b,
            f(similarity.clamp(0.01, 1.0)),
            f(blend.clamp(0.0, 1.0))
        )],
        VideoEffect::Blur { radius } => vec![format!("gblur=sigma={}", f(radius.clamp(0.0, 200.0)))],
        VideoEffect::Brightness { value } => vec![format!("eq=brightness={}", f(value.clamp(-1.0, 1.0)))],
        VideoEffect::Contrast { value } => vec![format!("eq=contrast={}", f(value.clamp(0.0, 4.0)))],
        VideoEffect::Saturation { value } => vec![format!("eq=saturation={}", f(value.clamp(0.0, 3.0)))],
        VideoEffect::Exposure { stops } => vec![format!("eq=gamma={}", f(2f64.powf(stops.clamp(-3.0, 3.0) * 0.5).clamp(0.1, 10.0)))],
        VideoEffect::Grayscale => vec!["hue=s=0".into()],
        VideoEffect::Flip => vec!["hflip".into()],
        VideoEffect::FlipVertical => vec!["vflip".into()],
        VideoEffect::Invert => vec!["negate".into()],
        VideoEffect::Vignette => vec!["vignette".into()],
        VideoEffect::Pixelate { size } => {
            let s = size.clamp(2, 256);
            vec![format!("pixelize=w={s}:h={s}")]
        }
        VideoEffect::Grain { amount } => vec![format!("noise=alls={}:allf=t", f((amount.clamp(0.0, 1.0) * 60.0).round()))],
        VideoEffect::Temperature { kelvin } => vec![format!("colortemperature=temperature={}", f(kelvin.clamp(1000.0, 40000.0)))],
        VideoEffect::CornerRadius { fraction } => {
            let fr = fraction.clamp(0.0, 0.5);
            // R = fraction of the shorter side.
            let r = format!("(min(W,H)*{})", f(fr));
            vec![format!(
                "geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(gt(abs(W/2-X),W/2-{r})*gt(abs(H/2-Y),H/2-{r}),if(lte(hypot({r}-(W/2-abs(W/2-X)),{r}-(H/2-abs(H/2-Y))),{r}),alpha(X,Y),0),alpha(X,Y))'"
            )]
        }
    }
}

// ---------------------------------------------------------------------------
// Stage 1: chunks
// ---------------------------------------------------------------------------

/// Splits the script track into chunk jobs covering `settings.range`.
pub fn plan_chunks(comp: &Composition, settings: &ExportSettings) -> Vec<Chunk> {
    let clips = comp.script_clips();
    let fps = settings.fps;
    let mut chunks: Vec<Chunk> = Vec::new();
    for clip in clips {
        if clip.duration() <= 1e-6 {
            continue;
        }
        let new_chunk = match chunks.last() {
            Some(c) => c.scene != clip.scene || c.clips.len() >= MAX_CLIPS_PER_CHUNK,
            None => true,
        };
        if new_chunk {
            chunks.push(Chunk { index: chunks.len(), scene: clip.scene, clips: Vec::new(), start: clip.comp_start, end: clip.comp_start, frames: 0 });
        }
        let c = chunks.last_mut().unwrap();
        c.end = clip.comp_end();
        c.clips.push(clip);
    }
    for c in &mut chunks {
        c.frames = frame_at(c.end, fps) - frame_at(c.start, fps);
    }
    chunks
}

// ---------------------------------------------------------------------------
// Stage 2: final
// ---------------------------------------------------------------------------

/// Filters that crop, scale and style a visual source for a placement box,
/// ending in RGBA. `w`/`h` are the canvas size in pixels.
fn placement_filters(tr: &Transform, crop: &Crop, fit: FitMode, effects: &[VideoEffect], w: u32, h: u32) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let (cl, ct, cr, cb) = (crop.left.clamp(0.0, 0.95), crop.top.clamp(0.0, 0.95), crop.right.clamp(0.0, 0.95), crop.bottom.clamp(0.0, 0.95));
    if cl + ct + cr + cb > 1e-6 {
        let kw = (1.0 - cl - cr).max(0.02);
        let kh = (1.0 - ct - cb).max(0.02);
        v.push(format!("crop=iw*{}:ih*{}:iw*{}:ih*{}", f(kw), f(kh), f(cl), f(ct)));
    }
    let bw = ((tr.w.max(0.005) * w as f64).round() as i64).max(2);
    let bh = ((tr.h.max(0.005) * h as f64).round() as i64).max(2);
    match fit {
        FitMode::Fit => v.push(format!("scale={bw}:{bh}:force_original_aspect_ratio=decrease:flags=bicubic")),
        FitMode::Fill => {
            v.push(format!("scale={bw}:{bh}:force_original_aspect_ratio=increase:flags=bicubic"));
            v.push(format!("crop={bw}:{bh}"));
        }
        FitMode::Stretch => v.push(format!("scale={bw}:{bh}:flags=bicubic")),
    }
    v.push("setsar=1".into());
    v.push("format=rgba".into());
    for e in effects {
        v.extend(video_effect_filters(e));
    }
    if effects.iter().any(|e| !matches!(e, VideoEffect::ChromaKey { .. } | VideoEffect::CornerRadius { .. })) {
        v.push("format=rgba".into());
    }
    if tr.rotation_deg.abs() > 1e-6 {
        let a = tr.rotation_deg.to_radians();
        v.push(format!("rotate={}:c=none:ow=rotw({}):oh=roth({})", f(a), f(a), f(a)));
    }
    if tr.opacity < 1.0 - 1e-6 {
        v.push(format!("colorchannelmixer=aa={}", f(tr.opacity.clamp(0.0, 1.0))));
    }
    v
}

/// Overlay position expressions that centre the overlay at the transform's
/// centre.
fn overlay_xy(tr: &Transform, w: u32, h: u32) -> (String, String) {
    (format!("{}-w/2", f(tr.x * w as f64)), format!("{}-h/2", f(tr.y * h as f64)))
}

/// Adds slide-in/out offsets (time-dependent) to overlay position expressions.
fn animated_xy(x: String, y: String, start: Secs, end: Secs, a_in: Animation, a_out: Animation, w: u32, h: u32) -> (String, String) {
    let d = ANIM_SECS.min((end - start) / 2.0).max(0.001);
    // Progress 0→1 over the in animation, eased (cubic out).
    let pin = format!("(1-pow(1-clip((t-{})/{},0,1),3))", f(start), f(d));
    let pout = format!("pow(clip((t-{})/{},0,1),3)", f(end - d), f(d));
    let (mut xs, mut ys) = (x, y);
    let (wf, hf) = (w as f64, h as f64);
    match a_in {
        Animation::SlideLeft => xs = format!("{xs}+{}*(1-{pin})", f(wf)),
        Animation::SlideRight => xs = format!("{xs}-{}*(1-{pin})", f(wf)),
        Animation::SlideUp => ys = format!("{ys}+{}*(1-{pin})", f(hf)),
        Animation::SlideDown => ys = format!("{ys}-{}*(1-{pin})", f(hf)),
        _ => {}
    }
    match a_out {
        Animation::SlideLeft => xs = format!("{xs}-{}*{pout}", f(wf)),
        Animation::SlideRight => xs = format!("{xs}+{}*{pout}", f(wf)),
        Animation::SlideUp => ys = format!("{ys}-{}*{pout}", f(hf)),
        Animation::SlideDown => ys = format!("{ys}+{}*{pout}", f(hf)),
        _ => {}
    }
    (xs, ys)
}

/// Alpha fades for a layer whose stream timestamps are in composition time.
fn alpha_fades(start: Secs, end: Secs, fade_in: Secs, fade_out: Secs, a_in: Animation, a_out: Animation) -> Vec<String> {
    let mut v = Vec::new();
    let half = (end - start) / 2.0;
    let fi = if fade_in > 0.0 { fade_in } else if matches!(a_in, Animation::Fade | Animation::ZoomIn | Animation::ZoomOut) { ANIM_SECS } else { 0.0 }.min(half);
    let fo = if fade_out > 0.0 { fade_out } else if matches!(a_out, Animation::Fade | Animation::ZoomIn | Animation::ZoomOut) { ANIM_SECS } else { 0.0 }.min(half);
    if fi > 1e-4 {
        v.push(format!("fade=t=in:st={}:d={}:alpha=1", f(start), f(fi)));
    }
    if fo > 1e-4 {
        v.push(format!("fade=t=out:st={}:d={}:alpha=1", f(end - fo), f(fo)));
    }
    v
}

/// `atempo` chain for an arbitrary speed.
fn atempo_chain(speed: f64) -> Vec<String> {
    let mut s = speed.clamp(0.05, 64.0);
    let mut v = Vec::new();
    while s < 0.5 {
        v.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    while s > 2.0 {
        v.push("atempo=2.0".to_string());
        s /= 2.0;
    }
    if (s - 1.0).abs() > 1e-6 {
        v.push(format!("atempo={}", f(s)));
    }
    v
}

/// Piecewise-linear dB automation as a linear-gain `volume` expression of `t`.
fn volume_keys_expr(keys: &[(Secs, f64)]) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let mut k = keys.to_vec();
    k.sort_by(|a, b| a.0.total_cmp(&b.0));
    // dB(t): hold before first / after last, interpolate between.
    let mut expr = f(k.last().unwrap().1);
    for i in (0..k.len() - 1).rev() {
        let (t0, d0) = k[i];
        let (t1, d1) = k[i + 1];
        let seg = if (t1 - t0).abs() < 1e-9 { f(d1) } else { format!("({}+({})*(t-{})/{})", f(d0), f(d1 - d0), f(t0), f(t1 - t0)) };
        expr = format!("if(lt(t,{}),{},{})", f(t1), seg, expr);
    }
    expr = format!("if(lt(t,{}),{},{})", f(k[0].0), f(k[0].1), expr);
    Some(format!("volume='pow(10,({expr})/20)':eval=frame"))
}

/// Puts a mono source in both channels at full level. ffmpeg's automatic
/// mono-to-stereo conversion would lower each channel by 3 dB.
const MONO_TO_STEREO: &str = "pan=stereo|c0=c0|c1=c0,";

/// How much earlier than a clip's start its input is opened, so the frame on
/// screen at the cut is decoded: at least two source frames.
fn preroll(asset: &MediaAsset, src_start: Secs) -> Secs {
    let frame = if asset.fps > 0.5 { 1.0 / asset.fps } else { 0.1 };
    (2.0 * frame).clamp(0.1, 2.5).min(src_start.max(0.0))
}

/// Arguments (after the program name) that render one chunk to `out`.
pub fn chunk_args(project: &Project, comp: &Composition, chunk: &Chunk, settings: &ExportSettings, out: &Path) -> Vec<String> {
    let (w, h, fps) = (settings.width, settings.height, settings.fps);
    let want_video = settings.video.is_some();
    let placement = comp.scenes.get(chunk.scene).and_then(|s| s.script_video.as_ref()).unwrap_or(&comp.script_video);
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into(), "-nostdin".into()];
    let mut graph: Vec<String> = Vec::new();
    let mut vlabels: Vec<String> = Vec::new();
    let mut alabels: Vec<String> = Vec::new();
    let mut input = 0usize;
    // Every clip of the composition, to see what each chunk clip abuts.
    let all = comp.script_clips();
    let continues = |a: &ScriptClip, b: &ScriptClip| a.asset.is_some() && a.asset == b.asset && (b.src_start - a.src_end).abs() <= CONTIGUITY_EPS;

    for (k, clip) in chunk.clips.iter().enumerate() {
        let d = clip.duration();
        let frames = frame_at(clip.comp_end(), fps) - frame_at(clip.comp_start, fps);
        let asset = clip.asset.and_then(|id| project.asset(id));
        let mut have_v = false;
        let mut have_a = false;
        let mut idx = None;
        // A clip boundary is only a cut if the source jumps there. Clips are
        // also split at scene breaks and chunk limits, where the source runs
        // straight on and must not be faded.
        let pos = all.iter().position(|c| c.tokens == clip.tokens);
        let joined_before = pos.is_some_and(|p| p > 0 && continues(&all[p - 1], &all[p]));
        let joined_after = pos.is_some_and(|p| p + 1 < all.len() && continues(&all[p], &all[p + 1]));
        // Seconds between the input's time zero and the clip's first moment.
        let mut lead = 0.0;
        if let Some(a) = asset {
            let is_image = a.kind == MediaKind::Image;
            if is_image {
                args.extend(["-loop".into(), "1".into(), "-framerate".into(), f(fps), "-t".into(), f(d + 1.0)]);
            } else {
                lead = preroll(a, clip.src_start);
                args.extend(["-ss".into(), f((clip.src_start - lead).max(0.0)), "-t".into(), f(lead + d + 0.5)]);
            }
            args.extend(["-i".into(), path_arg(&a.path)]);
            idx = Some(input);
            input += 1;
            have_v = a.has_video && !placement.hidden;
            have_a = a.has_audio && !is_image;
        }
        if want_video && frames > 0 {
            let bg = format!("color=c={}:s={w}x{h}:r={}:d={}", comp.background.ffmpeg(), f(fps), f(d + 1.0));
            if let (true, Some(i)) = (have_v, idx) {
                // Shift so the clip starts at 0, then conform: output frame n
                // shows the latest source frame at or before n (to rounding),
                // and frame 0 is filled even if the stream starts late.
                let mut chain = vec![format!("setpts=PTS-{}/TB", f(lead)), format!("fps={}:start_time=0", f(fps))];
                chain.extend(placement_filters(&placement.transform, &placement.crop, placement.fit, &placement.effects, w, h));
                let (x, y) = overlay_xy(&placement.transform, w, h);
                graph.push(format!("[{i}:v]{}[fg{k}]", chain.join(",")));
                graph.push(format!("{bg}[bg{k}]"));
                graph.push(format!(
                    "[bg{k}][fg{k}]overlay=x={x}:y={y}:eof_action=repeat,trim=end_frame={frames},setpts=PTS-STARTPTS,format=yuv420p[v{k}]"
                ));
            } else {
                graph.push(format!("{bg},trim=end_frame={frames},setpts=PTS-STARTPTS,format=yuv420p[v{k}]"));
            }
            vlabels.push(format!("[v{k}]"));
        }
        // Audio: exactly `d` seconds, with a microfade at each real cut.
        let fade = MICROFADE.min(d / 2.0);
        let mut tail = format!("aformat=sample_fmts=fltp:channel_layouts=stereo,apad,atrim=duration={},asetpts=PTS-STARTPTS", f(d));
        if !joined_before {
            tail.push_str(&format!(",afade=t=in:d={}:curve=tri", f(fade)));
        }
        if !joined_after {
            tail.push_str(&format!(",afade=t=out:st={}:d={}:curve=tri", f(d - fade), f(fade)));
        }
        tail.push_str(&format!("[a{k}]"));
        if let (true, Some(i)) = (have_a, idx) {
            // Keep the audio where its timestamps put it (silence before a
            // late start), then cut at the clip's first moment.
            let mono = asset.is_some_and(|a| a.channels == 1);
            graph.push(format!("[{i}:a]aresample={AUDIO_RATE}:async=1:first_pts=0,atrim=start={},asetpts=PTS-STARTPTS,{}{tail}", f(lead), if mono { MONO_TO_STEREO } else { "" }));
        } else {
            graph.push(format!("anullsrc=r={AUDIO_RATE}:cl=stereo,{tail}"));
        }
        alabels.push(format!("[a{k}]"));
    }

    if want_video && !vlabels.is_empty() {
        graph.push(format!("{}concat=n={}:v=1:a=0[vout]", vlabels.concat(), vlabels.len()));
    }
    graph.push(format!("{}concat=n={}:v=0:a=1[aout]", alabels.concat(), alabels.len()));
    args.extend(["-filter_complex".into(), graph.join(";")]);
    if want_video && !vlabels.is_empty() {
        args.extend(["-map".into(), "[vout]".into()]);
        args.extend(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "12", "-pix_fmt", "yuv420p", "-r"].map(String::from));
        args.push(f(fps));
        args.extend(["-frames:v".into(), chunk.frames.to_string()]);
    }
    args.extend(["-map".into(), "[aout]".into(), "-c:a".into(), "pcm_s24le".into(), "-ar".into(), AUDIO_RATE.to_string()]);
    args.extend(["-f".into(), "matroska".into(), path_arg(out)]);
    args
}

/// Everything the final stage needs besides the composition.
pub struct FinalInputs<'a> {
    pub chunks: &'a [(Chunk, PathBuf)],
    pub overlays: &'a [OverlayInput],
    /// FFMETADATA file with chapters, if any.
    pub metadata: Option<&'a Path>,
    pub out: &'a Path,
    /// Gain applied to the finished mix, in dB (peak normalisation).
    pub master_gain_db: Option<f64>,
}

/// Half-open visibility window `[start, end)` as an ffmpeg expression. The
/// small bias keeps frames that land exactly on a boundary on the right side
/// despite floating-point timestamps.
fn enable_expr(start: Secs, end: Secs) -> String {
    format!("gte(t,{})*lt(t,{})", f(start - 1e-4), f(end - 1e-4))
}

fn video_encoder_args(s: &ExportSettings) -> Vec<String> {
    let v = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let codec = s.video.unwrap_or(VideoCodec::H264);
    // Rough bitrate targets for hardware encoders, scaled by pixel count.
    let px = (s.width as f64 * s.height as f64) / (1920.0 * 1080.0);
    let hw_rate = |base_mbps: f64| format!("{}k", ((base_mbps * px.max(0.15) * 1000.0).round() as u64).max(300));
    match (codec, s.hardware) {
        (VideoCodec::H264, false) => {
            let crf = match s.quality { Quality::Low => "28", Quality::Medium => "23", Quality::High => "18" };
            v(&["-c:v", "libx264", "-preset", "medium", "-crf", crf, "-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        }
        (VideoCodec::H264, true) => {
            let rate = hw_rate(match s.quality { Quality::Low => 3.0, Quality::Medium => 6.0, Quality::High => 12.0 });
            let mut a = v(&["-c:v", "h264_videotoolbox", "-b:v"]);
            a.push(rate);
            a.extend(v(&["-pix_fmt", "yuv420p", "-movflags", "+faststart"]));
            a
        }
        (VideoCodec::Hevc, false) => {
            let crf = match s.quality { Quality::Low => "30", Quality::Medium => "26", Quality::High => "21" };
            v(&["-c:v", "libx265", "-preset", "medium", "-crf", crf, "-pix_fmt", "yuv420p", "-tag:v", "hvc1", "-movflags", "+faststart"])
        }
        (VideoCodec::Hevc, true) => {
            let rate = hw_rate(match s.quality { Quality::Low => 2.0, Quality::Medium => 4.0, Quality::High => 8.0 });
            let mut a = v(&["-c:v", "hevc_videotoolbox", "-b:v"]);
            a.push(rate);
            a.extend(v(&["-pix_fmt", "yuv420p", "-tag:v", "hvc1", "-movflags", "+faststart"]));
            a
        }
        (VideoCodec::ProRes, _) => {
            let profile = match s.quality { Quality::Low => "1", Quality::Medium => "2", Quality::High => "3" };
            v(&["-c:v", "prores_ks", "-profile:v", profile, "-pix_fmt", "yuv422p10le"])
        }
        (VideoCodec::Vp9, _) => {
            let crf = match s.quality { Quality::Low => "40", Quality::Medium => "34", Quality::High => "28" };
            v(&["-c:v", "libvpx-vp9", "-crf", crf, "-b:v", "0", "-row-mt", "1", "-pix_fmt", "yuv420p"])
        }
        (VideoCodec::Gif, _) => v(&["-c:v", "gif", "-loop", "0"]),
    }
}

fn audio_encoder_args(s: &ExportSettings) -> Vec<String> {
    let mut codec = s.audio.unwrap_or(AudioCodec::Aac);
    // Containers that cannot hold the chosen codec fall back to one they can.
    match s.video {
        Some(VideoCodec::Vp9) => codec = AudioCodec::Opus,
        Some(VideoCodec::H264 | VideoCodec::Hevc) if matches!(codec, AudioCodec::Wav | AudioCodec::Flac | AudioCodec::Opus) => codec = AudioCodec::Aac,
        Some(VideoCodec::ProRes) if matches!(codec, AudioCodec::Mp3 | AudioCodec::Flac | AudioCodec::Opus) => codec = AudioCodec::Wav,
        _ => {}
    }
    let br = format!("{}k", s.audio_bitrate_kbps.clamp(32, 512));
    let mut a: Vec<String> = match codec {
        AudioCodec::Aac => vec!["-c:a".into(), "aac".into(), "-b:a".into(), br],
        AudioCodec::Mp3 => vec!["-c:a".into(), "libmp3lame".into(), "-b:a".into(), br],
        AudioCodec::Opus => vec!["-c:a".into(), "libopus".into(), "-b:a".into(), br],
        AudioCodec::Wav => vec!["-c:a".into(), "pcm_s16le".into()],
        AudioCodec::Flac => vec!["-c:a".into(), "flac".into()],
    };
    let rate = if codec == AudioCodec::Opus { 48_000 } else { s.sample_rate.clamp(8_000, 96_000) };
    a.extend(["-ar".into(), rate.to_string(), "-ac".into(), s.channels.clamp(1, 2).to_string()]);
    a
}

/// Arguments that composite the chunks, layers and overlays into the final
/// file.
pub fn final_args(project: &Project, comp: &Composition, settings: &ExportSettings, inputs: &FinalInputs) -> Vec<String> {
    let (w, h, fps) = (settings.width, settings.height, settings.fps);
    let want_video = settings.video.is_some();
    let want_audio = settings.audio.is_some() && settings.video != Some(VideoCodec::Gif);
    let total = comp.duration().max(inputs.chunks.last().map(|c| c.0.end).unwrap_or(0.0));
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into(), "-nostdin".into()];
    let mut graph: Vec<String> = Vec::new();
    let mut n_in = 0usize;

    // --- chunk inputs -------------------------------------------------------
    let chunk_has_video = want_video;
    for (_, p) in inputs.chunks {
        args.extend(["-i".into(), path_arg(p)]);
        n_in += 1;
    }

    // --- base video: join chunks, with transitions between scenes -----------
    let mut vcur: Option<String> = None;
    if want_video {
        if inputs.chunks.iter().all(|c| c.0.frames == 0) {
            graph.push(format!("color=c={}:s={w}x{h}:r={}:d={},format=yuv420p[base0]", comp.background.ffmpeg(), f(fps), f(total.max(0.1))));
            vcur = Some("[base0]".into());
        } else {
            // Group chunk indices by scene.
            let mut scenes: Vec<(usize, Vec<usize>)> = Vec::new();
            for (i, (c, _)) in inputs.chunks.iter().enumerate() {
                // A chunk shorter than half a frame has no video stream.
                if c.frames == 0 {
                    continue;
                }
                match scenes.last_mut() {
                    Some((s, v)) if *s == c.scene => v.push(i),
                    _ => scenes.push((c.scene, vec![i])),
                }
            }
            let mut acc_dur = 0.0;
            for (si, (scene, idxs)) in scenes.iter().enumerate() {
                let label = format!("[sc{si}]");
                let dur: Secs = idxs.iter().map(|&i| inputs.chunks[i].0.frames as f64 / fps).sum();
                let srcs: String = idxs.iter().map(|i| format!("[{i}:v]")).collect();
                let joined = if idxs.len() == 1 { format!("{srcs}setpts=PTS-STARTPTS") } else { format!("{srcs}concat=n={}:v=1:a=0", idxs.len()) };
                let st = comp.scenes.get(*scene);
                let tr = st.map(|s| s.transition).unwrap_or(Transition::Cut);
                let td = st.map(|s| s.transition_secs).unwrap_or(0.5).min(dur * 0.9).min(acc_dur * 0.9);
                // Transitions last a whole number of frames.
                let td_frames = (td * fps).round().max(0.0) as u64;
                let td = td_frames as f64 / fps;
                match (&vcur, tr.xfade_name()) {
                    (Some(prev), Some(name)) if td_frames >= 1 => {
                        // Hold the incoming scene's first frame for `td` so the
                        // transition does not shorten the timeline.
                        graph.push(format!("{joined},fps={},tpad=start_mode=clone:start={td_frames},format=yuv420p,settb=AVTB{label}", f(fps)));
                        let out = format!("[x{si}]");
                        graph.push(format!(
                            "{prev}fps={},format=yuv420p,settb=AVTB[xp{si}];[xp{si}]{label}xfade=transition={name}:duration={}:offset={}{out}",
                            f(fps),
                            f(td),
                            f((acc_dur - td).max(0.0))
                        ));
                        vcur = Some(out);
                    }
                    (Some(prev), _) => {
                        graph.push(format!("{joined},fps={},format=yuv420p,settb=AVTB{label}", f(fps)));
                        let out = format!("[x{si}]");
                        graph.push(format!("{prev}fps={},format=yuv420p,settb=AVTB[xp{si}];[xp{si}]{label}concat=n=2:v=1:a=0{out}", f(fps)));
                        vcur = Some(out);
                    }
                    (None, _) => {
                        graph.push(format!("{joined},fps={},format=yuv420p,settb=AVTB{label}", f(fps)));
                        vcur = Some(label);
                    }
                }
                acc_dur += dur;
            }
            // Extend with background if layers run past the script.
            if total > acc_dur + 0.5 / fps {
                let prev = vcur.take().unwrap();
                graph.push(format!(
                    "color=c={}:s={w}x{h}:r={}:d={},format=yuv420p,settb=AVTB[tailbg];{prev}fps={},settb=AVTB[tailpre];[tailpre][tailbg]concat=n=2:v=1:a=0[based]",
                    comp.background.ffmpeg(),
                    f(fps),
                    f(total - acc_dur),
                    f(fps)
                ));
                vcur = Some("[based]".into());
            }
        }
    }

    // --- script audio -------------------------------------------------------
    let mut mix: Vec<String> = Vec::new();
    let duckers: Vec<&Layer> = comp
        .layers
        .iter()
        .filter(|l| !l.hidden && matches!(&l.content, LayerContent::Audio(m) | LayerContent::Video(m) if m.duck.is_some() && !m.audio.muted))
        .collect();
    let mut sidechains: Vec<String> = Vec::new();
    if want_audio {
        if inputs.chunks.is_empty() {
            graph.push(format!("anullsrc=r={AUDIO_RATE}:cl=stereo,atrim=duration={}[sa0]", f(total.max(0.1))));
        } else {
            let srcs: String = (0..inputs.chunks.len()).map(|i| format!("[{i}:a]")).collect();
            graph.push(format!("{srcs}concat=n={}:v=0:a=1[sa0]", inputs.chunks.len()));
        }
        let mut chain = audio_settings_filters(&comp.script_audio);
        chain.push(format!("aresample={AUDIO_RATE}"));
        chain.push("aformat=sample_fmts=fltp:channel_layouts=stereo".into());
        if duckers.is_empty() {
            graph.push(format!("[sa0]{}[sa]", chain.join(",")));
        } else {
            let outs: String = (0..duckers.len()).map(|i| format!("[side{i}]")).collect();
            graph.push(format!("[sa0]{},asplit={}[sa]{outs}", chain.join(","), duckers.len() + 1));
            sidechains = (0..duckers.len()).map(|i| format!("[side{i}]")).collect();
        }
        mix.push("[sa]".into());
    }

    // --- layers -------------------------------------------------------------
    enum Vis<'a> {
        Layer(&'a Layer),
        Overlay(&'a OverlayInput),
    }
    let mut visuals: Vec<(i32, usize, Vis)> = Vec::new();
    for (i, l) in comp.layers.iter().enumerate() {
        if l.hidden || l.duration <= 0.0 {
            continue;
        }
        if matches!(l.content, LayerContent::Video(_) | LayerContent::Image(_)) {
            visuals.push((l.track, i, Vis::Layer(l)));
        }
    }
    for (i, o) in inputs.overlays.iter().enumerate() {
        visuals.push((o.track, 10_000 + i, Vis::Overlay(o)));
    }
    visuals.sort_by_key(|v| (v.0, v.1));

    // Inputs for media layers (one per layer, so each can seek/loop on its own).
    let mut layer_input: std::collections::HashMap<LayerId, usize> = std::collections::HashMap::new();
    for l in &comp.layers {
        if l.hidden || l.duration <= 0.0 {
            continue;
        }
        match &l.content {
            LayerContent::Video(m) | LayerContent::Audio(m) => {
                let Some(a) = project.asset(m.asset) else { continue };
                let is_video = matches!(l.content, LayerContent::Video(_));
                if (is_video && !want_video && !want_audio) || (!is_video && !want_audio) {
                    continue;
                }
                if m.loop_playback {
                    args.extend(["-stream_loop".into(), "-1".into()]);
                }
                args.extend(["-ss".into(), f(m.src_in.max(0.0)), "-t".into(), f(l.duration * m.speed.max(0.05) + 0.5), "-i".into(), path_arg(&a.path)]);
                layer_input.insert(l.id, n_in);
                n_in += 1;
            }
            LayerContent::Image(im) => {
                if !want_video {
                    continue;
                }
                let Some(a) = project.asset(im.asset) else { continue };
                args.extend(["-loop".into(), "1".into(), "-framerate".into(), f(fps), "-t".into(), f(l.duration + 0.5), "-i".into(), path_arg(&a.path)]);
                layer_input.insert(l.id, n_in);
                n_in += 1;
            }
            _ => {}
        }
    }
    let mut overlay_input: Vec<usize> = Vec::new();
    for o in inputs.overlays {
        if !want_video {
            overlay_input.push(usize::MAX);
            continue;
        }
        if o.sequence {
            args.extend(["-f".into(), "concat".into(), "-safe".into(), "0".into(), "-i".into(), path_arg(&o.path)]);
        } else {
            args.extend(["-loop".into(), "1".into(), "-framerate".into(), f(fps), "-t".into(), f((o.end - o.start).max(0.05) + 0.5), "-i".into(), path_arg(&o.path)]);
        }
        overlay_input.push(n_in);
        n_in += 1;
    }

    if want_video {
        for (n, (_, key, vis)) in visuals.iter().enumerate() {
            let prev = vcur.clone().unwrap();
            let out = format!("[ov{n}]");
            match vis {
                Vis::Layer(l) => {
                    let Some(&i) = layer_input.get(&l.id) else { continue };
                    let (start, end) = (l.start, l.end());
                    let mut chain: Vec<String> = Vec::new();
                    let (crop, fit, effects, speed, zoom_to) = match &l.content {
                        LayerContent::Video(m) => (&m.crop, m.fit, &m.effects, m.speed.max(0.05), 1.0),
                        LayerContent::Image(im) => (&im.crop, im.fit, &im.effects, 1.0, im.zoom_to),
                        _ => unreachable!(),
                    };
                    let asset_has_video = match &l.content {
                        LayerContent::Video(m) => project.asset(m.asset).is_some_and(|a| a.has_video),
                        _ => true,
                    };
                    if !asset_has_video {
                        continue;
                    }
                    chain.push(format!("fps={}", f(fps)));
                    chain.push(format!("trim=duration={}", f(l.duration * speed)));
                    chain.push(format!("setpts=(PTS-STARTPTS)/{}+{}/TB", f(speed), f(start)));
                    chain.extend(placement_filters(&l.transform, crop, fit, effects, w, h));
                    // Zoom animations scale per frame around the centre.
                    let zoom_expr = |from: f64, to: f64| {
                        format!("scale=w='iw*({}+({})*clip((t-{})/{},0,1))':h=-1:eval=frame:flags=bilinear", f(from), f(to - from), f(start), f(l.duration.max(0.01)))
                    };
                    if (zoom_to - 1.0).abs() > 1e-6 {
                        chain.push(zoom_expr(1.0, zoom_to));
                    }
                    chain.extend(alpha_fades(start, end, l.fade_in, l.fade_out, l.animation_in, l.animation_out));
                    let (x, y) = overlay_xy(&l.transform, w, h);
                    let (x, y) = animated_xy(x, y, start, end, l.animation_in, l.animation_out, w, h);
                    graph.push(format!("[{i}:v]{}[lv{n}]", chain.join(",")));
                    graph.push(format!("{prev}[lv{n}]overlay=x='{x}':y='{y}':eval=frame:eof_action=pass:enable='{}'{out}", enable_expr(start, end)));
                    let _ = key;
                }
                Vis::Overlay(o) => {
                    let i = overlay_input[*key - 10_000];
                    if i == usize::MAX {
                        continue;
                    }
                    let mut chain: Vec<String> = vec![format!("fps={}", f(fps)), format!("scale={w}:{h}:flags=bilinear"), "format=rgba".into()];
                    chain.push(format!("setpts=PTS-STARTPTS+{}/TB", f(o.start)));
                    chain.extend(alpha_fades(o.start, o.end, o.fade_in, o.fade_out, o.animation_in, o.animation_out));
                    let (x, y) = animated_xy("0".into(), "0".into(), o.start, o.end, o.animation_in, o.animation_out, w, h);
                    graph.push(format!("[{i}:v]{}[lv{n}]", chain.join(",")));
                    graph.push(format!("{prev}[lv{n}]overlay=x='{x}':y='{y}':eval=frame:eof_action=pass:enable='{}'{out}", enable_expr(o.start, o.end)));
                }
            }
            vcur = Some(out);
        }
    }

    // --- layer audio --------------------------------------------------------
    if want_audio {
        let mut duck_i = 0usize;
        for (n, l) in comp.layers.iter().enumerate() {
            let (LayerContent::Audio(m) | LayerContent::Video(m)) = &l.content else { continue };
            let Some(&i) = layer_input.get(&l.id) else { continue };
            let Some(a) = project.asset(m.asset) else { continue };
            let ducks = m.duck.is_some() && !m.audio.muted && !l.hidden;
            if !a.has_audio || m.audio.muted {
                if ducks {
                    duck_i += 1;
                }
                continue;
            }
            let speed = m.speed.max(0.05);
            let mut chain: Vec<String> = vec![format!("atrim=duration={}", f(l.duration * speed)), "asetpts=PTS-STARTPTS".into()];
            chain.extend(atempo_chain(speed));
            chain.push(format!("aresample={AUDIO_RATE}"));
            if a.channels == 1 {
                chain.push(MONO_TO_STEREO.trim_end_matches(',').to_string());
            }
            chain.push("aformat=sample_fmts=fltp:channel_layouts=stereo".into());
            chain.extend(audio_settings_filters(&m.audio));
            if let Some(v) = volume_keys_expr(&m.volume_keys) {
                chain.push(v);
            }
            let half = l.duration / 2.0;
            if l.fade_in > 1e-4 {
                chain.push(format!("afade=t=in:d={}", f(l.fade_in.min(half))));
            }
            if l.fade_out > 1e-4 {
                let fo = l.fade_out.min(half);
                chain.push(format!("afade=t=out:st={}:d={}", f(l.duration - fo), f(fo)));
            }
            let delay_ms = (l.start.max(0.0) * 1000.0).round() as u64;
            if delay_ms > 0 {
                chain.push(format!("adelay={delay_ms}:all=1"));
            }
            let label = format!("[la{n}]");
            if let (true, Some(d)) = (ducks, m.duck) {
                let side = sidechains.get(duck_i).cloned();
                duck_i += 1;
                if let Some(side) = side {
                    // Ratio chosen so speech at a typical level pulls the bed
                    // down by roughly `amount_db`.
                    let ratio = (1.0 + d.amount_db.clamp(0.0, 40.0) / 3.0).clamp(1.0, 20.0);
                    graph.push(format!("[{i}:a]{}[lpre{n}]", chain.join(",")));
                    graph.push(format!(
                        "[lpre{n}]{side}sidechaincompress=threshold=0.02:ratio={}:attack={}:release={}:makeup=1{label}",
                        f(ratio),
                        f(d.attack_ms.clamp(0.01, 2000.0)),
                        f(d.release_ms.clamp(0.01, 9000.0))
                    ));
                    mix.push(label);
                    continue;
                }
            }
            graph.push(format!("[{i}:a]{}{label}", chain.join(",")));
            mix.push(label);
        }
        // Unused sidechain taps must still be consumed.
        for s in sidechains.iter().skip(duck_i) {
            graph.push(format!("{s}anullsink"));
        }
        let mixed = if mix.len() == 1 {
            mix[0].clone()
        } else {
            graph.push(format!("{}amix=inputs={}:normalize=0:duration=longest:dropout_transition=0[mixed]", mix.concat(), mix.len()));
            "[mixed]".into()
        };
        let mut master: Vec<String> = Vec::new();
        match settings.normalize {
            Normalize::Off => {}
            Normalize::Lufs(l) => {
                master.push(format!("loudnorm=I={}:TP=-1.5:LRA=11", f(l.clamp(-70.0, -5.0))));
                master.push(format!("aresample={AUDIO_RATE}"));
            }
            Normalize::Peak(db) => {
                // The gain comes from a measuring pass; the limiter only
                // catches rounding and inter-sample overshoot.
                if let Some(g) = inputs.master_gain_db {
                    if g.abs() > 1e-3 {
                        master.push(format!("volume={}dB", f(g)));
                    }
                }
                master.push(format!("alimiter=limit={}:level=disabled", f(db_to_lin(db).clamp(0.0625, 1.0))));
            }
        }
        master.push("alimiter=limit=0.98:level=disabled".into());
        master.push("apad".into());
        master.push(format!("atrim=duration={}", f(total.max(0.05))));
        graph.push(format!("{mixed}{}[aout]", master.join(",")));
    }

    // --- output -------------------------------------------------------------
    let is_gif = settings.video == Some(VideoCodec::Gif);
    if want_video {
        let prev = vcur.clone().unwrap();
        let end_frames = frame_at(total, fps).max(1);
        if is_gif {
            graph.push(format!(
                "{prev}trim=end_frame={end_frames},fps={},split[g0][g1];[g0]palettegen=stats_mode=full[pal];[g1][pal]paletteuse=dither=bayer:bayer_scale=4[vout]",
                f(fps.min(20.0))
            ));
        } else {
            graph.push(format!("{prev}trim=end_frame={end_frames},setpts=PTS-STARTPTS,format=yuv420p[vout]"));
        }
    }
    if let Some(m) = inputs.metadata {
        args.extend(["-f".into(), "ffmetadata".into(), "-i".into(), path_arg(m)]);
    }
    args.extend(["-filter_complex".into(), graph.join(";")]);
    if want_video {
        args.extend(["-map".into(), "[vout]".into()]);
    }
    if want_audio {
        args.extend(["-map".into(), "[aout]".into()]);
    }
    if inputs.metadata.is_some() {
        args.extend(["-map_metadata".into(), n_in.to_string(), "-map_chapters".into(), n_in.to_string()]);
    }
    if let Some((a, b)) = settings.range {
        args.extend(["-ss".into(), f(a.max(0.0)), "-t".into(), f((b - a).max(0.05))]);
    }
    if want_video {
        args.extend(video_encoder_args(settings));
        if !is_gif {
            args.extend(["-r".into(), f(fps)]);
        }
    } else {
        args.push("-vn".into());
    }
    if want_audio {
        args.extend(audio_encoder_args(settings));
    } else {
        args.push("-an".into());
    }
    if let Some(t) = &settings.title {
        args.extend(["-metadata".into(), format!("title={t}")]);
    }
    let _ = chunk_has_video;
    args.push(path_arg(inputs.out));
    args
}

/// ffconcat list for a PNG sequence overlay: `(file name, duration)` pairs.
pub fn ffconcat(frames: &[(String, Secs)]) -> String {
    let mut s = String::from("ffconcat version 1.0\n");
    for (name, d) in frames {
        s.push_str(&format!("file '{}'\nduration {}\n", name.replace('\'', "'\\''"), f(d.max(0.001))));
    }
    // The concat demuxer ignores the last duration unless the file repeats.
    if let Some((name, _)) = frames.last() {
        s.push_str(&format!("file '{}'\n", name.replace('\'', "'\\''")));
    }
    s
}
