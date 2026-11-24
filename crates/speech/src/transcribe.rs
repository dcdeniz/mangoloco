//! Local speech-to-text with word-level timestamps (whisper.cpp, linked in).

use std::ffi::{c_int, c_void};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use mango_core::{Transcript, TranscriptWord};
use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters,
};

use crate::Cancelled;
pub use crate::words::Timing;
use crate::proc::{ChildGuard, drain_stderr, last_lines, tool};

/// Sample rate whisper works at.
pub const SAMPLE_RATE: usize = 16_000;

#[derive(Clone, Debug)]
pub struct Options {
    /// Path to a whisper.cpp `ggml-*.bin` model.
    pub model: PathBuf,
    /// ISO 639-1 code ("en", "de", ...). `None` = auto-detect (multilingual models only;
    /// English-only models always transcribe English).
    pub language: Option<String>,
    /// Preferred spellings, fed to the decoder as an initial prompt. This is a hint, not a
    /// guarantee: whisper usually adopts the spelling but is free to ignore it.
    pub glossary: Vec<String>,
    /// Number of CPU threads. `None` = up to 4.
    pub threads: Option<usize>,
}

impl Options {
    pub fn new(model: impl Into<PathBuf>) -> Self {
        Self { model: model.into(), language: None, glossary: Vec::new(), threads: None }
    }
}

/// Transcribes any media file ffmpeg can read.
///
/// The first audio stream is decoded to 16 kHz mono through an ffmpeg pipe and run through
/// whisper locally. Words come back in source time, punctuation attached ("Hello," "world."),
/// no leading spaces, monotonic and non-overlapping, clamped to the audio duration, with
/// `confidence` taken from the decoder's token probabilities.
///
/// `on_progress` receives values in `0..=1`, never decreasing, ending at `1.0` on success.
/// Setting `cancel` aborts promptly; the error then downcasts to [`crate::Cancelled`]
/// (see [`crate::is_cancelled`]).
///
/// Errors: missing file, missing/invalid model, no audio stream. Silent audio is not an
/// error: it returns a transcript with zero words.
pub fn transcribe(
    media: &Path,
    opts: &Options,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Transcript> {
    check_model_file(&opts.model)?;
    let mut progress = Progress::new(on_progress);
    progress.report(0.0);
    let samples = decode_audio(media, cancel)?;
    progress.report(DECODE_SHARE);
    let transcript = transcribe_pcm(&samples, opts, &mut progress, cancel)?;
    progress.report(1.0);
    Ok(transcript)
}

/// Transcribes 16 kHz mono samples that are already in memory. Same contract as [`transcribe`].
pub fn transcribe_samples(
    samples: &[f32],
    opts: &Options,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<Transcript> {
    check_model_file(&opts.model)?;
    let mut progress = Progress::new(on_progress);
    progress.report(0.0);
    let transcript = transcribe_pcm(samples, opts, &mut progress, cancel)?;
    progress.report(1.0);
    Ok(transcript)
}

/// Whether whisper was built with GPU (Metal) support in this binary.
pub fn gpu_enabled() -> bool {
    cfg!(target_os = "macos")
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// Share of the progress bar given to decoding the audio.
const DECODE_SHARE: f32 = 0.03;

/// Clamps to 0..=1 and never goes backwards.
struct Progress<'a> {
    sink: &'a mut dyn FnMut(f32),
    last: f32,
}

impl<'a> Progress<'a> {
    fn new(sink: &'a mut dyn FnMut(f32)) -> Self {
        Self { sink, last: -1.0 }
    }
    fn report(&mut self, value: f32) {
        let v = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
        if v > self.last {
            self.last = v;
            (self.sink)(v);
        }
    }
}

// ---------------------------------------------------------------------------
// Audio decoding
// ---------------------------------------------------------------------------

/// Decodes the first audio stream of `media` to 16 kHz mono `f32`, time-aligned so that sample 0
/// is source time 0 even when the audio stream starts late.
pub fn decode_audio(media: &Path, cancel: &AtomicBool) -> Result<Vec<f32>> {
    if !media.exists() {
        bail!("media file not found: {}", media.display());
    }
    if media.is_dir() {
        bail!("{} is a folder, not a media file", media.display());
    }
    let ffmpeg = tool("ffmpeg")?;
    let mut cmd = Command::new(&ffmpeg);
    cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(media)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1"])
        .args(["-af", "aresample=16000:async=1:first_pts=0"])
        .args(["-f", "f32le", "-acodec", "pcm_f32le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().with_context(|| format!("could not start {}", ffmpeg.display()))?;
    let mut child = ChildGuard(child);
    let stderr = drain_stderr(child.0.stderr.take().expect("stderr was piped"));
    let mut stdout = child.0.stdout.take().expect("stdout was piped");

    let mut samples: Vec<f32> = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.0.kill();
            return Err(Cancelled.into());
        }
        let n = match stdout.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(anyhow!(e).context("reading decoded audio from ffmpeg")),
        };
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        let whole = pending.len() / 4 * 4;
        samples.extend(pending[..whole].chunks_exact(4).map(|b| {
            let v = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }
        }));
        pending.drain(..whole);
    }
    let status = child.0.wait().context("waiting for ffmpeg")?;
    let stderr = stderr.join().unwrap_or_default();
    if !status.success() {
        let lower = stderr.to_lowercase();
        if lower.contains("matches no streams") || lower.contains("does not contain any stream") {
            bail!("{} has no audio stream, so there is nothing to transcribe", media.display());
        }
        bail!("ffmpeg could not read {}: {}", media.display(), last_lines(&stderr, 4));
    }
    if samples.is_empty() {
        bail!("{} has no audio stream, so there is nothing to transcribe", media.display());
    }
    Ok(samples)
}

