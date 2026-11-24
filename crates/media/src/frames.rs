//! Still frames from media files.

use crate::ffmpeg::{ffmpeg_path, run_capture, run_capture_raw};
use anyhow::{Context, Result, anyhow, bail};
use std::path::Path;

/// A straight-alpha (non-premultiplied) RGBA8 image, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// A fully transparent frame.
    pub fn transparent(width: u32, height: u32) -> Self {
        Self { width, height, rgba: vec![0; width as usize * height as usize * 4] }
    }
    /// The pixel at `(x, y)` as `[r, g, b, a]`; transparent black outside.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        match self.rgba.get(i..i + 4) {
            Some(p) => [p[0], p[1], p[2], p[3]],
            None => [0; 4],
        }
    }
    pub fn is_fully_transparent(&self) -> bool {
        self.rgba.chunks_exact(4).all(|p| p[3] == 0)
    }
}

fn scale_filter(max_w: u32, max_h: u32) -> String {
    // Square pixels first, then shrink (never enlarge) to fit the box.
    format!(
        "scale='max(1,trunc(iw*sar))':ih,setsar=1,scale=w='min(iw,{mw})':h='min(ih,{mh})':force_original_aspect_ratio=decrease:flags=bicubic",
        mw = max_w.max(1),
        mh = max_h.max(1)
    )
}

/// Parses a stream of PAM images (ffmpeg `-c:v pam`, RGBA) and returns the
/// last complete one.
fn last_pam(data: &[u8]) -> Result<Option<Frame>> {
    let mut pos = 0usize;
    let mut last = None;
    while pos < data.len() {
        let rest = &data[pos..];
        if !rest.starts_with(b"P7\n") {
            bail!("unexpected image data from ffmpeg");
        }
        let Some(hdr_len) = rest.windows(7).position(|w| w == b"ENDHDR\n").map(|i| i + 7) else { break };
        let header = std::str::from_utf8(&rest[..hdr_len]).context("bad image header from ffmpeg")?;
        let (mut w, mut h, mut depth, mut maxval) = (0usize, 0usize, 0usize, 0usize);
        for line in header.lines() {
            let mut it = line.split_whitespace();
            match (it.next(), it.next().and_then(|v| v.parse::<usize>().ok())) {
                (Some("WIDTH"), Some(v)) => w = v,
                (Some("HEIGHT"), Some(v)) => h = v,
                (Some("DEPTH"), Some(v)) => depth = v,
                (Some("MAXVAL"), Some(v)) => maxval = v,
                _ => {}
            }
        }
        if w == 0 || h == 0 || depth != 4 || maxval != 255 {
            bail!("unexpected image format from ffmpeg ({w}x{h}, depth {depth}, max {maxval})");
        }
        let size = w.checked_mul(h).and_then(|n| n.checked_mul(4)).ok_or_else(|| anyhow!("image too large"))?;
        if rest.len() < hdr_len + size {
            break; // truncated final image
        }
        last = Some(Frame { width: w as u32, height: h as u32, rgba: rest[hdr_len..hdr_len + size].to_vec() });
        pos += hdr_len + size;
    }
    Ok(last)
}

/// How far before the wanted time decoding starts, so the frame that is on
/// screen at that time (whose timestamp is earlier) is not skipped.
const LOOKBACK: f64 = 1.0;

/// Runs ffmpeg and returns the last image it produced (if any) and, when it
/// failed without producing one, the reason.
fn grab(path: &Path, pre_input: &[String], pre_filter: &str, max_w: u32, max_h: u32, single: bool) -> Result<(Option<Frame>, String)> {
    let program = ffmpeg_path()?;
    let mut args: Vec<String> = ["-hide_banner", "-v", "error", "-nostdin"].iter().map(|s| s.to_string()).collect();
    args.extend(pre_input.iter().cloned());
    args.extend(["-i".to_string(), path.to_string_lossy().into_owned()]);
    args.extend(["-map", "0:v:0", "-an", "-sn", "-dn"].iter().map(|s| s.to_string()));
    args.extend(["-vf".to_string(), format!("{pre_filter}{}", scale_filter(max_w, max_h))]);
    if single {
        args.extend(["-frames:v".to_string(), "1".to_string()]);
    }
    args.extend(["-fps_mode", "passthrough", "-pix_fmt", "rgba", "-c:v", "pam", "-f", "image2pipe", "pipe:1"].iter().map(|s| s.to_string()));
    let (status, out, err) = run_capture_raw(&program, &args)?;
    let frame = last_pam(&out)?;
    if frame.is_none() && !status.success() {
        return Ok((None, crate::ffmpeg::error_tail(&err)));
    }
    Ok((frame, String::new()))
}

