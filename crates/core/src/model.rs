//! Document model.
//!
//! A [`Project`] owns media assets and compositions. A [`Composition`] is
//! edited primarily through its *script*: a flat list of [`Token`]s, each of
//! which points at a time range inside a source asset. Deleting text deletes
//! tokens; the script track of the timeline is derived from runs of tokens.
//! Everything that is not the script (titles, b-roll, music, captions) lives
//! on [`Layer`]s positioned in composition time.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Seconds. All times in the model are seconds as `f64`.
pub type Secs = f64;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);
        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

id_type!(AssetId);
id_type!(CompositionId);
id_type!(LayerId);
id_type!(SpeakerId);
id_type!(MarkerId);
id_type!(CommentId);
id_type!(SceneId);

// ---------------------------------------------------------------------------
// Project
// ---------------------------------------------------------------------------

pub const PROJECT_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub format_version: u32,
    pub name: String,
    pub assets: Vec<MediaAsset>,
    pub compositions: Vec<Composition>,
    pub speakers: Vec<Speaker>,
    /// Words the transcriber should prefer, and corrections applied afterwards.
    #[serde(default)]
    pub glossary: Vec<GlossaryEntry>,
    #[serde(default)]
    pub folders: Vec<String>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format_version: PROJECT_FORMAT_VERSION,
            name: name.into(),
            assets: Vec::new(),
            compositions: Vec::new(),
            speakers: Vec::new(),
            glossary: Vec::new(),
            folders: Vec::new(),
        }
    }

    pub fn asset(&self, id: AssetId) -> Option<&MediaAsset> {
        self.assets.iter().find(|a| a.id == id)
    }
    pub fn asset_mut(&mut self, id: AssetId) -> Option<&mut MediaAsset> {
        self.assets.iter_mut().find(|a| a.id == id)
    }
    pub fn composition(&self, id: CompositionId) -> Option<&Composition> {
        self.compositions.iter().find(|c| c.id == id)
    }
    pub fn composition_mut(&mut self, id: CompositionId) -> Option<&mut Composition> {
        self.compositions.iter_mut().find(|c| c.id == id)
    }
    pub fn speaker(&self, id: SpeakerId) -> Option<&Speaker> {
        self.speakers.iter().find(|s| s.id == id)
    }
    pub fn speaker_name(&self, id: Option<SpeakerId>) -> &str {
        id.and_then(|i| self.speaker(i)).map(|s| s.name.as_str()).unwrap_or("Speaker")
    }

    /// Returns the id of the speaker with this name, creating it if needed.
    pub fn ensure_speaker(&mut self, name: &str) -> SpeakerId {
        if let Some(s) = self.speakers.iter().find(|s| s.name == name) {
            return s.id;
        }
        let id = SpeakerId::new();
        let color = SPEAKER_COLORS[self.speakers.len() % SPEAKER_COLORS.len()];
        self.speakers.push(Speaker { id, name: name.to_string(), color });
        id
    }
}

