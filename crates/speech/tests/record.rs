mod common;

use std::path::Path;
use std::time::Duration;

use mango_speech::record::{self, Device, Options, Recorder};

/// Captured from `ffmpeg -f avfoundation -list_devices true -i ""` on the development Mac.
const SAMPLE_THIS_MAC: &str = "\
[AVFoundation indev @ 0x9f4c20140] AVFoundation video devices:
[AVFoundation indev @ 0x9f4c20140] [0] FaceTime HD Camera
[AVFoundation indev @ 0x9f4c20140] [1] Capture screen 0
[AVFoundation indev @ 0x9f4c20140] AVFoundation audio devices:
[AVFoundation indev @ 0x9f4c20140] [0] MacBook Pro Microphone
[AVFoundation indev @ 0x9f4c20140] [1] Microsoft Teams Audio
[in#0 @ 0x9f4c20000] Error opening input: Input/output error
Error opening input file .
Error opening input files: Input/output error
";

/// A busier machine: banner noise, an iPhone, two displays, names with brackets and digits.
const SAMPLE_BUSY: &str = "\
ffmpeg version 8.1 Copyright (c) 2000-2026 the FFmpeg developers
  built with Apple clang version 17.0.0
objc[48036]: class `NSKVONotifying_AVCaptureScreenInput' not linked into application
[AVFoundation indev @ 0x12a704f20] AVFoundation video devices:
[AVFoundation indev @ 0x12a704f20] [0] FaceTime HD Camera
[AVFoundation indev @ 0x12a704f20] [1] Cem's iPhone Camera
[AVFoundation indev @ 0x12a704f20] [2] OBS Virtual Camera [v2]
[AVFoundation indev @ 0x12a704f20] [3] Capture screen 0
[AVFoundation indev @ 0x12a704f20] [4] Capture screen 1
[AVFoundation indev @ 0x12a704f20] AVFoundation audio devices:
[AVFoundation indev @ 0x12a704f20] [0] Studio Display Microphone
[AVFoundation indev @ 0x12a704f20] [1] Scarlett 2i2 USB
[AVFoundation indev @ 0x12a704f20] [2] BlackHole 2ch
[AVFoundation indev @ 0x12a704f20] [10] Aggregate Device
: Input/output error
";

fn d(index: u32, name: &str) -> Device {
    Device { index, name: name.to_string() }
}

#[test]
fn parses_the_device_list_of_this_mac() {
    let devices = record::parse_device_list(SAMPLE_THIS_MAC);
    assert_eq!(devices.cameras, [d(0, "FaceTime HD Camera")]);
    assert_eq!(devices.screens, [d(1, "Capture screen 0")]);
    assert_eq!(devices.microphones, [d(0, "MacBook Pro Microphone"), d(1, "Microsoft Teams Audio")]);
}

#[test]
fn parses_a_busy_device_list() {
    let devices = record::parse_device_list(SAMPLE_BUSY);
    assert_eq!(devices.cameras, [d(0, "FaceTime HD Camera"), d(1, "Cem's iPhone Camera"), d(2, "OBS Virtual Camera [v2]")]);
    assert_eq!(devices.screens, [d(3, "Capture screen 0"), d(4, "Capture screen 1")]);
    assert_eq!(devices.microphones, [d(0, "Studio Display Microphone"), d(1, "Scarlett 2i2 USB"), d(2, "BlackHole 2ch"), d(10, "Aggregate Device")]);
}

#[test]
fn parser_survives_garbage() {
    for text in ["", "no devices here", "[x] [y] z\n[AVFoundation indev @ 0x1] [abc] bad index\n[1] orphan", "[AVFoundation indev @ 0x1] AVFoundation video devices:\n"] {
        let devices = record::parse_device_list(text);
        assert!(devices.cameras.is_empty() && devices.screens.is_empty() && devices.microphones.is_empty(), "{text:?} -> {devices:?}");
    }
}

#[test]
fn real_devices_are_enumerated() {
    let devices = record::devices().unwrap();
    println!("{devices:#?}");
    assert!(
        !devices.screens.is_empty() || !devices.cameras.is_empty() || !devices.microphones.is_empty(),
        "expected at least one capture device on this Mac"
    );
    for dev in devices.screens.iter().chain(&devices.cameras).chain(&devices.microphones) {
        assert!(!dev.name.trim().is_empty());
    }
    assert!(devices.screens.iter().all(|s| s.name.starts_with("Capture screen")));
    assert!(devices.cameras.iter().all(|s| !s.name.starts_with("Capture screen")));
}

fn options(dir: &Path, name: &str) -> Options {
    Options { screen: None, camera: None, microphone: None, fps: 5, capture_cursor: true, out_dir: dir.to_path_buf(), base_name: name.to_string() }
}

fn files_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir).map(|d| d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
}

#[test]
fn invalid_requests_are_rejected_before_anything_starts() {
    let dir = tempfile::tempdir().unwrap();
    let err = Recorder::start(&options(dir.path(), "take")).err().expect("nothing selected");
    assert!(err.to_string().contains("nothing to record"), "{err}");
    for bad in ["", "  ", "a/b", "../x", ".hidden"] {
        let mut o = options(dir.path(), bad);
        o.microphone = Some(0);
        assert!(Recorder::start(&o).is_err(), "base name {bad:?} must be rejected");
    }
    let mut o = options(dir.path(), "take");
    o.microphone = Some(0);
    o.fps = 0;
    assert!(Recorder::start(&o).err().unwrap().to_string().contains("frame rate"));
    assert!(files_in(dir.path()).is_empty());
}

#[test]
fn existing_recordings_are_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("take-mic.m4a"), b"precious").unwrap();
    let mut o = options(dir.path(), "take");
    o.microphone = Some(0);
    let err = Recorder::start(&o).err().expect("must refuse to overwrite");
    assert!(err.to_string().contains("already exists"), "{err}");
    assert_eq!(std::fs::read(dir.path().join("take-mic.m4a")).unwrap(), b"precious");
}

#[test]
fn a_device_that_does_not_exist_gives_a_plain_error_with_ffmpeg_output() {
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), "ghost");
    o.screen = Some(97);
    let started = std::time::Instant::now();
    let err = Recorder::start(&o).err().expect("device 97 does not exist");
    let text = err.to_string();
    println!("{text}");
    assert!(text.contains("permission") && text.contains("System Settings"), "should explain the likely cause: {text}");
    assert!(text.contains("ffmpeg said:"), "should include ffmpeg's output: {text}");
    assert!(text.lines().count() >= 2);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(files_in(dir.path()).is_empty(), "no files may be left: {:?}", files_in(dir.path()));
}

/// What a failed start must look like, whatever the reason.
fn assert_clear_capture_error(err: &anyhow::Error, dir: &Path) {
    let text = err.to_string();
    println!("capture was refused:\n{text}");
    assert!(text.contains("permission"), "{text}");
    assert!(text.contains("Screen Recording") && text.contains("Camera") && text.contains("Microphone"), "{text}");
    assert!(text.contains("System Settings"), "{text}");
    assert!(text.contains("ffmpeg said:"), "{text}");
    assert!(files_in(dir).is_empty(), "a refused recording must not leave files: {:?}", files_in(dir));
}

fn assert_playable(path: &Path, seconds: f64, video: bool) {
    let duration = common::probe_duration(path);
    assert!((duration - seconds).abs() < 1.2, "{} lasts {duration} s, expected about {seconds} s", path.display());
    let streams = common::run(std::process::Command::new(common::ffprobe())
        .args(["-v", "error", "-show_entries", "stream=codec_type,r_frame_rate,avg_frame_rate", "-of", "default=noprint_wrappers=1"])
        .arg(path));
    if video {
        assert!(streams.contains("codec_type=video"), "{streams}");
        assert!(streams.contains("r_frame_rate=5/1") && streams.contains("avg_frame_rate=5/1"), "expected constant 5 fps: {streams}");
    } else {
        assert!(streams.contains("codec_type=audio"), "{streams}");
    }
    // Decodes to the end without errors.
    common::run(std::process::Command::new(common::ffmpeg()).args(["-v", "error", "-xerror", "-i"]).arg(path).args(["-f", "null", "-"]));
}

/// Tries a real 2-second capture. Where macOS allows it, the file must be right; where macOS
/// refuses (no Screen Recording / Microphone permission for this process), the error must
/// say so. Either way nothing is faked.
fn capture_or_explain(o: &Options, video: bool) -> bool {
    match Recorder::start(o) {
        Ok(recorder) => {
            std::thread::sleep(Duration::from_secs(2));
            assert!(recorder.elapsed() >= 2.0 && recorder.elapsed() < 4.0);
            assert!(recorder.bytes_written() > 1024);
            let files = recorder.stop().unwrap();
            assert_eq!(files.len(), 1);
            assert_playable(&files[0], 2.0, video);
            true
        }
        Err(err) => {
            assert_clear_capture_error(&err, &o.out_dir);
            false
        }
    }
}

#[test]
fn screen_capture_works_or_explains_the_missing_permission() {
    let devices = record::devices().unwrap();
    let Some(screen) = devices.screens.first() else {
        println!("no screen device on this machine");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), "screen-test");
    o.screen = Some(screen.index);
    let worked = capture_or_explain(&o, true);
    println!("screen capture worked: {worked}");
}

#[test]
fn microphone_capture_works_or_explains_the_missing_permission() {
    let devices = record::devices().unwrap();
    let Some(mic) = devices.microphones.first() else {
        println!("no microphone on this machine");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), "mic-test");
    o.microphone = Some(mic.index);
    let worked = capture_or_explain(&o, false);
    println!("microphone capture worked: {worked}");
}

#[test]
#[ignore = "needs macOS capture permission"]
fn records_the_screen_with_microphone_to_a_playable_constant_frame_rate_file() {
    let devices = record::devices().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), "full");
    o.screen = Some(devices.screens.first().expect("a screen").index);
    o.microphone = Some(devices.microphones.first().expect("a microphone").index);
    let recorder = Recorder::start(&o).expect("capture permission is required for this test");
    assert_eq!(recorder.files(), [dir.path().join("full-screen.mp4")]);
    std::thread::sleep(Duration::from_secs(3));
    let files = recorder.stop().unwrap();
    assert_eq!(files, [dir.path().join("full-screen.mp4")]);
    assert_playable(&files[0], 3.0, true);
    let streams = common::run(std::process::Command::new(common::ffprobe())
        .args(["-v", "error", "-show_entries", "stream=codec_name", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(&files[0]));
    assert!(streams.contains("h264") && streams.contains("aac"), "screen file should carry the microphone: {streams}");
}

#[test]
#[ignore = "needs macOS capture permission"]
fn records_screen_and_camera_to_separate_files_and_cancel_deletes_them() {
    let devices = record::devices().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), "both");
    o.screen = Some(devices.screens.first().expect("a screen").index);
    o.camera = Some(devices.cameras.first().expect("a camera").index);
    o.microphone = devices.microphones.first().map(|m| m.index);
    let recorder = Recorder::start(&o).expect("capture permission is required for this test");
    std::thread::sleep(Duration::from_secs(2));
    let files = recorder.stop().unwrap();
    assert_eq!(files, [dir.path().join("both-screen.mp4"), dir.path().join("both-camera.mp4")]);
    for f in &files {
        assert_playable(f, 2.0, true);
    }

    o.base_name = "discard".into();
    let recorder = Recorder::start(&o).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    recorder.cancel().unwrap();
    assert!(!files_in(dir.path()).iter().any(|f| f.starts_with("discard")), "{:?}", files_in(dir.path()));
}

#[test]
#[ignore = "needs macOS capture permission"]
fn records_the_microphone_alone_to_m4a() {
    let devices = record::devices().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut o = options(dir.path(), "voice");
    o.microphone = Some(devices.microphones.first().expect("a microphone").index);
    let recorder = Recorder::start(&o).expect("capture permission is required for this test");
    std::thread::sleep(Duration::from_secs(2));
    let files = recorder.stop().unwrap();
    assert_eq!(files, [dir.path().join("voice-mic.m4a")]);
    assert_playable(&files[0], 2.0, false);
}
