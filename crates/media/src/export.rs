//! The export runner: executes the two-stage plan from `mango_core::render`.

use crate::ffmpeg::{self, Cancelled};
use anyhow::{Context, Result, anyhow, bail};
use mango_core::render::{self, AudioCodec, Chunk, ExportSettings, FinalInputs, Normalize, VideoCodec, chunk_args, final_args};
use mango_core::{Composition, LayerContent, Project};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Checking inputs and drawing text, captions and graphics.
    Preparing,
    /// Rendering one piece of the script track.
    Chunk { index: usize, of: usize },
    /// Compositing, mixing and encoding the final file.
    Final,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub stage: Stage,
    /// Overall progress, `0..=1`, never decreasing.
    pub fraction: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub path: PathBuf,
    /// Duration of the written file in seconds.
    pub duration: f64,
    pub bytes: u64,
}

/// Share of the progress bar given to each stage.
const PREPARE_SHARE: f64 = 0.08;
const CHUNK_SHARE: f64 = 0.47;
/// Chunks this far outside the exported range are rendered as blanks. Longer
/// than the longest scene transition, which reaches across a scene boundary.
const RANGE_MARGIN: f64 = 6.0;

struct Reporter<'a> {
    sink: &'a mut dyn FnMut(Progress),
    last: f64,
}

impl Reporter<'_> {
    fn emit(&mut self, stage: Stage, fraction: f64) {
        let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { self.last };
        self.last = self.last.max(f);
        (self.sink)(Progress { stage, fraction: self.last });
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) { Err(Cancelled.into()) } else { Ok(()) }
}

fn require_file(path: &Path, what: &str) -> Result<()> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => Ok(()),
        Ok(_) => bail!("The file for {what} is not a regular file: {}", path.display()),
        Err(_) => bail!("The file for {what} is missing: {}. Move it back or relink it, then export again.", path.display()),
    }
}

/// Every source the export will open must exist; fail early with its name
/// instead of with an ffmpeg error halfway through.
fn check_sources(project: &Project, comp: &Composition, settings: &ExportSettings) -> Result<()> {
    let want_video = settings.video.is_some();
    let mut seen = std::collections::HashSet::new();
    for clip in comp.script_clips() {
        let Some(id) = clip.asset else { continue };
        if !seen.insert(id) {
            continue;
        }
        let asset = project.asset(id).ok_or_else(|| anyhow!("The script uses a file that is no longer in the project"))?;
        require_file(&asset.path, &format!("\"{}\"", asset.name))?;
    }
    for layer in &comp.layers {
        if layer.hidden || layer.duration <= 0.0 {
            continue;
        }
        let id = match &layer.content {
            LayerContent::Video(m) | LayerContent::Audio(m) => m.asset,
            LayerContent::Image(im) if want_video => im.asset,
            _ => continue,
        };
        let asset = project.asset(id).ok_or_else(|| anyhow!("Layer \"{}\" uses a file that is no longer in the project", layer.name))?;
        require_file(&asset.path, &format!("layer \"{}\" ({})", layer.name, asset.name))?;
    }
    Ok(())
}

/// Image formats ffmpeg's image2 demuxer can loop with `-loop 1`.
fn loopable_image(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff")
}

/// The final pass loops still images with `-loop 1`, which only the image2
/// demuxer understands. Images in other containers (GIF, HEIC, WebP …) are
/// converted to PNG in the work directory and the project copy repointed.
fn normalise_images(project: &Project, comp: &Composition, work: &Path) -> Result<Project> {
    let mut patched = project.clone();
    let mut n = 0usize;
    for layer in &comp.layers {
        let LayerContent::Image(im) = &layer.content else { continue };
        if layer.hidden || layer.duration <= 0.0 {
            continue;
        }
        let Some(asset) = patched.asset_mut(im.asset) else { continue };
        // Already loopable, or already converted for an earlier layer.
        if loopable_image(&asset.path) {
            continue;
        }
        let png = work.join(format!("image_{n:03}.png"));
        n += 1;
        crate::frames::convert_to_png(&asset.path, &png)?;
        asset.path = png;
    }
    Ok(patched)
}

/// A stand-in for a chunk nobody will see or hear: same timing, but every
/// clip is a gap (background colour and silence), which renders instantly.
fn blank_chunk(chunk: &Chunk) -> Chunk {
    let mut c = chunk.clone();
    for clip in &mut c.clips {
        let d = clip.duration();
        clip.asset = None;
        clip.src_start = 0.0;
        clip.src_end = d;
    }
    c
}