// ---------------------------------------------------------------------------
// Model loading
// ---------------------------------------------------------------------------

/// Walks the whole ggml file (mel filters, vocabulary, then every tensor record) without
/// reading the weights, and reports whether it ends exactly where the last tensor ends.
/// A truncated file makes whisper.cpp fail half-way through loading, which leaks GPU buffers
/// and later aborts the process at exit, so such files must never reach it.
fn model_file_is_complete(model: &Path, len: u64) -> std::io::Result<bool> {
    use std::io::{BufReader, Seek, SeekFrom};
    let mut f = BufReader::new(std::fs::File::open(model)?);
    let read_i32 = |f: &mut BufReader<std::fs::File>| -> std::io::Result<i32> {
        let mut b = [0u8; 4];
        f.read_exact(&mut b)?;
        Ok(i32::from_le_bytes(b))
    };
    f.seek(SeekFrom::Start(48))?;
    let n_mel = read_i32(&mut f)?;
    let n_fft = read_i32(&mut f)?;
    if !(1..=512).contains(&n_mel) || !(1..=4096).contains(&n_fft) {
        return Ok(false);
    }
    f.seek_relative(n_mel as i64 * n_fft as i64 * 4)?;
    let n_vocab = read_i32(&mut f)?;
    if !(1..=200_000).contains(&n_vocab) {
        return Ok(false);
    }
    for _ in 0..n_vocab {
        let word_len = read_i32(&mut f)?;
        if !(0..=1024).contains(&word_len) {
            return Ok(false);
        }
        f.seek_relative(word_len as i64)?;
    }
    let mut tensors = 0usize;
    loop {
        let pos = f.stream_position()?;
        if pos == len {
            return Ok(tensors > 0);
        }
        if pos > len {
            return Ok(false);
        }
        let (n_dims, name_len, ttype) = match (read_i32(&mut f), read_i32(&mut f), read_i32(&mut f)) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            _ => return Ok(false),
        };
        if !(1..=4).contains(&n_dims) || !(1..=256).contains(&name_len) {
            return Ok(false);
        }
        let mut elements: u64 = 1;
        for _ in 0..n_dims {
            let d = read_i32(&mut f)?;
            if d <= 0 {
                return Ok(false);
            }
            elements = elements.saturating_mul(d as u64);
        }
        // (bytes per block, elements per block) for the ggml tensor types whisper models use.
        let (block_bytes, block_elems): (u64, u64) = match ttype {
            0 => (4, 1),      // f32
            1 => (2, 1),      // f16
            2 => (18, 32),    // q4_0
            3 => (20, 32),    // q4_1
            6 => (22, 32),    // q5_0
            7 => (24, 32),    // q5_1
            8 => (34, 32),    // q8_0
            12 => (144, 256), // q4_k
            13 => (176, 256), // q5_k
            14 => (210, 256), // q6_k
            // A type this code does not know: cannot check further, let whisper decide.
            _ => return Ok(true),
        };
        let bytes = elements / block_elems * block_bytes;
        let next = f.stream_position()?.saturating_add(name_len as u64).saturating_add(bytes);
        if next > len {
            return Ok(false);
        }
        f.seek(SeekFrom::Start(next))?;
        tensors += 1;
    }
}

