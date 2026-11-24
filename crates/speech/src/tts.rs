//! Text-to-speech through the system synthesiser (macOS `say`).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};

use crate::proc::{ChildGuard, drain_stderr, last_lines, run_capture};

const SAY: &str = "/usr/bin/say";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voice {
    /// What to pass as `voice` to [`synthesize`].
    pub id: String,
    /// Display name.
    pub name: String,
    /// BCP 47 style tag, e.g. `en-US`.
    pub language: String,
}

fn say_path() -> Result<PathBuf> {
    let p = PathBuf::from(SAY);
    if p.is_file() {
        Ok(p)
    } else {
        bail!("the system speech synthesiser ({SAY}) is not available on this computer")
    }
}

/// Voices from the system speech synthesiser.
pub fn voices() -> Result<Vec<Voice>> {
    let (ok, stdout, stderr) = run_capture(Command::new(say_path()?).args(["-v", "?"]))?;
    if !ok {
        bail!("could not list the system voices: {}", last_lines(&stderr, 3));
    }
    Ok(parse_voices(&stdout))
}

/// Parses the output of `say -v '?'`: `Name   lo_CALE    # sample sentence` per line. Names
/// may contain spaces and parentheses ("Eddy (English (US))").
pub fn parse_voices(listing: &str) -> Vec<Voice> {
    let mut out: Vec<Voice> = Vec::new();
    for line in listing.lines() {
        let head = line.split('#').next().unwrap_or("").trim_end();
        let Some(split) = head.rfind(char::is_whitespace) else { continue };
        let (name, locale) = (head[..split].trim(), head[split..].trim());
        // "en_US", "ar_001", "zh_CN", "es_419", "yue" ...
        let mut parts = locale.split(['_', '-']);
        let language_ok = parts.next().is_some_and(|l| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_lowercase()));
        let locale_ok = language_ok && parts.all(|p| (2..=4).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()));
        if name.is_empty() || !locale_ok {
            continue;
        }
        if out.iter().any(|v| v.id == name) {
            continue;
        }
        out.push(Voice { id: name.to_string(), name: name.to_string(), language: locale.replace('_', "-") });
    }
    out
}

/// Synthesises `text` to a 48 kHz 16-bit mono WAV file at `out` and returns its duration in
/// seconds. `voice` is a [`Voice::id`] (`None` = the system default voice); `rate_wpm` is the
/// speaking rate in words per minute (`None` = the voice's default). Empty text, an unknown
/// voice or an unusable rate are errors, and no file is left at `out` when synthesis fails.
pub fn synthesize(text: &str, voice: Option<&str>, rate_wpm: Option<u32>, out: &Path) -> Result<f64> {
    let text = text.trim();
    if text.is_empty() {
        bail!("there is no text to speak");
    }
    if let Some(rate) = rate_wpm {
        if !(40..=720).contains(&rate) {
            bail!("speaking rate {rate} words per minute is out of range (40 to 720)");
        }
    }
    // `say` silently falls back to the default voice when it does not know the name.
    let voice = voice.map(str::trim).filter(|v| !v.is_empty());
    if let Some(v) = voice {
        let known = voices()?;
        if !known.iter().any(|k| k.id == v) {
            bail!("the voice \"{v}\" is not installed on this computer");
        }
    }
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;
    }
    // Write next to the destination and rename, so a failure never leaves a broken file.
    let mut tmp_name = out.file_name().ok_or_else(|| anyhow!("{} is not a file path", out.display()))?.to_os_string();
    tmp_name.push(format!(".{}.tmp.wav", std::process::id()));
    let tmp = out.with_file_name(tmp_name);
    let _ = std::fs::remove_file(&tmp);

    let mut cmd = Command::new(say_path()?);
    if let Some(v) = voice {
        cmd.arg("-v").arg(v);
    }
    if let Some(rate) = rate_wpm {
        cmd.arg("-r").arg(rate.to_string());
    }
    // The text goes in on stdin so that nothing in it can be read as an option.
    cmd.args(["--file-format=WAVE", "--data-format=LEI16@48000", "--channels=1", "-o"])
        .arg(&tmp)
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = ChildGuard(cmd.spawn().context("could not start the system speech synthesiser")?);
    let stderr = drain_stderr(child.0.stderr.take().expect("stderr was piped"));
    {
        let mut stdin = child.0.stdin.take().expect("stdin was piped");
        // A broken pipe here means `say` already failed; its exit status tells the story.
        let _ = stdin.write_all(text.as_bytes());
    }
    let status = child.0.wait().context("waiting for the system speech synthesiser")?;
    let stderr = stderr.join().unwrap_or_default();
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("speech synthesis failed: {}", last_lines(&stderr, 3));
    }
    let duration = match wav_duration(&tmp) {
        Ok(d) if d > 0.0 => d,
        Ok(_) => {
            let _ = std::fs::remove_file(&tmp);
            bail!("the synthesiser produced no audio for this text");
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.context("the synthesiser did not produce a readable WAV file"));
        }
    };
    std::fs::rename(&tmp, out).with_context(|| format!("could not write {}", out.display()))?;
    Ok(duration)
}

/// Duration of a PCM WAV file from its header.
fn wav_duration(path: &Path) -> Result<f64> {
    let mut f = std::fs::File::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let file_len = f.metadata()?.len();
    let mut riff = [0u8; 12];
    f.read_exact(&mut riff)?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let mut byte_rate: Option<u32> = None;
    let mut pos = 12u64;
    loop {
        let mut head = [0u8; 8];
        f.seek(SeekFrom::Start(pos))?;
        if f.read_exact(&mut head).is_err() {
            bail!("WAV file has no audio data");
        }
        let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as u64;
        let body = pos + 8;
        match &head[0..4] {
            b"fmt " => {
                let mut fmt = [0u8; 16];
                f.read_exact(&mut fmt)?;
                byte_rate = Some(u32::from_le_bytes([fmt[8], fmt[9], fmt[10], fmt[11]]));
            }
            b"data" => {
                let rate = byte_rate.filter(|r| *r > 0).ok_or_else(|| anyhow!("WAV file has no format"))?;
                // Streamed writers may leave the size as 0 or 0xFFFFFFFF; trust the file length then.
                let available = file_len.saturating_sub(body);
                let len = if size == 0 || size == 0xFFFF_FFFF || size > available { available } else { size };
                return Ok(len as f64 / rate as f64);
            }
            _ => {}
        }
        pos = body + size + (size & 1);
    }
}
