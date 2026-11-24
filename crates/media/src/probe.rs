//! Probing media files with ffprobe.

use crate::ffmpeg::{error_tail, ffprobe_path, run_capture_raw};
use anyhow::{Context, Result, anyhow, bail};
use mango_core::{AssetId, MediaAsset, MediaKind};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize, Default)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    #[serde(default)]
    format: Option<ProbeFormat>,
}

#[derive(Deserialize, Default)]
struct ProbeFormat {
    #[serde(default)]
    format_name: Option<String>,
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Deserialize, Default)]
struct ProbeStream {
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    codec_name: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
    #[serde(default)]
    r_frame_rate: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    nb_frames: Option<String>,
    #[serde(default)]
    sample_rate: Option<String>,
    #[serde(default)]
    channels: Option<u32>,
    #[serde(default)]
    sample_aspect_ratio: Option<String>,
    #[serde(default)]
    disposition: Option<Disposition>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
    #[serde(default)]
    tags: Option<StreamTags>,
}

#[derive(Deserialize, Default)]
struct Disposition {
    #[serde(default)]
    attached_pic: i32,
}

#[derive(Deserialize, Default)]
struct SideData {
    #[serde(default)]
    rotation: Option<f64>,
}

#[derive(Deserialize, Default)]
struct StreamTags {
    /// Older files carry rotation as a tag instead of a display matrix.
    #[serde(default)]
    rotate: Option<String>,
}

fn secs(s: &Option<String>) -> Option<f64> {
    s.as_deref().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0)
}

fn ratio(s: &Option<String>) -> Option<f64> {
    let s = s.as_deref()?;
    let (a, b) = s.split_once(['/', ':'])?;
    let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
    if a > 0.0 && b > 0.0 { Some(a / b) } else { None }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Reads a file's properties into a new [`MediaAsset`].
///
/// Width and height are *display* dimensions (rotation and pixel aspect
/// applied). Cover art in audio files is not treated as video. Still images
/// have duration 0.
pub fn probe(path: &Path) -> Result<MediaAsset> {
    let name = file_name(path);
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!("File not found: {}", path.display()),
        Err(e) => bail!("Cannot open {}: {e}", path.display()),
    };
    if meta.is_dir() {
        bail!("{} is a folder, not a media file", path.display());
    }
    if let Err(e) = std::fs::File::open(path) {
        bail!("Cannot read {}: {e}", path.display());
    }
    if meta.len() == 0 {
        bail!("{name} is empty");
    }
    let ffprobe = ffprobe_path()?;
    let args: Vec<String> = ["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]
        .iter()
        .map(|s| s.to_string())
        .chain([path.to_string_lossy().into_owned()])
        .collect();
    let (status, out, err) = run_capture_raw(&ffprobe, &args)?;
    if !status.success() {
        let why = error_tail(&err);
        let why = why.lines().last().unwrap_or("unknown format").trim().to_string();
        // ffprobe prefixes its message with the path; keep only the reason.
        let reason = why.rsplit(": ").next().unwrap_or(&why).to_string();
        bail!("{name} is not a media file Mango Loco can open ({reason})");
    }
    let parsed: ProbeOutput = serde_json::from_slice(&out).with_context(|| format!("could not understand ffprobe's description of {name}"))?;
    let format = parsed.format.unwrap_or_default();
    let format_name = format.format_name.clone().unwrap_or_default();
    // Plain text is "decodable" as terminal art; that is never what the user meant.
    if format_name == "tty" || parsed.streams.iter().any(|s| s.codec_name.as_deref() == Some("ansi")) {
        bail!("{name} is not a media file Mango Loco can open (it looks like a text file)");
    }

    let audio = parsed.streams.iter().find(|s| s.codec_type.as_deref() == Some("audio") && s.channels.unwrap_or(0) > 0);
    let video = parsed.streams.iter().find(|s| {
        s.codec_type.as_deref() == Some("video")
            && s.disposition.as_ref().map(|d| d.attached_pic).unwrap_or(0) == 0
            && s.width.unwrap_or(0) > 0
            && s.height.unwrap_or(0) > 0
    });
    if audio.is_none() && video.is_none() {
        bail!("{name} has no audio or video Mango Loco can use");
    }

    let format_duration = secs(&format.duration);
    let (mut width, mut height, mut fps) = (0u32, 0u32, 0.0f64);
    let mut is_still = false;
    if let Some(v) = video {
        let (mut w, h) = (v.width.unwrap_or(0), v.height.unwrap_or(0));
        // Anamorphic pixels: widen to the display width.
        if let Some(sar) = ratio(&v.sample_aspect_ratio) {
            if (sar - 1.0).abs() > 1e-3 {
                w = ((w as f64 * sar).round() as u32).max(1);
            }
        }
        let rotation = v
            .side_data_list
            .iter()
            .find_map(|d| d.rotation)
            .or_else(|| v.tags.as_ref().and_then(|t| t.rotate.as_deref()).and_then(|r| r.trim().parse::<f64>().ok()))
            .unwrap_or(0.0);
        let quarter_turns = (rotation / 90.0).round() as i64;
        if quarter_turns.rem_euclid(2) == 1 {
            (width, height) = (h, w);
        } else {
            (width, height) = (w, h);
        }
        fps = ratio(&v.avg_frame_rate).or_else(|| ratio(&v.r_frame_rate)).unwrap_or(0.0);
        let single_frame = v.nb_frames.as_deref().and_then(|n| n.trim().parse::<u64>().ok()) == Some(1);
        let image_container = format_name == "image2" || format_name.ends_with("_pipe");
        let no_duration = format_duration.is_none() && secs(&v.duration).is_none();
        is_still = audio.is_none() && (image_container || single_frame || no_duration);
    }

    let (kind, duration) = if is_still {
        fps = 0.0;
        (MediaKind::Image, 0.0)
    } else {
        let d = format_duration
            .or_else(|| video.and_then(|v| secs(&v.duration)))
            .or_else(|| audio.and_then(|a| secs(&a.duration)))
            .ok_or_else(|| anyhow!("{name} does not report a duration; it may be damaged or still downloading"))?;
        (if video.is_some() { MediaKind::Video } else { MediaKind::Audio }, d)
    };

    Ok(MediaAsset {
        id: AssetId::new(),
        name,
        path: path.to_path_buf(),
        kind,
        duration,
        width,
        height,
        fps,
        has_audio: audio.is_some() && !is_still,
        has_video: video.is_some(),
        sample_rate: audio.and_then(|a| a.sample_rate.as_deref()).and_then(|r| r.trim().parse::<u32>().ok()).unwrap_or(0),
        channels: audio.and_then(|a| a.channels).unwrap_or(0),
        folder: None,
        transcript: None,
    })
}