/// The fixed-size header of a ggml whisper model.
struct ModelHeader {
    n_vocab: i32,
    n_audio_layer: i32,
    n_text_layer: i32,
}

/// Validates the model file before whisper.cpp sees it: whisper.cpp aborts the whole process
/// on some malformed files instead of returning an error.
fn check_model_file(model: &Path) -> Result<ModelHeader> {
    let meta = std::fs::metadata(model)
        .map_err(|e| anyhow!("speech model not found at {} ({e}). Download a model first.", model.display()))?;
    if !meta.is_file() {
        bail!("speech model path {} is not a file", model.display());
    }
    let not_a_model = || {
        anyhow!("{} is not a whisper.cpp ggml model file (it may be a partial or corrupt download)", model.display())
    };
    // magic, n_vocab, n_audio_ctx, n_audio_state, n_audio_head, n_audio_layer,
    // n_text_ctx, n_text_state, n_text_head, n_text_layer, n_mels, ftype (i32 LE each).
    let mut head = [0u8; 48];
    std::fs::File::open(model).and_then(|mut f| f.read_exact(&mut head)).map_err(|_| not_a_model())?;
    let field = |i: usize| i32::from_le_bytes([head[i * 4], head[i * 4 + 1], head[i * 4 + 2], head[i * 4 + 3]]);
    if field(0) != 0x6767_6d6c {
        return Err(not_a_model());
    }
    let (n_vocab, n_audio_ctx, n_audio_state, n_audio_head, n_audio_layer) = (field(1), field(2), field(3), field(4), field(5));
    let (n_text_ctx, n_text_state, n_text_head, n_text_layer, n_mels, ftype) = (field(6), field(7), field(8), field(9), field(10), field(11));
    let plausible = (51864..=51870).contains(&n_vocab)
        && n_audio_ctx == 1500
        && matches!(n_audio_state, 384 | 512 | 768 | 1024 | 1280)
        && (1..=32).contains(&n_audio_head)
        && (1..=64).contains(&n_audio_layer)
        && n_text_ctx == 448
        && n_text_state == n_audio_state
        && (1..=32).contains(&n_text_head)
        && (1..=64).contains(&n_text_layer)
        && matches!(n_mels, 80 | 128)
        && (0..100).contains(&(ftype % 1000))
        && (0..=3).contains(&(ftype / 1000));
    if !plausible {
        return Err(not_a_model());
    }
    if !model_file_is_complete(model, meta.len()).unwrap_or(false) {
        bail!(
            "the speech model {} is incomplete or damaged (probably an interrupted download); delete it and download it again",
            model.display()
        );
    }
    Ok(ModelHeader { n_vocab, n_audio_layer, n_text_layer })
}

struct LoadedModel {
    ctx: WhisperContext,
    /// Whether cross-attention (DTW) token timestamps are available for this model.
    dtw: bool,
    /// Typical DTW lag of this model family, seconds.
    default_lag: f64,
}

// The model is loaded per call and freed when the call returns. Keeping a context alive in a
// static would make ggml's Metal backend abort the process at exit (it asserts that every
// context was freed before its device is torn down).
fn load_model(model: &Path) -> Result<LoadedModel> {
    let header = check_model_file(model)?;
    whisper_rs::install_logging_hooks();
    let fail = || {
        anyhow!(
            "could not load the speech model {} (the file may be a partial or corrupt download; delete it and download it again)",
            model.display()
        )
    };
    // First try with the alignment-head preset that matches the model; if whisper rejects
    // it (custom or fine-tuned model with a different layout) load without DTW.
    if let Some(preset) = dtw_preset(&header) {
        let default_lag = match preset {
            DtwModelPreset::Tiny | DtwModelPreset::TinyEn => crate::words::TINY_LAG,
            _ => crate::words::DEFAULT_LAG,
        };
        let mut params = WhisperContextParameters::default();
        params.dtw_parameters(DtwParameters { mode: DtwMode::ModelPreset { model_preset: preset }, ..Default::default() });
        if let Ok(ctx) = WhisperContext::new_with_params(model, params) {
            return Ok(LoadedModel { ctx, dtw: true, default_lag });
        }
    }
    let ctx = WhisperContext::new_with_params(model, WhisperContextParameters::default()).map_err(|_| fail())?;
    Ok(LoadedModel { ctx, dtw: false, default_lag: crate::words::DEFAULT_LAG })
}

