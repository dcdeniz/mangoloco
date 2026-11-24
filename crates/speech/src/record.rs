//! Screen, camera and microphone capture through ffmpeg's avfoundation input.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::proc::{ChildGuard, last_lines, run_capture, tool};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// avfoundation device index within its list (video and audio are numbered separately).
    pub index: u32,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Devices {
    pub screens: Vec<Device>,
    pub cameras: Vec<Device>,
    pub microphones: Vec<Device>,
}

/// Enumerates capture devices.
pub fn devices() -> Result<Devices> {
    let ffmpeg = tool("ffmpeg")?;
    // ffmpeg always exits non-zero here (there is no input to open); the list is on stderr.
    let (_, _, stderr) = run_capture(Command::new(ffmpeg).args([
        "-hide_banner",
        "-f",
        "avfoundation",
        "-list_devices",
        "true",
        "-i",
        "",
    ]))?;
    if !stderr.contains("AVFoundation video devices") && !stderr.contains("AVFoundation audio devices") {
        bail!("ffmpeg could not list capture devices: {}", last_lines(&stderr, 4));
    }
    Ok(parse_device_list(&stderr))
}

/// Parses the stderr of `ffmpeg -f avfoundation -list_devices true -i ""`.
pub fn parse_device_list(stderr: &str) -> Devices {
    #[derive(PartialEq)]
    enum Section {
        None,
        Video,
        Audio,
    }
    let mut section = Section::None;
    let mut out = Devices::default();
    for line in stderr.lines() {
        // Lines look like "[AVFoundation indev @ 0x...] [1] Capture screen 0".
        let body = match line.trim_start().strip_prefix('[') {
            Some(rest) => match rest.find("] ") {
                Some(i) => rest[i + 2..].trim(),
                None => continue,
            },
            None => continue,
        };
        if body.starts_with("AVFoundation video devices") {
            section = Section::Video;
            continue;
        }
        if body.starts_with("AVFoundation audio devices") {
            section = Section::Audio;
            continue;
        }
        let Some(rest) = body.strip_prefix('[') else { continue };
        let Some(close) = rest.find(']') else { continue };
        let Ok(index) = rest[..close].trim().parse::<u32>() else { continue };
        let name = rest[close + 1..].trim().to_string();
        if name.is_empty() {
            continue;
        }
        let device = Device { index, name };
        match section {
            Section::Video if device.name.starts_with("Capture screen") => out.screens.push(device),
            Section::Video => out.cameras.push(device),
            Section::Audio => out.microphones.push(device),
            Section::None => {}
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Index from [`Devices::screens`].
    pub screen: Option<u32>,
    /// Index from [`Devices::cameras`].
    pub camera: Option<u32>,
    /// Index from [`Devices::microphones`].
    pub microphone: Option<u32>,
    /// Frame rate of the recorded files (constant).
    pub fps: u32,
    pub capture_cursor: bool,
    pub out_dir: PathBuf,
    /// Files are named `<base_name>-screen.mp4`, `<base_name>-camera.mp4`, `<base_name>-mic.m4a`.
    pub base_name: String,
}

/// A recording in progress. Dropping it without calling [`Recorder::stop`] kills ffmpeg.
pub struct Recorder {
    child: ChildGuard,
    stdin: Option<ChildStdin>,
    stderr: Arc<Mutex<Vec<u8>>>,
    files: Vec<PathBuf>,
    started: Instant,
}

const PERMISSION_HINT: &str = "ffmpeg stopped before the recording could run. The usual cause is that macOS has not granted \
Screen Recording, Camera or Microphone permission to Mango Loco: open System Settings > Privacy & Security, allow it under \
Screen & System Audio Recording, Camera and Microphone, then restart the app. It can also mean the device is in use or was unplugged.";

fn early_exit_error(stderr: &str) -> anyhow::Error {
    let tail = last_lines(stderr, 8);
    if tail.is_empty() {
        anyhow!("{PERMISSION_HINT}\nffmpeg gave no further details.")
    } else {
        anyhow!("{PERMISSION_HINT}\nffmpeg said:\n{tail}")
    }
}

fn no_data_error(stderr: &str) -> anyhow::Error {
    let tail = last_lines(stderr, 8);
    anyhow!(
        "No picture or sound arrived from the capture device, so nothing was recorded. The usual cause is that macOS has not \
granted Screen Recording, Camera or Microphone permission to Mango Loco: open System Settings > Privacy & Security, allow it \
under Screen & System Audio Recording, Camera and Microphone, then restart the app.\nffmpeg said:\n{}",
        if tail.is_empty() { "(nothing)" } else { tail.as_str() }
    )
}

/// How long `start` waits for the first data before giving up.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(6);
/// A file larger than this holds captured data, not just container headers.
const LIVE_BYTES: u64 = 1024;

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn stderr_text(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    let guard = buf.lock().unwrap_or_else(|p| p.into_inner());
    String::from_utf8_lossy(&guard).into_owned()
}

impl Recorder {
    /// Starts recording. Each selected video source is written to its own file
    /// (`-screen.mp4`, `-camera.mp4`), each with the microphone muxed in when one is selected,
    /// so the editor can place them on separate tracks; a microphone alone gives `-mic.m4a`.
    /// Output is constant frame rate H.264 + AAC.
    ///
    /// Returns an error in plain words (including ffmpeg's last output) when ffmpeg exits
    /// straight away, which is what happens when macOS has not granted capture permission.
    pub fn start(opts: &Options) -> Result<Recorder> {
        if opts.screen.is_none() && opts.camera.is_none() && opts.microphone.is_none() {
            bail!("nothing to record: choose a screen, a camera or a microphone");
        }
        if !(1..=120).contains(&opts.fps) {
            bail!("frame rate {} is out of range (1 to 120)", opts.fps);
        }
        let base = opts.base_name.trim();
        if base.is_empty() || base.contains(['/', '\\', ':', '\0']) || base.starts_with('.') {
            bail!("\"{}\" cannot be used as a recording name", opts.base_name);
        }
        std::fs::create_dir_all(&opts.out_dir)
            .with_context(|| format!("could not create the recording folder {}", opts.out_dir.display()))?;
        let ffmpeg = tool("ffmpeg")?;

        let mut cmd = Command::new(&ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "warning", "-nostats"]);
        let mut next_input = 0usize;
        let mut add_video_input = |cmd: &mut Command, device: u32, framerate: u32, cursor: Option<bool>| {
            cmd.args(["-thread_queue_size", "1024", "-f", "avfoundation"]);
            cmd.args(["-framerate", &framerate.to_string(), "-pixel_format", "nv12"]);
            if let Some(cursor) = cursor {
                cmd.args(["-capture_cursor", if cursor { "1" } else { "0" }]);
            }
            cmd.args(["-i", &format!("{device}:none")]);
            let index = next_input;
            next_input += 1;
            index
        };
        let screen_input = opts.screen.map(|d| add_video_input(&mut cmd, d, opts.fps, Some(opts.capture_cursor)));
        // Cameras only accept the rates of their native modes; capture at 30 and let the
        // output stage produce the requested constant rate.
        let camera_input = opts.camera.map(|d| add_video_input(&mut cmd, d, 30, None));
        let mic_input = opts.microphone.map(|d| {
            cmd.args(["-thread_queue_size", "1024", "-f", "avfoundation", "-i", &format!("none:{d}")]);
            let index = next_input;
            next_input += 1;
            index
        });

        let mut files: Vec<PathBuf> = Vec::new();
        let mut add_output = |cmd: &mut Command, video: Option<usize>, suffix: &str| -> Result<()> {
            let path = opts.out_dir.join(format!("{base}-{suffix}"));
            if path.exists() {
                bail!("{} already exists; choose another recording name", path.display());
            }
            if let Some(v) = video {
                cmd.args(["-map", &format!("{v}:v:0")]);
            }
            if let Some(a) = mic_input {
                cmd.args(["-map", &format!("{a}:a:0")]);
            }
            if video.is_some() {
                cmd.args(["-c:v", "h264_videotoolbox", "-realtime", "1", "-q:v", "60", "-pix_fmt", "yuv420p"]);
                cmd.args(["-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2"]);
                cmd.args(["-r", &opts.fps.to_string(), "-fps_mode", "cfr"]);
                cmd.args(["-g", &(opts.fps * 2).to_string()]);
            }
            if mic_input.is_some() {
                cmd.args(["-c:a", "aac", "-b:a", "192k", "-ar", "48000"]);
            }
            cmd.args(["-flush_packets", "1"]);
            cmd.arg(&path);
            files.push(path);
            Ok(())
        };
        if let Some(v) = screen_input {
            add_output(&mut cmd, Some(v), "screen.mp4")?;
        }
        if let Some(v) = camera_input {
            add_output(&mut cmd, Some(v), "camera.mp4")?;
        }
        if screen_input.is_none() && camera_input.is_none() {
            add_output(&mut cmd, None, "mic.m4a")?;
        }

        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
        let mut child = ChildGuard(cmd.spawn().with_context(|| format!("could not start {}", ffmpeg.display()))?);
        let stdin = child.0.stdin.take();
        let stderr = Arc::new(Mutex::new(Vec::new()));
        {
            let mut pipe = child.0.stderr.take().expect("stderr was piped");
            let sink = stderr.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut tail = sink.lock().unwrap_or_else(|p| p.into_inner());
                            tail.extend_from_slice(&buf[..n]);
                            if tail.len() > 32 * 1024 {
                                let cut = tail.len() - 16 * 1024;
                                tail.drain(..cut);
                            }
                        }
                    }
                }
            });
        }

        // Without capture permission ffmpeg either exits within moments or sits there
        // receiving nothing. Either way, do not report a recording that is not recording:
        // wait until every file has real data in it.
        let mut recorder = Recorder { child, stdin, stderr, files, started: Instant::now() };
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Ok(Some(_)) = recorder.child.0.try_wait() {
                std::thread::sleep(Duration::from_millis(50)); // let the stderr reader finish
                let text = stderr_text(&recorder.stderr);
                recorder.delete_files();
                return Err(early_exit_error(&text));
            }
            if recorder.files.iter().all(|f| file_len(f) > LIVE_BYTES) {
                break;
            }
            if Instant::now() >= deadline {
                recorder.stdin.take();
                let _ = recorder.child.0.kill();
                let _ = recorder.child.0.wait();
                std::thread::sleep(Duration::from_millis(50));
                let text = stderr_text(&recorder.stderr);
                recorder.delete_files();
                return Err(no_data_error(&text));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        recorder.started = Instant::now();
        Ok(recorder)
    }

    fn delete_files(&self) -> Vec<String> {
        let mut failed = Vec::new();
        for f in &self.files {
            match std::fs::remove_file(f) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => failed.push(format!("{}: {e}", f.display())),
            }
        }
        failed
    }

    /// Total size of the files written so far; it grows steadily while capture is healthy.
    pub fn bytes_written(&self) -> u64 {
        self.files.iter().map(|f| file_len(f)).sum()
    }

    /// Seconds since recording started.
    pub fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// The files being written.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Asks ffmpeg to finish: `q` on stdin, then SIGINT, then kill. Returns true when ffmpeg
    /// exited on its own (so the files were finalised).
    fn shut_down(&mut self, patience: Duration) -> (bool, Option<std::process::ExitStatus>) {
        if let Ok(Some(status)) = self.child.0.try_wait() {
            return (true, Some(status));
        }
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
            // Dropping stdin closes the pipe, which ffmpeg also treats as a request to stop.
        }
        if let Some(status) = self.child.wait_timeout(patience) {
            return (true, Some(status));
        }
        // SAFETY: plain signal delivery to a child process we own and have not reaped.
        unsafe {
            libc::kill(self.child.0.id() as libc::pid_t, libc::SIGINT);
        }
        if let Some(status) = self.child.wait_timeout(patience / 2) {
            return (true, Some(status));
        }
        let _ = self.child.0.kill();
        (false, self.child.0.wait().ok())
    }

    /// Stops gracefully so the files are finalised and playable, and returns them.
    pub fn stop(mut self) -> Result<Vec<PathBuf>> {
        let exited_early = matches!(self.child.0.try_wait(), Ok(Some(_)));
        let (graceful, _status) = self.shut_down(Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(50));
        let text = stderr_text(&self.stderr);
        let usable = |p: &Path| file_len(p) > LIVE_BYTES;
        if exited_early {
            // ffmpeg died during the recording; whatever it wrote was not finalised by us.
            let complete = self.files.iter().all(|f| usable(f));
            if !complete {
                self.delete_files();
                return Err(early_exit_error(&text));
            }
            bail!(
                "the recording stopped on its own after it started (a device may have been disconnected). The partial files were kept: {}\nffmpeg said:\n{}",
                self.files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", "),
                last_lines(&text, 8)
            );
        }
        if !graceful {
            bail!(
                "ffmpeg did not finish the recording when asked and had to be stopped; the files may not be playable: {}\nffmpeg said:\n{}",
                self.files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", "),
                last_lines(&text, 8)
            );
        }
        if let Some(missing) = self.files.iter().find(|f| !usable(f)) {
            let missing = missing.display().to_string();
            self.delete_files();
            return Err(no_data_error(&text).context(format!("nothing was recorded to {missing}")));
        }
        // The container must have been finalised: a file ffprobe cannot read is not a recording.
        for f in &self.files {
            if let Err(e) = crate::proc::probe_duration(f) {
                bail!("the recording {} was written but is not playable ({e})\nffmpeg said:\n{}", f.display(), last_lines(&text, 8));
            }
        }
        Ok(std::mem::take(&mut self.files))
    }

    /// Stops and deletes the files.
    pub fn cancel(mut self) -> Result<()> {
        if let Ok(None) = self.child.0.try_wait() {
            self.stdin.take();
            let _ = self.child.0.kill();
            let _ = self.child.0.wait();
        }
        let failed = self.delete_files();
        if failed.is_empty() { Ok(()) } else { bail!("could not delete the cancelled recording: {}", failed.join("; ")) }
    }
}