fn escape_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '=' | ';' | '#' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

/// FFMETADATA with the composition's chapters in composition time. For a
/// range export ffmpeg itself shifts and clips them to the exported range.
/// `None` when there are no chapters to write.
fn chapter_metadata(comp: &Composition) -> Option<String> {
    let chapters = mango_core::transcript_export::chapters(comp);
    if chapters.is_empty() {
        return None;
    }
    let total = comp.duration();
    let mut s = String::from(";FFMETADATA1\n");
    let mut written = 0;
    for (i, (t, name)) in chapters.iter().enumerate() {
        let end = chapters.get(i + 1).map(|c| c.0).unwrap_or(total).min(total);
        let start = t.max(0.0);
        if end - start < 0.001 {
            continue;
        }
        s.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={}\n",
            (start * 1000.0).round() as i64,
            (end * 1000.0).round() as i64,
            escape_meta(name)
        ));
        written += 1;
    }
    (written > 0).then_some(s)
}

/// Largest absolute sample (0..=1) in a 16-bit PCM WAV file, read as a stream.
fn wav_peak(path: &Path) -> Result<f64> {
    use std::io::Read;
    let mut r = std::io::BufReader::with_capacity(1 << 16, std::fs::File::open(path)?);
    let mut head = [0u8; 12];
    r.read_exact(&mut head)?;
    if &head[..4] != b"RIFF" || &head[8..] != b"WAVE" {
        bail!("not a WAV file");
    }
    // Walk the chunks to the sample data.
    loop {
        let mut ch = [0u8; 8];
        r.read_exact(&mut ch).context("WAV file has no audio data")?;
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        if &ch[..4] == b"data" {
            break;
        }
        std::io::copy(&mut r.by_ref().take(size + (size & 1)), &mut std::io::sink())?;
    }
    // Read to the end of the file: the size field overflows on very long mixes.
    let mut peak = 0i32;
    let mut buf = vec![0u8; 1 << 16];
    let mut carry: Option<u8> = None;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let mut i = 0;
        if let Some(lo) = carry.take() {
            peak = peak.max((i16::from_le_bytes([lo, buf[0]]) as i32).abs());
            i = 1;
        }
        while i + 1 < n {
            peak = peak.max((i16::from_le_bytes([buf[i], buf[i + 1]]) as i32).abs());
            i += 2;
        }
        if i < n {
            carry = Some(buf[i]);
        }
    }
    Ok(peak as f64 / 32768.0)
}

/// Whether the container can carry chapters at all.
fn supports_chapters(settings: &ExportSettings, ext: &str) -> bool {
    if settings.video == Some(VideoCodec::Gif) {
        return false;
    }
    matches!(ext, "mp4" | "m4v" | "mov" | "m4a" | "m4b" | "mkv" | "webm" | "mp3" | "opus" | "ogg" | "flac")
}