/// The alignment-head preset for the standard model shapes.
fn dtw_preset(header: &ModelHeader) -> Option<DtwModelPreset> {
    let multilingual = header.n_vocab >= 51865;
    Some(match (header.n_audio_layer, header.n_text_layer, multilingual) {
        (4, 4, false) => DtwModelPreset::TinyEn,
        (4, 4, true) => DtwModelPreset::Tiny,
        (6, 6, false) => DtwModelPreset::BaseEn,
        (6, 6, true) => DtwModelPreset::Base,
        (12, 12, false) => DtwModelPreset::SmallEn,
        (12, 12, true) => DtwModelPreset::Small,
        (24, 24, false) => DtwModelPreset::MediumEn,
        (24, 24, true) => DtwModelPreset::Medium,
        (32, 4, true) => DtwModelPreset::LargeV3Turbo,
        (32, 32, true) if header.n_vocab >= 51866 => DtwModelPreset::LargeV3,
        (32, 32, true) => DtwModelPreset::LargeV2,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Running whisper
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) struct RawToken {
    pub bytes: Vec<u8>,
    /// whisper.cpp's heuristic token times, seconds.
    pub t0: f64,
    pub t1: f64,
    /// Cross-attention DTW time, seconds, when available.
    pub t_dtw: Option<f64>,
    /// Where whisper itself would have put a timestamp just before this token, seconds, with
    /// the probability it gave that timestamp. Precise when the probability is not tiny.
    pub ts: Option<(f64, f32)>,
    pub p: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct RawSegment {
    pub tokens: Vec<RawToken>,
}

struct CallbackState<'a, 'p> {
    progress: &'a mut Progress<'p>,
    base: f32,
    span: f32,
    panic: Option<Box<dyn std::any::Any + Send>>,
}

unsafe extern "C" fn progress_trampoline(
    _ctx: *mut whisper_rs::WhisperSysContext,
    _state: *mut whisper_rs::WhisperSysState,
    percent: c_int,
    user_data: *mut c_void,
) {
    // SAFETY: user_data is the CallbackState that outlives the whisper_full call, and this
    // callback is only invoked on the thread that called whisper_full.
    let st = unsafe { &mut *(user_data as *mut CallbackState) };
    if st.panic.is_some() {
        return;
    }
    let value = st.base + st.span * (percent as f32 / 100.0).clamp(0.0, 1.0);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| st.progress.report(value)));
    if let Err(payload) = result {
        st.panic = Some(payload);
    }
}

unsafe extern "C" fn encoder_begin_trampoline(
    _ctx: *mut whisper_rs::WhisperSysContext,
    _state: *mut whisper_rs::WhisperSysState,
    user_data: *mut c_void,
) -> bool {
    // SAFETY: user_data points at an AtomicBool that outlives the call.
    let cancel = unsafe { &*(user_data as *const AtomicBool) };
    !cancel.load(Ordering::Relaxed)
}

unsafe extern "C" fn abort_trampoline(user_data: *mut c_void) -> bool {
    // SAFETY: user_data points at an AtomicBool that outlives the call; atomics are safe to
    // read from whichever thread ggml calls this on.
    let cancel = unsafe { &*(user_data as *const AtomicBool) };
    cancel.load(Ordering::Relaxed)
}

fn default_threads(opts: &Options) -> c_int {
    let available = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    opts.threads.unwrap_or(4).clamp(1, available.max(1)) as c_int
}

/// Resolves the language to pass to whisper, validating it against the model.
fn resolve_language(model: &LoadedModel, opts: &Options) -> Result<Option<String>> {
    let multilingual = model.ctx.is_multilingual();
    match opts.language.as_deref().map(|l| l.trim().to_lowercase()) {
        Some(lang) if !lang.is_empty() && lang != "auto" => {
            if whisper_rs::get_lang_id(&lang).is_none() {
                bail!("unknown language code \"{lang}\" (use a code such as \"en\", \"de\", \"tr\")");
            }
            if !multilingual && lang != "en" && lang != "english" {
                bail!("the selected speech model only understands English; choose a multilingual model to transcribe \"{lang}\"");
            }
            Ok(Some(lang))
        }
        _ if multilingual => Ok(None),
        _ => Ok(Some("en".to_string())),
    }
}

/// The decoder prompt that carries the glossary.
fn glossary_prompt(glossary: &[String]) -> Option<String> {
    let terms: Vec<String> = glossary
        .iter()
        .map(|t| t.replace('\0', " ").split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return None;
    }
    // Whisper keeps only the last ~224 tokens of a prompt; keep it comfortably below that.
    let mut prompt = String::new();
    for t in terms {
        if prompt.len() + t.len() > 600 {
            break;
        }
        if !prompt.is_empty() {
            prompt.push_str(", ");
        }
        prompt.push_str(&t);
    }
    Some(format!("Glossary: {prompt}."))
}

struct WhisperRun<'a> {
    model: &'a LoadedModel,
    state: whisper_rs::WhisperState,
    language: Option<String>,
    prompt: Option<String>,
    threads: c_int,
    detected_language: Option<String>,
}