pub const SPEAKER_COLORS: [Rgba; 6] = [
    Rgba::rgb(0xFF, 0x8A, 0x00),
    Rgba::rgb(0x14, 0xA3, 0xC7),
    Rgba::rgb(0xE6, 0x3E, 0x6D),
    Rgba::rgb(0x4C, 0xAF, 0x50),
    Rgba::rgb(0x8E, 0x5B, 0xD9),
    Rgba::rgb(0xF2, 0xC9, 0x1D),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: SpeakerId,
    pub name: String,
    pub color: Rgba,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlossaryEntry {
    /// Preferred spelling.
    pub term: String,
    /// Misrecognitions that should be rewritten to `term`.
    #[serde(default)]
    pub sounds_like: Vec<String>,
}

// ---------------------------------------------------------------------------
// Media
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaAsset {
    pub id: AssetId,
    pub name: String,
    pub path: PathBuf,
    pub kind: MediaKind,
    pub duration: Secs,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
    pub has_video: bool,
    #[serde(default)]
    pub sample_rate: u32,
    #[serde(default)]
    pub channels: u32,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub transcript: Option<Transcript>,
}

/// A transcript in *source* time for one asset.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub language: String,
    pub words: Vec<TranscriptWord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWord {
    pub text: String,
    pub start: Secs,
    pub end: Secs,
    #[serde(default)]
    pub speaker: Option<SpeakerId>,
    #[serde(default = "one")]
    pub confidence: f32,
}

fn one() -> f32 {
    1.0
}

// ---------------------------------------------------------------------------
// Script
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenKind {
    /// A spoken word.
    Word,
    /// Silence / non-speech between words. Rendered in the script as a gap.
    Gap,
    /// A gap clip: silence and a black frame for `src_end - src_start`
    /// seconds. Does not play any source media; `asset` is only a hint for
    /// which file's room tone to use.
    Blank,
}

/// One unit of the script. Points at `[src_start, src_end)` of `asset`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub kind: TokenKind,
    /// Display text. May differ from what was recognised after a correction;
    /// correcting never changes the media range.
    pub text: String,
    pub asset: AssetId,
    pub src_start: Secs,
    pub src_end: Secs,
    #[serde(default)]
    pub speaker: Option<SpeakerId>,
    /// Struck through: stays in the script, skipped in playback and export.
    #[serde(default)]
    pub ignored: bool,
    /// This token starts a new paragraph.
    #[serde(default)]
    pub paragraph_start: bool,
    /// This token starts a new scene (the "/" boundary).
    #[serde(default)]
    pub scene_start: bool,
    #[serde(default = "one")]
    pub confidence: f32,
    /// Media plays, but the text is hidden from script, captions and exports.
    #[serde(default)]
    pub hidden_text: bool,
    /// Highlight colour index, if highlighted.
    #[serde(default)]
    pub highlight: Option<u8>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    /// A marker (section header) placed immediately before this token.
    #[serde(default)]
    pub marker: Option<String>,
    /// An inline note such as "(laughter)" shown before this token. Not media.
    #[serde(default)]
    pub note: Option<String>,
}

impl Token {
    pub fn new(kind: TokenKind, text: impl Into<String>, asset: AssetId, src_start: Secs, src_end: Secs) -> Self {
        Self {
            kind,
            text: text.into(),
            asset,
            src_start,
            src_end,
            speaker: None,
            ignored: false,
            paragraph_start: false,
            scene_start: false,
            confidence: 1.0,
            hidden_text: false,
            highlight: None,
            bold: false,
            italic: false,
            marker: None,
            note: None,
        }
    }
    /// Whether this token's text should appear in captions and exports.
    pub fn shows_text(&self) -> bool {
        self.is_word() && !self.ignored && !self.hidden_text
    }
    pub fn duration(&self) -> Secs {
        (self.src_end - self.src_start).max(0.0)
    }
    pub fn is_word(&self) -> bool {
        self.kind == TokenKind::Word
    }
    /// Duration this token contributes to the composition.
    pub fn play_duration(&self) -> Secs {
        if self.ignored { 0.0 } else { self.duration() }
    }
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    pub id: CompositionId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub background: Rgba,
    pub script: Vec<Token>,
    /// How the script's own video is placed on the canvas.
    pub script_video: VideoPlacement,
    /// Audio processing for the script track.
    pub script_audio: AudioSettings,
    pub layers: Vec<Layer>,
    pub scenes: Vec<SceneSettings>,
    pub captions: CaptionSettings,
    pub markers: Vec<Marker>,
    pub comments: Vec<Comment>,
    /// Target integrated loudness for the master, LUFS. `None` = no normalisation.
    #[serde(default)]
    pub master_loudness: Option<f64>,
}

impl Composition {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: CompositionId::new(),
            name: name.into(),
            width: 1920,
            height: 1080,
            fps: 30.0,
            background: Rgba::rgb(0, 0, 0),
            script: Vec::new(),
            script_video: VideoPlacement::default(),
            script_audio: AudioSettings::default(),
            layers: Vec::new(),
            // There is always at least one scene.
            scenes: vec![SceneSettings::default()],
            captions: CaptionSettings::default(),
            markers: Vec::new(),
            comments: Vec::new(),
            master_loudness: None,
        }
    }
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }
    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AspectPreset {
    Landscape16x9,
    Portrait9x16,
    Square1x1,
    Portrait4x5,
    Classic4x3,
}

