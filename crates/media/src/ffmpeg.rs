//! Locating and running ffmpeg / ffprobe.

use anyhow::{Context, Result, anyhow, bail};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// How much of ffmpeg's stderr is kept for error messages.
const STDERR_TAIL: usize = 16 * 1024;

/// Returned (inside `anyhow::Error`) when a run was stopped through its
/// cancel flag. Test with [`is_cancelled`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Whether an error (anywhere in its chain) is a cancellation.
pub fn is_cancelled(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.downcast_ref::<Cancelled>().is_some())
}

fn tool(name: &str) -> Result<PathBuf> {
    mango_core::tools::find_tool(name).ok_or_else(|| {
        anyhow!(
            "{name} was not found. Install FFmpeg (for example with `brew install ffmpeg`) or set MANGO_{} to its full path.",
            name.to_uppercase()
        )
    })
}

pub fn ffmpeg_path() -> Result<PathBuf> {
    tool("ffmpeg")
}

pub fn ffprobe_path() -> Result<PathBuf> {
    tool("ffprobe")
}

/// Kills and reaps the child if it is still running when dropped, so no
/// error path can leak a running ffmpeg.
pub(crate) struct ChildGuard(pub Child);

impl ChildGuard {
    pub(crate) fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            self.kill();
        }
    }
}

/// Reads a stream to its end on a thread, keeping only the last `keep` bytes.
pub(crate) fn drain_tail<R: Read + Send + 'static>(mut r: R, keep: usize) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tail.extend_from_slice(&buf[..n]);
                    if tail.len() > keep * 2 {
                        let cut = tail.len() - keep;
                        tail.drain(..cut);
                    }
                }
            }
        }
        if tail.len() > keep {
            let cut = tail.len() - keep;
            tail.drain(..cut);
        }
        String::from_utf8_lossy(&tail).into_owned()
    })
}

/// The useful end of an ffmpeg stderr dump: the last few non-empty lines.
pub(crate) fn error_tail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let from = lines.len().saturating_sub(12);
    lines[from..].join("\n")
}

pub(crate) fn describe_exit(status: ExitStatus) -> String {
    match status.code() {
        Some(c) => format!("exit code {c}"),
        None => "killed by a signal".to_string(),
    }
}

/// Runs a tool to completion, capturing all of stdout. Used for short
/// decode-to-pipe jobs (ffprobe, single frames, PCM). Fails with the tail of
/// stderr on a non-zero exit.
pub(crate) fn run_capture(program: &std::path::Path, args: &[String]) -> Result<Vec<u8>> {
    let (status, out, err) = run_capture_raw(program, args)?;
    if !status.success() {
        bail!("{} failed ({}):\n{}", program.file_name().unwrap_or_default().to_string_lossy(), describe_exit(status), error_tail(&err));
    }
    Ok(out)
}

/// Like [`run_capture`] but returns the status and stderr instead of failing.
pub(crate) fn run_capture_raw(program: &std::path::Path, args: &[String]) -> Result<(ExitStatus, Vec<u8>, String)> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not start {}", program.display()))?;
    let mut child = ChildGuard(child);
    let stderr = child.0.stderr.take().ok_or_else(|| anyhow!("no stderr pipe"))?;
    let mut stdout = child.0.stdout.take().ok_or_else(|| anyhow!("no stdout pipe"))?;
    let err_thread = drain_tail(stderr, STDERR_TAIL);
    let mut out = Vec::new();
    stdout.read_to_end(&mut out).context("reading tool output")?;
    let status = child.0.wait().context("waiting for tool")?;
    let err = err_thread.join().unwrap_or_default();
    Ok((status, out, err))
}

/// Runs ffmpeg with `args`, reporting the output timestamp (seconds) as it
/// advances. `-progress pipe:1 -nostats` is added unless the arguments
/// already ask for progress. Returns an error carrying the end of ffmpeg's
/// stderr on a non-zero exit, and a [`Cancelled`] error (after killing the
/// process) when `cancel` becomes true.
pub fn run(args: &[String], on_time: &mut dyn FnMut(f64), cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        return Err(Cancelled.into());
    }
    let program = ffmpeg_path()?;
    let mut full: Vec<String> = Vec::with_capacity(args.len() + 6);
    if !args.iter().any(|a| a == "-progress") {
        full.extend(["-progress", "pipe:1", "-nostats", "-stats_period", "0.25"].map(String::from));
    }
    full.extend(args.iter().cloned());
    if std::env::var_os("MANGO_FFMPEG_LOG").is_some() {
        // Diagnostic aid: MANGO_FFMPEG_LOG=1 prints every ffmpeg command line.
        eprintln!("[mango-media] ffmpeg {}", full.iter().map(|a| format!("{a:?}")).collect::<Vec<_>>().join(" "));
    }

    let child = Command::new(&program)
        .args(&full)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("could not start {}", program.display()))?;
    let mut child = ChildGuard(child);
    let stderr = child.0.stderr.take().ok_or_else(|| anyhow!("no stderr pipe"))?;
    let stdout = child.0.stdout.take().ok_or_else(|| anyhow!("no stdout pipe"))?;
    let err_thread = drain_tail(stderr, STDERR_TAIL);

    // Progress lines are parsed on a thread so the main loop can poll the
    // cancel flag even while ffmpeg is silent.
    let (tx, rx) = mpsc::channel::<f64>();
    let out_thread = std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(t) = parse_progress_line(&line) {
                if tx.send(t).is_err() {
                    break;
                }
            }
        }
    });

    let mut cancelled = false;
    loop {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            child.kill();
            break;
        }
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(t) => on_time(t),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // stdout closed: the process is finishing.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = out_thread.join();
    if cancelled {
        let _ = err_thread.join();
        return Err(Cancelled.into());
    }
    // ffmpeg may close stdout slightly before exiting; keep honouring cancel.
    let status = loop {
        match child.0.try_wait().context("waiting for ffmpeg")? {
            Some(s) => break s,
            None => {
                if cancel.load(Ordering::SeqCst) {
                    child.kill();
                    let _ = err_thread.join();
                    return Err(Cancelled.into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    let err = err_thread.join().unwrap_or_default();
    if !status.success() {
        bail!("ffmpeg failed ({}):\n{}", describe_exit(status), error_tail(&err));
    }
    Ok(())
}

/// Extracts the output time in seconds from one `-progress` line.
fn parse_progress_line(line: &str) -> Option<f64> {
    let (key, value) = line.trim().split_once('=')?;
    match key {
        // Both are microseconds despite the name of the second.
        "out_time_us" | "out_time_ms" => value.trim().parse::<i64>().ok().filter(|v| *v >= 0).map(|v| v as f64 / 1e6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress_line("out_time_us=1500000"), Some(1.5));
        assert_eq!(parse_progress_line("out_time_ms=250000"), Some(0.25));
        assert_eq!(parse_progress_line("out_time_us=N/A"), None);
        assert_eq!(parse_progress_line("out_time=00:00:01.500000"), None);
        assert_eq!(parse_progress_line("progress=end"), None);
    }

    #[test]
    fn error_tail_keeps_last_lines() {
        let s = (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let t = error_tail(&s);
        assert!(t.ends_with("line 39"));
        assert!(!t.contains("line 5\n"));
    }
}