impl<'a> WhisperRun<'a> {
    fn new(model: &'a LoadedModel, opts: &Options) -> Result<Self> {
        let language = resolve_language(model, opts)?;
        let state = model.ctx.create_state().map_err(|e| anyhow!("could not start the speech engine: {e:?}"))?;
        Ok(Self {
            model,
            state,
            language,
            prompt: glossary_prompt(&opts.glossary),
            threads: default_threads(opts),
            detected_language: None,
        })
    }

    /// Runs whisper over `samples` and returns its segments with times relative to the start
    /// of `samples`. Progress is mapped into `base..base+span`.
    fn run(
        &mut self,
        samples: &[f32],
        progress: &mut Progress,
        base: f32,
        span: f32,
        cancel: &AtomicBool,
    ) -> Result<Vec<RawSegment>> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        // whisper.cpp refuses clips shorter than 100 ms and behaves poorly below about a
        // second; pad with silence.
        let mut padded;
        let audio: &[f32] = if samples.len() < SAMPLE_RATE + SAMPLE_RATE / 4 {
            padded = samples.to_vec();
            padded.resize(SAMPLE_RATE + SAMPLE_RATE / 4, 0.0);
            &padded
        } else {
            samples
        };

        let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 });
        params.set_n_threads(self.threads);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_translate(false);
        params.set_token_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        match (&self.language, &self.detected_language) {
            (Some(lang), _) => params.set_language(Some(lang.as_str())),
            // Keep the language detected on the first chunk for the rest of the file.
            (None, Some(lang)) => params.set_language(Some(lang.as_str())),
            (None, None) => params.set_language(Some("auto")),
        }
        if let Some(prompt) = &self.prompt {
            params.set_initial_prompt(prompt);
        }

        let mut cb = CallbackState { progress, base, span, panic: None };
        // SAFETY: the callbacks only run during `full` below; `cb` and `cancel` outlive it.
        unsafe {
            params.set_progress_callback(Some(progress_trampoline));
            params.set_progress_callback_user_data(&mut cb as *mut CallbackState as *mut c_void);
            params.set_start_encoder_callback(Some(encoder_begin_trampoline));
            params.set_start_encoder_callback_user_data(cancel as *const AtomicBool as *mut c_void);
            params.set_abort_callback(Some(abort_trampoline));
            params.set_abort_callback_user_data(cancel as *const AtomicBool as *mut c_void);
        }
        let result = self.state.full(params, audio);
        if let Some(payload) = cb.panic.take() {
            std::panic::resume_unwind(payload);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        result.map_err(|e| anyhow!("speech recognition failed: {e:?}"))?;

        if self.detected_language.is_none() {
            let id = self.state.full_lang_id_from_state();
            self.detected_language = whisper_rs::get_lang_str(id).map(str::to_string);
        }

        let limit = samples.len() as f64 / SAMPLE_RATE as f64;
        let eot = self.model.ctx.token_eot();
        let mut segments = Vec::new();
        let beg = self.model.ctx.token_beg();
        // Start of the 30-second window the decoder is working in; timestamp tokens count from it.
        let mut window: Option<f64> = None;
        for seg in self.state.as_iter() {
            let mut tokens = Vec::new();
            let seg_start = seg.start_timestamp() as f64 / 100.0;
            // A segment normally opens with the timestamp token of its own start time.
            if let Some(first) = seg.get_token(0) {
                let id = first.token_id();
                if id >= beg {
                    window = Some(seg_start - 0.02 * (id - beg) as f64);
                }
            }
            if window.is_some_and(|w| seg_start < w - 0.01 || seg_start > w + 30.5) {
                window = None; // inherited window does not fit this segment
            }
            for i in 0..seg.n_tokens() {
                let Some(tok) = seg.get_token(i) else { continue };
                let data = tok.token_data();
                if data.id >= eot {
                    continue; // special and timestamp tokens
                }
                let Ok(bytes) = tok.to_bytes() else { continue };
                let ts = match window {
                    Some(w) if data.tid >= beg && data.pt.is_finite() => {
                        let t = w + 0.02 * (data.tid - beg) as f64;
                        (t >= 0.0 && t <= limit).then_some((t, data.pt))
                    }
                    _ => None,
                };
                tokens.push(RawToken {
                    bytes: bytes.to_vec(),
                    t0: (data.t0 as f64 / 100.0).clamp(0.0, limit),
                    t1: (data.t1 as f64 / 100.0).clamp(0.0, limit),
                    t_dtw: (self.model.dtw && data.t_dtw >= 0).then(|| (data.t_dtw as f64 / 100.0).clamp(0.0, limit)),
                    ts,
                    p: data.p,
                });
            }
            segments.push(RawSegment { tokens });
        }
        Ok(segments)
    }
}