impl AspectPreset {
    pub const ALL: [AspectPreset; 5] = [
        AspectPreset::Landscape16x9,
        AspectPreset::Portrait9x16,
        AspectPreset::Square1x1,
        AspectPreset::Portrait4x5,
        AspectPreset::Classic4x3,
    ];
    pub fn size(self) -> (u32, u32) {
        match self {
            AspectPreset::Landscape16x9 => (1920, 1080),
            AspectPreset::Portrait9x16 => (1080, 1920),
            AspectPreset::Square1x1 => (1080, 1080),
            AspectPreset::Portrait4x5 => (1080, 1350),
            AspectPreset::Classic4x3 => (1440, 1080),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            AspectPreset::Landscape16x9 => "16:9 Landscape",
            AspectPreset::Portrait9x16 => "9:16 Portrait",
            AspectPreset::Square1x1 => "1:1 Square",
            AspectPreset::Portrait4x5 => "4:5 Portrait",
            AspectPreset::Classic4x3 => "4:3 Classic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    pub const WHITE: Rgba = Rgba::rgb(255, 255, 255);
    pub const BLACK: Rgba = Rgba::rgb(0, 0, 0);
    /// `0xRRGGBB` form used by ffmpeg, with `@alpha`.
    pub fn ffmpeg(self) -> String {
        format!("0x{:02X}{:02X}{:02X}@{:.3}", self.r, self.g, self.b, self.a as f64 / 255.0)
    }
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
    pub fn parse_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 && s.len() != 8 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(if s.len() == 6 {
            Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
        } else {
            Rgba::rgba((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)
        })
    }
}

/// Position on the canvas in normalised units: `x`/`y` are the centre, as a
/// fraction of the canvas, and `w`/`h` are the size as a fraction of the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub rotation_deg: f64,
    pub opacity: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Self { x: 0.5, y: 0.5, w: 1.0, h: 1.0, rotation_deg: 0.0, opacity: 1.0 }
    }
}

