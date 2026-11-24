//! Small helpers for the child processes this crate runs (ffmpeg, ffprobe, say).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

/// Locates an external tool or explains what is missing.
pub(crate) fn tool(name: &str) -> Result<PathBuf> {
    mango_core::tools::find_tool(name).ok_or_else(|| {
        anyhow!("{name} was not found. Install it (for example `brew install ffmpeg`) or set MANGO_{} to its path.", name.to_uppercase())
    })
}

/// Reads a child's stderr to the end on a background thread so the child can
/// never block on a full pipe. Keeps only the tail.
pub(crate) fn drain_stderr(stderr: ChildStderr) -> JoinHandle<String> {
    std::thread::spawn(move || {
        const KEEP: usize = 16 * 1024;
        let mut stderr = stderr;
        let mut tail: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > KEEP * 2 {
                        let cut = tail.len() - KEEP;
                        tail.drain(..cut);
                    }
                }
            }
        }
        String::from_utf8_lossy(&tail).into_owned()
    })
}

/// The last `n` non-empty lines of a tool's stderr, for error messages.
/// ffmpeg redraws its status line with carriage returns; those count as line breaks.
pub(crate) fn last_lines(stderr: &str, n: usize) -> String {
    let lines: Vec<&str> = stderr.split(['\n', '\r']).map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(n);
    lines[from..].join("\n")
}

/// Kills and reaps the child when dropped, unless it has already exited.
pub(crate) struct ChildGuard(pub Child);

impl ChildGuard {
    /// Waits up to `timeout` for the child to exit on its own.
    pub(crate) fn wait_timeout(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.0.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) => {}
                Err(_) => return None,
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

/// Runs a short-lived command to completion and returns (success, stdout, stderr).
pub(crate) fn run_capture(cmd: &mut Command) -> Result<(bool, String, String)> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("could not run {program}"))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Duration of a media file in seconds, via ffprobe.
pub(crate) fn probe_duration(path: &std::path::Path) -> Result<f64> {
    let ffprobe = tool("ffprobe")?;
    let (ok, stdout, stderr) = run_capture(
        Command::new(ffprobe)
            .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
            .arg(path),
    )?;
    if !ok {
        return Err(anyhow!("ffprobe could not read {}: {}", path.display(), last_lines(&stderr, 3)));
    }
    stdout
        .trim()
        .parse::<f64>()
        .map_err(|_| anyhow!("ffprobe reported no duration for {}", path.display()))
}