fn transcribe_pcm(samples: &[f32], opts: &Options, progress: &mut Progress, cancel: &AtomicBool) -> Result<Transcript> {
    transcribe_pcm_with(samples, opts, Timing::DtwSnapped, progress, cancel)
}

fn transcribe_pcm_with(
    samples: &[f32],
    opts: &Options,
    timing: Timing,
    progress: &mut Progress,
    cancel: &AtomicBool,
) -> Result<Transcript> {
    let duration = samples.len() as f64 / SAMPLE_RATE as f64;
    let model = load_model(&opts.model)?;
    let mut run = WhisperRun::new(&model, opts)?;
    let activity = crate::words::analyse(samples);
    // Nothing audible: whisper would only hallucinate ("Thank you.", "[BLANK_AUDIO]").
    if activity.regions.as_ref().is_some_and(|r| r.is_empty()) {
        return Ok(Transcript { language: run_language(&run), words: Vec::new() });
    }
    let segments = run.run(samples, progress, DECODE_SHARE, 1.0 - DECODE_SHARE, cancel)?;
    let drafts = crate::words::draft_words(&segments);
    let words = crate::words::time_words(drafts, &activity, timing, duration, model.default_lag);
    let words = words
        .into_iter()
        .map(|w| TranscriptWord { text: w.text, start: w.start, end: w.end, speaker: None, confidence: w.confidence })
        .collect();
    Ok(Transcript { language: run_language(&run), words })
}

fn run_language(run: &WhisperRun) -> String {
    run.language.clone().or(run.detected_language.clone()).unwrap_or_else(|| "en".to_string())
}

/// Transcribes with a specific timestamp strategy. Exists so the alternatives can be measured
/// against each other; the app should call [`transcribe`].
#[doc(hidden)]
pub fn transcribe_samples_with(samples: &[f32], opts: &Options, timing: Timing) -> Result<Transcript> {
    check_model_file(&opts.model)?;
    let mut sink = |_: f32| {};
    let mut progress = Progress::new(&mut sink);
    transcribe_pcm_with(samples, opts, timing, &mut progress, &AtomicBool::new(false))
}

/// The audible stretches the timing step works with. For diagnostics and tests.
#[doc(hidden)]
pub fn audible_stretches(samples: &[f32]) -> Option<Vec<(f64, f64)>> {
    crate::words::analyse(samples).regions
}