/// Fractions of the source cropped from each edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Crop {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FitMode {
    /// Letterbox inside the box.
    #[default]
    Fit,
    /// Cover the box, cropping overflow.
    Fill,
    /// Ignore aspect ratio.
    Stretch,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct VideoPlacement {
    pub transform: Transform,
    pub crop: Crop,
    pub fit: FitMode,
    #[serde(default)]
    pub effects: Vec<VideoEffect>,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VideoEffect {
    /// Key out a colour (green screen).
    ChromaKey { color: Rgba, similarity: f64, blend: f64 },
    Blur { radius: f64 },
    Brightness { value: f64 },
    Contrast { value: f64 },
    Saturation { value: f64 },
    Grayscale,
    /// Mirror horizontally.
    Flip,
    CornerRadius { fraction: f64 },
    Vignette,
    Invert,
    Pixelate { size: u32 },
    Grain { amount: f64 },
    /// Colour temperature in kelvin (6500 = neutral).
    Temperature { kelvin: f64 },
    /// Exposure in stops.
    Exposure { stops: f64 },
    FlipVertical,
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioSettings {
    /// Linear gain in dB applied before effects.
    pub gain_db: f64,
    pub muted: bool,
    /// -1.0 = hard left, 0 = centre, 1.0 = hard right.
    #[serde(default)]
    pub pan: f64,
    #[serde(default)]
    pub effects: Vec<AudioEffect>,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self { gain_db: 0.0, muted: false, pan: 0.0, effects: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AudioEffect {
    /// Speech clean-up chain: denoise, high-pass, de-ess, level.
    /// `intensity` is 0..=1.
    SpeechEnhance { intensity: f64 },
    Denoise { strength_db: f64 },
    HighPass { hz: f64 },
    LowPass { hz: f64 },
    /// One peaking band.
    Eq { hz: f64, gain_db: f64, q: f64 },
    Compressor { threshold_db: f64, ratio: f64, attack_ms: f64, release_ms: f64, makeup_db: f64 },
    Limiter { ceiling_db: f64 },
    DeEsser { intensity: f64 },
    Gate { threshold_db: f64 },
    LowShelf { hz: f64, gain_db: f64 },
    HighShelf { hz: f64, gain_db: f64 },
    Reverb { mix: f64, room: f64 },
    Distortion { drive: f64 },
    Bitcrusher { bits: u32 },
    Flanger { depth: f64, rate_hz: f64 },
    /// Per-track loudness target, LUFS.
    Loudness { target_lufs: f64 },
}

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    /// Composition time at which the layer starts.
    pub start: Secs,
    pub duration: Secs,
    /// Stacking / track index. Higher draws on top.
    pub track: i32,
    pub transform: Transform,
    pub content: LayerContent,
    #[serde(default)]
    pub fade_in: Secs,
    #[serde(default)]
    pub fade_out: Secs,
    #[serde(default)]
    pub animation_in: Animation,
    #[serde(default)]
    pub animation_out: Animation,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub locked: bool,
}

impl Layer {
    pub fn new(name: impl Into<String>, start: Secs, duration: Secs, content: LayerContent) -> Self {
        Self {
            id: LayerId::new(),
            name: name.into(),
            start,
            duration,
            track: 1,
            transform: Transform::default(),
            content,
            fade_in: 0.0,
            fade_out: 0.0,
            animation_in: Animation::None,
            animation_out: Animation::None,
            hidden: false,
            locked: false,
        }
    }
    pub fn end(&self) -> Secs {
        self.start + self.duration
    }
    pub fn is_visual(&self) -> bool {
        !matches!(self.content, LayerContent::Audio(_))
    }
    pub fn has_audio(&self) -> bool {
        matches!(self.content, LayerContent::Audio(_) | LayerContent::Video(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Animation {
    #[default]
    None,
    Fade,
    SlideLeft,
    SlideRight,
    SlideUp,
    SlideDown,
    ZoomIn,
    ZoomOut,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LayerContent {
    Video(MediaClip),
    Image(ImageClip),
    Audio(MediaClip),
    Text(TextStyle),
    Shape(Shape),
    /// Animated waveform of the composition audio (audiogram).
    Waveform(WaveformStyle),
    /// Playback progress bar.
    Progress { color: Rgba, track_color: Rgba },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaClip {
    pub asset: AssetId,
    /// Offset into the source at which this clip starts.
    pub src_in: Secs,
    /// Playback rate. 1.0 = normal.
    pub speed: f64,
    pub audio: AudioSettings,
    #[serde(default)]
    pub crop: Crop,
    #[serde(default)]
    pub fit: FitMode,
    #[serde(default)]
    pub effects: Vec<VideoEffect>,
    /// Lower this clip's volume while the script is speaking.
    #[serde(default)]
    pub duck: Option<Ducking>,
    /// Volume automation in dB, as (layer-relative time, dB) points.
    #[serde(default)]
    pub volume_keys: Vec<(Secs, f64)>,
    #[serde(default)]
    pub loop_playback: bool,
}

impl MediaClip {
    pub fn new(asset: AssetId) -> Self {
        Self {
            asset,
            src_in: 0.0,
            speed: 1.0,
            audio: AudioSettings::default(),
            crop: Crop::default(),
            fit: FitMode::Fit,
            effects: Vec::new(),
            duck: None,
            volume_keys: Vec::new(),
            loop_playback: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ducking {
    /// How far to reduce, in dB (positive number).
    pub amount_db: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
}

impl Default for Ducking {
    fn default() -> Self {
        Self { amount_db: 12.0, attack_ms: 80.0, release_ms: 400.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageClip {
    pub asset: AssetId,
    #[serde(default)]
    pub crop: Crop,
    #[serde(default)]
    pub fit: FitMode,
    #[serde(default)]
    pub effects: Vec<VideoEffect>,
    /// Slow zoom over the layer's duration ("Ken Burns"). 1.0 = none.
    #[serde(default = "one_f64")]
    pub zoom_to: f64,
}

fn one_f64() -> f64 {
    1.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    pub text: String,
    pub font: String,
    /// Font size as a fraction of canvas height.
    pub size: f64,
    pub color: Rgba,
    pub bold: bool,
    pub italic: bool,
    pub align: TextAlign,
    #[serde(default)]
    pub background: Option<Rgba>,
    #[serde(default)]
    pub outline: Option<(Rgba, f64)>,
    #[serde(default)]
    pub shadow: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            text: "Title".into(),
            font: "Helvetica Neue".into(),
            size: 0.08,
            color: Rgba::WHITE,
            bold: true,
            italic: false,
            align: TextAlign::Center,
            background: None,
            outline: None,
            shadow: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Line,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub kind: ShapeKind,
    pub fill: Rgba,
    #[serde(default)]
    pub stroke: Option<(Rgba, f64)>,
    #[serde(default)]
    pub corner_radius: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WaveformKind {
    #[default]
    Bars,
    Line,
    Mirror,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaveformStyle {
    pub kind: WaveformKind,
    pub color: Rgba,
}

// ---------------------------------------------------------------------------
// Scenes, captions, markers, comments
// ---------------------------------------------------------------------------

/// Settings for the n-th scene (scenes are delimited by `Token::scene_start`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneSettings {
    pub id: SceneId,
    #[serde(default)]
    pub name: String,
    /// Transition *into* this scene.
    #[serde(default)]
    pub transition: Transition,
    #[serde(default = "default_transition_secs")]
    pub transition_secs: Secs,
    /// Overrides the composition's script video placement for this scene
    /// (used for layouts and multicam angle choice).
    #[serde(default)]
    pub script_video: Option<VideoPlacement>,
}

fn default_transition_secs() -> Secs {
    0.5
}

impl Default for SceneSettings {
    fn default() -> Self {
        Self {
            id: SceneId::new(),
            name: String::new(),
            transition: Transition::Cut,
            transition_secs: default_transition_secs(),
            script_video: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Transition {
    #[default]
    Cut,
    CrossDissolve,
    FadeThroughBlack,
    WipeLeft,
    WipeRight,
    SlideLeft,
    SlideUp,
}

impl Transition {
    pub const ALL: [Transition; 7] = [
        Transition::Cut,
        Transition::CrossDissolve,
        Transition::FadeThroughBlack,
        Transition::WipeLeft,
        Transition::WipeRight,
        Transition::SlideLeft,
        Transition::SlideUp,
    ];
    /// Name of the matching ffmpeg `xfade` transition, if any.
    pub fn xfade_name(self) -> Option<&'static str> {
        match self {
            Transition::Cut => None,
            Transition::CrossDissolve => Some("fade"),
            Transition::FadeThroughBlack => Some("fadeblack"),
            Transition::WipeLeft => Some("wipeleft"),
            Transition::WipeRight => Some("wiperight"),
            Transition::SlideLeft => Some("slideleft"),
            Transition::SlideUp => Some("slideup"),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Transition::Cut => "Cut",
            Transition::CrossDissolve => "Cross dissolve",
            Transition::FadeThroughBlack => "Fade through black",
            Transition::WipeLeft => "Wipe left",
            Transition::WipeRight => "Wipe right",
            Transition::SlideLeft => "Slide left",
            Transition::SlideUp => "Slide up",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CaptionHighlight {
    /// Plain captions.
    #[default]
    None,
    /// The word being spoken changes colour.
    ActiveWord,
    /// Words appear as they are spoken.
    Reveal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptionSettings {
    pub enabled: bool,
    pub font: String,
    /// Font size as a fraction of canvas height.
    pub size: f64,
    pub color: Rgba,
    pub highlight_color: Rgba,
    pub highlight: CaptionHighlight,
    pub background: Option<Rgba>,
    pub outline: Option<(Rgba, f64)>,
    /// Vertical centre of the caption block, fraction of canvas height.
    pub y: f64,
    pub max_chars_per_line: usize,
    pub max_lines: usize,
    pub uppercase: bool,
    pub bold: bool,
}

impl Default for CaptionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            font: "Helvetica Neue".into(),
            size: 0.055,
            color: Rgba::WHITE,
            highlight_color: Rgba::rgb(0xFF, 0x8A, 0x00),
            highlight: CaptionHighlight::None,
            background: None,
            outline: Some((Rgba::BLACK, 2.0)),
            y: 0.86,
            max_chars_per_line: 32,
            max_lines: 2,
            uppercase: false,
            bold: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: MarkerId,
    /// Composition time.
    pub time: Secs,
    pub name: String,
    /// Chapter markers are exported as chapters.
    #[serde(default)]
    pub chapter: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: CommentId,
    /// Composition time when created; kept as a fallback if the anchor is cut.
    pub time: Secs,
    /// The source moment this comment is attached to, so it follows edits.
    #[serde(default)]
    pub anchor: Option<(AssetId, Secs)>,
    pub author: String,
    pub text: String,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub replies: Vec<(String, String)>,
}