/// The frame on screen at `time` seconds, scaled down to fit within `max_w`
/// x `max_h` keeping aspect (never enlarged), with square pixels and any
/// rotation applied. For still images, the image itself. A time past the end
/// returns the last frame.
pub fn frame_at(path: &Path, time: f64, max_w: u32, max_h: u32) -> Result<Frame> {
    if max_w == 0 || max_h == 0 {
        bail!("frame size must not be zero");
    }
    if !path.is_file() {
        bail!("File not found: {}", path.display());
    }
    let t = if time.is_finite() { time.max(0.0) } else { 0.0 };
    let mut why = String::new();
    if t > 0.0 {
        // Start decoding a little early, move `t` to zero, and let the fps
        // filter pick the frame current at zero (1 ms resolution).
        let lead = LOOKBACK.min(t);
        let seek = vec!["-ss".to_string(), format!("{:.6}", t - lead)];
        let pick = format!("setpts=PTS-{lead:.6}/TB,fps=1000:start_time=0,");
        let (frame, err) = grab(path, &seek, &pick, max_w, max_h, true)?;
        if let Some(f) = frame {
            return Ok(f);
        }
        why = err;
        // Past the end: take the last frame of the final second.
        let (frame, _) = grab(path, &["-sseof".to_string(), "-1".to_string()], "", max_w, max_h, false)?;
        if let Some(f) = frame {
            return Ok(f);
        }
    }
    let (frame, err) = grab(path, &[], "", max_w, max_h, true)?;
    if let Some(f) = frame {
        return Ok(f);
    }
    if !err.is_empty() {
        why = err;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if why.is_empty() {
        bail!("{name} has no picture to show");
    }
    bail!("could not read a frame from {name}:\n{why}")
}

/// Writes a frame as a PNG.
pub fn save_png(frame: &Frame, out: &Path) -> Result<()> {
    let bytes = encode_png(frame)?;
    std::fs::write(out, bytes).with_context(|| format!("could not write {}", out.display()))
}

/// Encodes a frame as PNG bytes (fast compression; these are intermediates).
pub fn encode_png(frame: &Frame) -> Result<Vec<u8>> {
    let expected = frame.width as usize * frame.height as usize * 4;
    if frame.width == 0 || frame.height == 0 || frame.rgba.len() != expected {
        bail!("invalid frame: {}x{} with {} bytes", frame.width, frame.height, frame.rgba.len());
    }
    let mut bytes = Vec::with_capacity(expected / 8 + 1024);
    {
        let mut enc = png::Encoder::new(&mut bytes, frame.width, frame.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().context("encoding PNG")?;
        writer.write_image_data(&frame.rgba).context("encoding PNG")?;
        writer.finish().context("encoding PNG")?;
    }
    Ok(bytes)
}

/// Decodes any image ffmpeg can read into a PNG file (used to normalise
/// image formats ffmpeg cannot loop directly).
pub(crate) fn convert_to_png(src: &Path, out: &Path) -> Result<()> {
    let program = ffmpeg_path()?;
    let args: Vec<String> = vec![
        "-hide_banner".into(),
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        src.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-frames:v".into(),
        "1".into(),
        "-c:v".into(),
        "png".into(),
        "-f".into(),
        "image2".into(),
        "-update".into(),
        "1".into(),
        out.to_string_lossy().into_owned(),
    ];
    run_capture(&program, &args).with_context(|| format!("could not read image {}", src.display()))?;
    Ok(())
}