/// Removes its file when dropped unless disarmed: the partial output.
struct PartialFile {
    path: PathBuf,
    keep: bool,
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Where exports keep their intermediates by default: under the app's cache
/// directory, never in the (possibly cloud-synced) destination folder.
pub fn default_work_root() -> PathBuf {
    mango_core::tools::cache_dir().join("export-work")
}

/// Deletes work folders left behind by a crash (older than a day).
fn sweep_stale_work(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(WORK_PREFIX) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(24 * 3600));
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

const WORK_PREFIX: &str = "mango-export-";

/// Runs a full export of `comp` to `out`.
///
/// Plans the script track into chunks and renders each to a work folder,
/// rasterises text/caption/graphic overlays, writes chapter metadata when
/// asked for and available, then runs the final composite/encode.
///
/// Intermediates go to a fresh folder under [`default_work_root`], removed
/// on success, error and cancel alike. The output is written to a hidden
/// temporary name next to `out` and renamed over `out` only once complete,
/// so a failed or cancelled export never leaves a partial file at `out` (or
/// destroys a previous export there).
///
/// Returns a [`Cancelled`] error (see [`crate::is_cancelled`]) when `cancel`
/// is set during the export.
pub fn export(
    project: &Project,
    comp: &Composition,
    settings: &ExportSettings,
    out: &Path,
    on_progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Report> {
    export_with_work_root(project, comp, settings, out, &default_work_root(), on_progress, cancel)
}

/// [`export`] with an explicit parent folder for the intermediates (they can
/// be several gigabytes for a long video; pick a volume with room).
pub fn export_with_work_root(
    project: &Project,
    comp: &Composition,
    settings: &ExportSettings,
    out: &Path,
    work_root: &Path,
    on_progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Report> {
    let mut progress = Reporter { sink: on_progress, last: 0.0 };
    check_cancel(cancel)?;
    progress.emit(Stage::Preparing, 0.0);

    // --- validate -------------------------------------------------------------
    let want_video = settings.video.is_some();
    let is_gif = settings.video == Some(VideoCodec::Gif);
    let want_audio = settings.audio.is_some() && !is_gif;
    if !want_video && !want_audio {
        bail!("Nothing to export: both video and audio are turned off");
    }
    if !(settings.fps.is_finite() && settings.fps >= 1.0 && settings.fps <= 240.0) {
        bail!("The export frame rate must be between 1 and 240 fps");
    }
    let mut settings = settings.clone();
    if want_video {
        if settings.width < 2 || settings.height < 2 || settings.width > 16_384 || settings.height > 16_384 {
            bail!("The export size {}x{} is not supported", settings.width, settings.height);
        }
        // Every video encoder used here needs even dimensions.
        settings.width &= !1;
        settings.height &= !1;
    }
    let total = comp.duration();
    if !(total.is_finite() && total > 0.0) {
        bail!("There is nothing to export: the composition is empty");
    }
    if let Some((a, b)) = settings.range {
        if !(a.is_finite() && b.is_finite()) || b - a < 0.01 || a < 0.0 || a >= total {
            bail!("The export range is empty or outside the composition");
        }
        settings.range = Some((a, b.min(total)));
    }
    let out_duration = settings.range.map(|(a, b)| b - a).unwrap_or(total);
    check_sources(project, comp, &settings)?;

    let parent = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if !parent.is_dir() {
        bail!("The folder to export into does not exist: {}", parent.display());
    }
    if out.is_dir() {
        bail!("Cannot export over a folder: {}", out.display());
    }
    let ext = out.extension().map(|e| e.to_string_lossy().to_lowercase()).filter(|e| !e.is_empty()).unwrap_or_else(|| settings.extension().to_string());

    // Both are removed when dropped: on success, on any error and on cancel.
    std::fs::create_dir_all(work_root).with_context(|| format!("could not create the working folder {}", work_root.display()))?;
    sweep_stale_work(work_root);
    let work = tempfile::Builder::new()
        .prefix(WORK_PREFIX)
        .tempdir_in(work_root)
        .with_context(|| format!("could not create a working folder in {}", work_root.display()))?;
    let stem = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "export".into());
    let partial = tempfile::Builder::new()
        .prefix(&format!(".{stem}."))
        .suffix(&format!(".partial.{ext}"))
        .tempfile_in(&parent)
        .with_context(|| format!("cannot write to {}", parent.display()))?;
    // Keep only the name: ffmpeg writes the file, and the guard deletes it.
    let mut partial = PartialFile { path: partial.into_temp_path().keep().context("could not reserve the output file")?, keep: false };
    let tmp_out = partial.path.clone();

    // --- overlays ---------------------------------------------------------------
    let project = &normalise_images(project, comp, work.path())?;
    let overlays = if want_video {
        crate::raster::overlays_for_export_cancellable(project, comp, &settings, &work.path().join("overlays"), Some(cancel))?
    } else {
        Vec::new()
    };
    check_cancel(cancel)?;
    progress.emit(Stage::Preparing, PREPARE_SHARE);

    // --- stage 1: chunks ----------------------------------------------------------
    let chunks = render::plan_chunks(comp, &settings);
    let script_total: f64 = chunks.iter().map(Chunk::duration).sum::<f64>().max(1e-9);
    let mut rendered: Vec<(Chunk, PathBuf)> = Vec::with_capacity(chunks.len());
    let mut done = 0.0f64;
    let count = chunks.len();
    for chunk in chunks {
        check_cancel(cancel)?;
        let stage = Stage::Chunk { index: chunk.index, of: count };
        progress.emit(stage, PREPARE_SHARE + CHUNK_SHARE * done / script_total);
        let path = work.path().join(format!("chunk_{:05}.mkv", chunk.index));
        let unseen = match settings.range {
            // Audio before the range still feeds the loudness normaliser.
            Some((a, b)) => chunk.start >= b + RANGE_MARGIN || (chunk.end <= a - RANGE_MARGIN && settings.normalize == Normalize::Off),
            None => false,
        };
        let job = if unseen { blank_chunk(&chunk) } else { chunk.clone() };
        let args = chunk_args(project, comp, &job, &settings, &path);
        let d = chunk.duration();
        ffmpeg::run(
            &args,
            &mut |t| progress.emit(stage, PREPARE_SHARE + CHUNK_SHARE * (done + t.clamp(0.0, d)) / script_total),
            cancel,
        )
        .map_err(|e| if ffmpeg::is_cancelled(&e) { e } else { e.context(format!("Rendering part {} of {count} of the script failed", chunk.index + 1)) })?;
        done += d;
        rendered.push((chunk, path));
    }
    let after_chunks = PREPARE_SHARE + CHUNK_SHARE;
    progress.emit(Stage::Final, after_chunks);
    check_cancel(cancel)?;

    // --- chapters -------------------------------------------------------------------
    let mut metadata: Option<PathBuf> = None;
    if settings.embed_chapters && supports_chapters(&settings, &ext) {
        if let Some(text) = chapter_metadata(comp) {
            let path = work.path().join("chapters.ffmetadata");
            std::fs::write(&path, text).with_context(|| format!("could not write {}", path.display()))?;
            metadata = Some(path);
        }
    }

    // --- peak normalisation: measure the mix first ----------------------------------
    let mut final_from = after_chunks;
    let mut master_gain_db = None;
    if let (true, Normalize::Peak(target_db)) = (want_audio, settings.normalize) {
        let measure_share = (1.0 - after_chunks) * 0.15;
        let mut probe = settings.clone();
        probe.video = None;
        probe.audio = Some(AudioCodec::Wav);
        probe.normalize = Normalize::Off;
        probe.title = None;
        let wav = work.path().join("mix-measure.wav");
        let inputs = FinalInputs { chunks: &rendered, overlays: &[], metadata: None, out: &wav, master_gain_db: None };
        let args = final_args(project, comp, &probe, &inputs);
        ffmpeg::run(
            &args,
            &mut |t| progress.emit(Stage::Final, after_chunks + measure_share * (t / out_duration).clamp(0.0, 1.0)),
            cancel,
        )
        .map_err(|e| if ffmpeg::is_cancelled(&e) { e } else { e.context("Measuring the mix level failed") })?;
        let peak = wav_peak(&wav).context("could not read the measured mix")?;
        // The best-effort removal keeps disk use down; the folder goes anyway.
        let _ = std::fs::remove_file(&wav);
        if peak > 1e-5 {
            let peak_db = 20.0 * peak.log10();
            master_gain_db = Some((target_db.min(0.0) - peak_db).clamp(-60.0, 60.0));
        }
        final_from = after_chunks + measure_share;
        check_cancel(cancel)?;
    }

    // --- stage 2: final ---------------------------------------------------------------
    let inputs = FinalInputs { chunks: &rendered, overlays: &overlays, metadata: metadata.as_deref(), out: &tmp_out, master_gain_db };
    let args = final_args(project, comp, &settings, &inputs);
    ffmpeg::run(
        &args,
        &mut |t| progress.emit(Stage::Final, final_from + (1.0 - final_from) * (t / out_duration).clamp(0.0, 1.0) * 0.99),
        cancel,
    )
    .map_err(|e| if ffmpeg::is_cancelled(&e) { e } else { e.context("Encoding the final file failed") })?;
    check_cancel(cancel)?;

    let bytes = std::fs::metadata(&tmp_out).map(|m| m.len()).unwrap_or(0);
    if bytes == 0 {
        bail!("The export produced an empty file");
    }
    let duration = crate::probe::probe(&tmp_out).map(|a| a.duration).ok().filter(|d| *d > 0.0).unwrap_or(out_duration);
    std::fs::rename(&tmp_out, out).with_context(|| format!("could not move the finished export to {}", out.display()))?;
    partial.keep = true;
    work.close().context("could not remove the export's working folder")?;
    progress.emit(Stage::Final, 1.0);
    Ok(Report { path: out.to_path_buf(), duration, bytes })
}
