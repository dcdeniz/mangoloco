//! Video preview: the raw frame stream, and the script / layer pictures that sit on top of it.

mod common;
use common::*;
use mango_core::*;
use mango_playback::video::{FrameStream, LayerVideo, Picture, ScriptVideo, VideoFrame};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const FRAME: f64 = 1.0 / VIDEO_FPS;

/// A private copy of the numbered video, so process checks only see this test's decoders.
fn private_copy(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::copy(numbered_video(6, true), &p).unwrap();
    p
}

fn video_editor(path: &Path) -> Editor {
    editor_with_script(asset_per_second(path, MediaKind::Video, 6)).0
}

/// The frame of the numbered video that is on screen at source time `s`.
fn frame_at(s: f64) -> u32 {
    (s * VIDEO_FPS + 1e-6).floor() as u32
}

/// How a frame shown while playing relates to the clock: `Err` if it is ahead of the
/// source time `s` (never acceptable), otherwise how many frames behind it is.
/// The preview compares times with a microsecond of tolerance, hence the upper bound.
fn lateness(n: u32, s: f64) -> Result<u32, String> {
    let newest_allowed = ((s + 3e-6) * VIDEO_FPS).floor() as u32;
    if n > newest_allowed {
        return Err(format!("frame {n} is shown at source {s:.6}, before its time (newest allowed {newest_allowed})"));
    }
    Ok(frame_at(s).saturating_sub(n))
}

/// Polls like a parked UI would until a frame arrives.
fn wait_for_frame(mut request: impl FnMut() -> Picture, budget: Duration) -> (Arc<VideoFrame>, Duration) {
    let t = Instant::now();
    loop {
        match request() {
            Picture::Frame(f) => return (f, t.elapsed()),
            Picture::Blank => panic!("unexpected blank picture"),
            Picture::Pending => {}
        }
        assert!(t.elapsed() < budget, "no frame within {budget:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

// --- FrameStream ------------------------------------------------------------------

#[test]
fn frames_are_sequential_and_frame_accurate() {
    let video = numbered_video(6, true);
    // On a frame boundary, inside a frame, at the start, on a keyframe, just before one.
    for (start, first) in [(2.0, 60), (1.2323, 37), (37.0 / 30.0, 37), (0.0, 0), (3.0, 90), (2.99, 90), (4.5 + 0.02, 136)] {
        let mut s = FrameStream::open(&video, start, 320, 180).unwrap();
        for i in 0..12u32 {
            let f = s.next_frame().unwrap().unwrap_or_else(|| panic!("stream from {start} ended at frame {i}"));
            let n = first + i;
            assert_eq!(frame_number(&f), n, "picture of frame {i} after {start}");
            assert!((f.pts - n as f64 * FRAME).abs() < 1e-4, "pts {} of frame {n}", f.pts);
            assert!(f.pts >= start - 0.001, "nothing before the requested time");
        }
    }
}

#[test]
fn frame_size_honours_the_limit_and_the_aspect() {
    let video = numbered_video(6, true);
    for ((max_w, max_h), (w, h)) in [((160, 160), (160, 90)), ((1000, 1000), (320, 180)), ((100, 30), (53, 30)), ((320, 180), (320, 180)), ((64, 4000), (64, 36)), ((1, 1), (1, 1))] {
        let mut s = FrameStream::open(&video, 1.0, max_w, max_h).unwrap();
        let f = s.next_frame().unwrap().unwrap();
        assert_eq!((f.width, f.height), (w, h), "limit {max_w}x{max_h}");
        assert_eq!(f.bgra.len(), (w * h * 4) as usize);
        if w >= 64 {
            assert_eq!(frame_number(&f), 30, "scaled picture is still frame 30");
        }
    }
    // BGRA: a pure red source arrives as (0, 0, 255, 255).
    let red = generated("red.mp4", |p| ffmpeg(&["-f", "lavfi", "-i", "color=red:s=64x48:r=10:d=1", "-c:v", "libx264", "-pix_fmt", "yuv420p", p.to_str().unwrap()]));
    let f = FrameStream::open(&red, 0.0, 64, 48).unwrap().next_frame().unwrap().unwrap();
    let px = &f.bgra[(24 * 64 + 32) * 4..][..4];
    assert!(px[0] < 40 && px[1] < 40 && px[2] > 215 && px[3] == 255, "pixel {px:?}");
    // Non-square pixels are shown at their display size: 64x48 stored with 2:1 pixels is 128x48.
    let wide = generated("wide.mp4", |p| ffmpeg(&["-f", "lavfi", "-i", "color=red:s=64x48:r=10:d=1", "-vf", "setsar=2/1", "-c:v", "libx264", "-pix_fmt", "yuv420p", p.to_str().unwrap()]));
    let f = FrameStream::open(&wide, 0.0, 1000, 1000).unwrap().next_frame().unwrap().unwrap();
    assert_eq!((f.width, f.height), (128, 48));
    let f = FrameStream::open(&wide, 0.0, 64, 64).unwrap().next_frame().unwrap().unwrap();
    assert_eq!((f.width, f.height), (64, 24));
}

#[test]
fn end_of_file_is_none() {
    let video = numbered_video(6, true);
    let mut s = FrameStream::open(&video, 5.8, 160, 90).unwrap();
    let mut numbers = Vec::new();
    while let Some(f) = s.next_frame().unwrap() {
        numbers.push(frame_number(&f));
    }
    assert_eq!(numbers, [174, 175, 176, 177, 178, 179]);
    assert!(s.next_frame().unwrap().is_none(), "and it stays ended");
    assert!(!process_alive(s.pid()), "the decoder has exited");

    // Starting past the end: no frames, no error.
    let mut s = FrameStream::open(&video, 60.0, 160, 90).unwrap();
    assert!(s.next_frame().unwrap().is_none());
}

#[test]
fn bad_input_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let e = FrameStream::open(&dir.path().join("gone.mp4"), 0.0, 160, 90).err().unwrap().to_string();
    assert!(e.contains("not found") && e.contains("gone.mp4"), "{e}");
    let e = FrameStream::open(dir.path(), 0.0, 160, 90).err().unwrap().to_string();
    assert!(e.contains("not a video file"), "{e}");
    let video = numbered_video(6, true);
    assert!(FrameStream::open(&video, 0.0, 0, 90).is_err());
    assert!(FrameStream::open(&video, f64::NAN, 160, 90).is_err());

    // A file without pictures: the first read explains.
    let mut s = FrameStream::open(&tone_wav(8), 0.0, 160, 90).unwrap();
    let e = format!("{:#}", s.next_frame().expect_err("no video stream"));
    assert!(e.contains("tone8.wav") && e.contains("matches no streams"), "{e}");
    assert!(s.next_frame().unwrap().is_none(), "after the error the stream is simply over");

    // Not media at all.
    let garbage = dir.path().join("garbage.mp4");
    std::fs::write(&garbage, b"not a video\n".repeat(500)).unwrap();
    let mut s = FrameStream::open(&garbage, 0.0, 160, 90).unwrap();
    let e = format!("{:#}", s.next_frame().expect_err("garbage"));
    assert!(e.contains("garbage.mp4"), "{e}");
}

#[test]
fn dropping_a_stream_kills_the_decoder() {
    let dir = tempfile::tempdir().unwrap();
    let video = private_copy(dir.path(), "drop-stream.mp4");
    // Before any frame was read, in the middle, and after the end.
    for frames in [0usize, 5, 1000] {
        let mut s = FrameStream::open(&video, 0.0, 320, 180).unwrap();
        let pid = s.pid();
        for _ in 0..frames {
            if s.next_frame().unwrap().is_none() {
                break;
            }
        }
        if frames == 5 {
            assert!(process_alive(pid), "the decoder waits for the reader mid-file");
        }
        drop(s);
        assert!(!process_alive(pid), "decoder {pid} survived the drop after {frames} frames");
    }
    assert_eq!(children_mentioning("drop-stream.mp4"), 0);
}

// --- ScriptVideo --------------------------------------------------------------------

#[test]
fn playing_across_a_cut_shows_the_right_frames() {
    let dir = tempfile::tempdir().unwrap();
    let video = private_copy(dir.path(), "play-cut.mp4");
    let mut e = video_editor(&video);
    let i = word_index(&e, "w2");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let comp = e.comp().unwrap().clone();
    assert_eq!(comp.script_clips().len(), 2);

    let sv = ScriptVideo::new(320, 180);
    sv.set_clips(&e.project, &comp);

    // A 60 Hz UI following a real-time clock from 0.5 s to 3.5 s; the cut is at 2.0 s.
    let (from, to, cut) = (0.5, 3.5, 2.0);
    let started = Instant::now();
    let mut shown: Vec<(f64, u32)> = Vec::new();
    let mut polls = 0;
    let mut first_frame_after: Option<Duration> = None;
    let mut pending_after_cut = 0;
    let mut slowest_request = Duration::ZERO;
    let mut late = 0;
    loop {
        let t = from + started.elapsed().as_secs_f64();
        if t >= to {
            break;
        }
        polls += 1;
        let call = Instant::now();
        let pic = sv.request(t, true);
        slowest_request = slowest_request.max(call.elapsed());
        match pic {
            Picture::Frame(f) => {
                let n = frame_number(&f);
                let (_, src) = comp.source_at(t).unwrap();
                // Never a frame from the future; normally the frame for this instant or the one before it.
                if lateness(n, src).unwrap_or_else(|e| panic!("at {t:.3} s: {e}")) > 1 {
                    late += 1;
                }
                assert!((f.pts - n as f64 * FRAME).abs() < 1e-4);
                if t >= cut {
                    assert!(n >= 90, "after the cut at {t:.3} s the picture is from before it: frame {n}");
                    first_frame_after.get_or_insert(Duration::from_secs_f64(t - cut));
                } else {
                    assert!(n < 60, "before the cut the deleted second must not show: frame {n}");
                }
                shown.push((t, n));
            }
            Picture::Pending => {
                if t >= cut && first_frame_after.is_none() {
                    pending_after_cut += 1;
                }
            }
            Picture::Blank => panic!("blank at {t}"),
        }
        std::thread::sleep(Duration::from_micros(16_667));
    }
    assert!(shown.len() * 10 >= polls * 9, "only {} of {polls} polls had a picture", shown.len());
    assert!(late * 20 <= polls, "{late} of {polls} polls showed a picture more than one frame late");
    assert!(shown.windows(2).all(|w| w[0].1 <= w[1].1), "frames never go backwards");
    assert!(shown.iter().all(|(_, n)| !(60..90).contains(n)), "the deleted second is never shown");
    // Motion is continuous on both sides: (almost) every source frame gets its turn.
    let distinct: std::collections::BTreeSet<u32> = shown.iter().map(|(_, n)| *n).collect();
    assert!(distinct.len() >= 80, "{} distinct frames in 3 s of 30 fps video", distinct.len());
    let stall = first_frame_after.expect("no frame after the cut");
    println!("MEASURE video stall crossing a cut while playing: first new frame {stall:?} after the cut ({pending_after_cut} pending polls at 60 Hz); {late} of {polls} polls more than a frame late; slowest request() call {slowest_request:?}");
    assert!(stall <= Duration::from_millis(50), "the cut stalled for {stall:?}");
    assert!(slowest_request < Duration::from_millis(20), "request() blocked for {slowest_request:?}");

    drop(sv);
    assert_eq!(children_mentioning("play-cut.mp4"), 0, "dropping the preview stops every decoder");
}

#[test]
fn gap_clips_and_audio_only_sources_are_blank() {
    let dir = tempfile::tempdir().unwrap();
    let video = private_copy(dir.path(), "gap.mp4");
    let mut e = video_editor(&video);
    let i = word_index(&e, "w1");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 1 }).unwrap();
    let sv = ScriptVideo::new(160, 90);
    assert!(matches!(sv.request(0.0, false), Picture::Blank), "nothing loaded yet");
    sv.set_clips(&e.project, e.comp().unwrap());

    for playing in [false, true] {
        for t in [1.0, 1.5, 1.999] {
            assert!(matches!(sv.request(t, playing), Picture::Blank), "gap at {t} (playing {playing})");
        }
    }
    // Either side of the gap is video.
    let (f, _) = wait_for_frame(|| sv.request(0.5, false), Duration::from_secs(5));
    assert_eq!(frame_number(&f), 15);
    let (f, _) = wait_for_frame(|| sv.request(2.5, false), Duration::from_secs(5));
    assert_eq!(frame_number(&f), 75);

    // Playing through the gap: blank during it, and the picture is back right after.
    let started = Instant::now();
    let mut first_after = None;
    loop {
        let t = 0.6 + started.elapsed().as_secs_f64();
        if t > 2.5 {
            break;
        }
        match sv.request(t, true) {
            Picture::Blank => assert!((1.0..2.0).contains(&t), "blank outside the gap at {t}"),
            Picture::Frame(f) => {
                assert!(!(1.0..2.0).contains(&t), "a frame inside the gap at {t}");
                let n = frame_number(&f);
                let behind = lateness(n, t).unwrap_or_else(|e| panic!("{e}"));
                assert!(behind <= 3, "frame {n} at {t} is {behind} frames late");
                assert!(if t < 1.0 { n < 30 } else { n >= 60 }, "frame {n} at {t} is from the wrong side of the gap");
                if t >= 2.0 {
                    first_after.get_or_insert(t - 2.0);
                }
            }
            Picture::Pending => assert!(!(1.0..2.0).contains(&t)),
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    assert!(first_after.expect("no picture after the gap") < 0.05, "picture came back {first_after:?} s after the gap");

    // A script made from an audio file has no picture at all.
    let audio = editor_with_script(asset_per_second(&tone_wav(8), MediaKind::Audio, 8)).0;
    sv.set_clips(&audio.project, audio.comp().unwrap());
    for t in [0.0, 3.3, 7.9, 100.0] {
        assert!(matches!(sv.request(t, false), Picture::Blank));
        assert!(matches!(sv.request(t, true), Picture::Blank));
    }
    // So does a hidden script video, and an empty script.
    let mut hidden = video_editor(&video);
    hidden.apply(Command::SetScriptVideo { placement: VideoPlacement { hidden: true, ..Default::default() } }).unwrap();
    sv.set_clips(&hidden.project, hidden.comp().unwrap());
    assert!(matches!(sv.request(1.0, false), Picture::Blank));
    let empty = Editor::new_project("Empty");
    sv.set_clips(&empty.project, empty.comp().unwrap());
    assert!(matches!(sv.request(1.0, true), Picture::Blank));
}

#[test]
fn seeking_while_paused_gives_the_exact_frame() {
    let dir = tempfile::tempdir().unwrap();
    let video = private_copy(dir.path(), "seek.mp4");
    let mut e = video_editor(&video);
    let i = word_index(&e, "w2");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let comp = e.comp().unwrap().clone();
    let sv = ScriptVideo::new(320, 180);
    sv.set_clips(&e.project, &comp);

    // Random access all over the timeline, including both sides of the cut and frame boundaries.
    let jumps = [4.71, 0.3, 2.5, 4.999, 1.0333, 2.0, 1.9999, 0.0, 3.0 + 7.0 * FRAME, 3.0 + 7.0 * FRAME - 0.0001, 4.2, 0.9];
    let mut latencies = Vec::new();
    let mut slowest_request = Duration::ZERO;
    for t in jumps {
        let (f, took) = wait_for_frame(
            || {
                let call = Instant::now();
                let p = sv.request(t, false);
                slowest_request = slowest_request.max(call.elapsed());
                p
            },
            Duration::from_secs(5),
        );
        let (_, src) = comp.source_at(t).unwrap();
        assert_eq!(frame_number(&f), frame_at(src), "paused at {t} (source {src})");
        // Asking again is immediate and gives the same frame.
        match sv.request(t, false) {
            Picture::Frame(again) => assert!(Arc::ptr_eq(&f, &again)),
            other => panic!("the frame went away: {other:?}"),
        }
        latencies.push(took);
    }
    latencies.sort();
    let (median, worst) = (latencies[latencies.len() / 2], *latencies.last().unwrap());

    // Stepping forward frame by frame reuses the running decoder.
    let mut steps = Vec::new();
    wait_for_frame(|| sv.request(3.0, false), Duration::from_secs(5));
    for k in 1..=20 {
        let t = 3.0 + k as f64 * FRAME + 0.001;
        let (f, took) = wait_for_frame(|| sv.request(t, false), Duration::from_secs(5));
        assert_eq!(frame_number(&f), frame_at(comp.source_at(t).unwrap().1), "step {k}");
        steps.push(took);
    }
    steps.sort();
    // Stepping backwards has to restart the decoder each time.
    let mut back = Vec::new();
    for k in 1..=6 {
        let t = 3.7 - k as f64 * FRAME + 0.001;
        let (f, took) = wait_for_frame(|| sv.request(t, false), Duration::from_secs(5));
        assert_eq!(frame_number(&f), frame_at(comp.source_at(t).unwrap().1), "step back {k}");
        back.push(took);
    }
    back.sort();
    println!(
        "MEASURE paused seek to exact frame (320x180 H.264, 1 s GOP): median {median:?}, worst {worst:?}; frame step forward median {:?}, worst {:?}; frame step back median {:?}, worst {:?}; slowest request() call {slowest_request:?}",
        steps[steps.len() / 2],
        steps.last().unwrap(),
        back[back.len() / 2],
        back.last().unwrap()
    );
    assert!(worst < Duration::from_millis(1000), "a paused seek took {worst:?}");
    assert!(*steps.last().unwrap() < Duration::from_millis(250), "a frame step took {:?}", steps.last().unwrap());
    assert!(slowest_request < Duration::from_millis(20), "request() blocked for {slowest_request:?}");

    // Scrubbing: a burst of requests at different times ends on the frame for the last one.
    for k in 0..40 {
        let _ = sv.request(0.2 + k as f64 * 0.11, false);
        std::thread::sleep(Duration::from_millis(4));
    }
    let (f, _) = wait_for_frame(|| sv.request(4.44, false), Duration::from_secs(5));
    assert_eq!(frame_number(&f), frame_at(comp.source_at(4.44).unwrap().1));
    assert_eq!(frame_number(&sv.last_frame().unwrap()), frame_number(&f));

    // At and past the end the last frame stays up.
    for t in [5.0, 6.0, 1e9, f64::INFINITY] {
        let (f, _) = wait_for_frame(|| sv.request(t, false), Duration::from_secs(5));
        assert_eq!(frame_number(&f), 179, "at {t}");
    }
    // Times before the start, or not a number, are treated as the start.
    for t in [-3.0, f64::NEG_INFINITY, f64::NAN] {
        let (f, _) = wait_for_frame(|| sv.request(t, false), Duration::from_secs(5));
        assert_eq!(frame_number(&f), 0, "at {t}");
    }
    drop(sv);
    assert_eq!(children_mentioning("seek.mp4"), 0);
}

#[test]
fn set_clips_after_an_edit_changes_the_picture() {
    let dir = tempfile::tempdir().unwrap();
    let video = private_copy(dir.path(), "edit.mp4");
    let mut e = video_editor(&video);
    let sv = ScriptVideo::new(320, 180);
    sv.set_clips(&e.project, e.comp().unwrap());
    let (f, _) = wait_for_frame(|| sv.request(2.5, false), Duration::from_secs(5));
    assert_eq!(frame_number(&f), 75);

    // Delete the word under the playhead: the same composition time now shows source 3.5 s.
    let i = word_index(&e, "w2");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    sv.set_clips(&e.project, e.comp().unwrap());
    let (f, took) = wait_for_frame(|| sv.request(2.5, false), Duration::from_secs(5));
    assert_eq!(frame_number(&f), 105);
    println!("MEASURE picture after an edit under the playhead: {took:?}");

    // Undo: back to the original frame.
    assert!(e.undo());
    sv.set_clips(&e.project, e.comp().unwrap());
    let (f, _) = wait_for_frame(|| sv.request(2.5, false), Duration::from_secs(5));
    assert_eq!(frame_number(&f), 75);

    // An edit elsewhere keeps the picture without a new decode.
    let i = word_index(&e, "w5");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    sv.set_clips(&e.project, e.comp().unwrap());
    match sv.request(2.5, false) {
        Picture::Frame(same) => assert!(Arc::ptr_eq(&same, &f), "the frame on screen is reused"),
        other => panic!("an unrelated edit dropped the picture: {other:?}"),
    }

    // Moving text: the moved part plays first.
    let mut e = video_editor(&video);
    let i = word_index(&e, "w4");
    e.apply(Command::MoveRange { start: i, end: i + 1, dest: 0 }).unwrap();
    sv.set_clips(&e.project, e.comp().unwrap());
    let (f, _) = wait_for_frame(|| sv.request(0.5, false), Duration::from_secs(5));
    assert_eq!(frame_number(&f), 135);
    let (f, _) = wait_for_frame(|| sv.request(1.5, false), Duration::from_secs(5));
    assert_eq!(frame_number(&f), 15);
}

#[test]
fn on_frame_fires_when_a_pending_picture_is_ready() {
    let dir = tempfile::tempdir().unwrap();
    let video = private_copy(dir.path(), "callback.mp4");
    let e = video_editor(&video);
    let sv = ScriptVideo::new(320, 180);
    sv.set_clips(&e.project, e.comp().unwrap());
    let fired = Arc::new(AtomicUsize::new(0));
    {
        let fired = fired.clone();
        sv.set_on_frame(Box::new(move || {
            fired.fetch_add(1, Ordering::SeqCst);
        }));
    }
    // An event-driven UI: ask once, then only ask again when told to.
    assert!(matches!(sv.request(3.3, false), Picture::Pending), "the first request cannot be ready yet");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = 0;
    let frame = loop {
        let now = fired.load(Ordering::SeqCst);
        if now != seen {
            seen = now;
            if let Picture::Frame(f) = sv.request(3.3, false) {
                break f;
            }
        }
        assert!(Instant::now() < deadline, "the callback never led to a frame ({seen} calls)");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(frame_number(&frame), 99);
    assert!(seen >= 1);
    // Once the picture is there and nobody is waiting, the callback is quiet.
    std::thread::sleep(Duration::from_millis(300));
    let settled = fired.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(fired.load(Ordering::SeqCst), settled, "no callbacks without a pending request");
}

#[test]
fn an_unreadable_source_is_blank_with_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let e = video_editor(&dir.path().join("moved-away.mp4"));
    let sv = ScriptVideo::new(320, 180);
    sv.set_clips(&e.project, e.comp().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match sv.request(1.0, false) {
            Picture::Blank => break,
            Picture::Pending => {}
            Picture::Frame(_) => panic!("a frame from a missing file"),
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let err = sv.take_error().expect("the reason is available");
    assert!(err.contains("not found") && err.contains("moved-away.mp4"), "{err}");
    assert_eq!(sv.take_error(), None, "reading clears it");
}

#[test]
fn hd_long_gop_source() {
    // What real footage costs: 1080p with keyframes 8.33 s apart, previewed at 960x540.
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("hd-preview.mp4");
    std::fs::copy(hd_video(), &video).unwrap();
    let mut asset = asset_per_second(&video, MediaKind::Video, 20);
    (asset.width, asset.height, asset.has_audio) = (1920, 1080, false);
    let (mut e, _) = editor_with_script(asset);
    // Cut 10 s..13 s out: the picture jumps from source 10.0 to 13.0, which is mid-GOP.
    let i = word_index(&e, "w10");
    e.apply(Command::DeleteRange { start: i, end: i + 3 }).unwrap();
    let comp = e.comp().unwrap().clone();
    let sv = ScriptVideo::new(960, 540);
    sv.set_clips(&e.project, &comp);

    // Paused seeks: right after a keyframe (cheap) to right before the next one (a whole GOP to decode).
    let mut report = Vec::new();
    let mut worst = Duration::ZERO;
    for t in [0.1, 8.2, 4.0, 16.9, 9.99, 10.0, 12.3, 1.0] {
        let (f, took) = wait_for_frame(|| sv.request(t, false), Duration::from_secs(10));
        let src = comp.source_at(t).unwrap().1;
        assert_eq!(hd_frame_number(&f), frame_at(src), "paused at {t} (source {src})");
        assert_eq!((f.width, f.height), (960, 540));
        report.push(format!("{t} s: {} ms", took.as_millis()));
        worst = worst.max(took);
    }
    println!("MEASURE paused seek, 1080p H.264 with 8.33 s GOP at 960x540: {} (worst {worst:?})", report.join(", "));
    assert!(worst < Duration::from_secs(2), "a paused seek took {worst:?}");

    // Playing across the cut in real time.
    let (from, to, cut) = (8.8, 11.2, 10.0);
    let started = Instant::now();
    let (mut polls, mut shown, mut late) = (0, 0, 0);
    let mut first_after = None;
    let mut distinct = std::collections::BTreeSet::new();
    loop {
        let t = from + started.elapsed().as_secs_f64();
        if t >= to {
            break;
        }
        polls += 1;
        if let Picture::Frame(f) = sv.request(t, true) {
            let n = hd_frame_number(&f);
            let behind = lateness(n, comp.source_at(t).unwrap().1).unwrap_or_else(|e| panic!("at {t:.3} s: {e}"));
            if t >= cut {
                assert!(n >= 390, "a frame from before the cut ({n}) at {t:.3} s");
                first_after.get_or_insert(t - cut);
            } else {
                assert!(n < 300, "a deleted frame ({n}) at {t:.3} s");
            }
            // Once playback has settled, the picture keeps up with the clock to within a frame.
            if t > from + 0.7 && behind > 1 {
                late += 1;
            }
            distinct.insert(n);
            shown += 1;
        }
        std::thread::sleep(Duration::from_micros(16_667));
    }
    let first_after = first_after.expect("no picture after the cut");
    println!("MEASURE playing 1080p across a cut: picture on {shown} of {polls} polls, {} distinct frames in {:.1} s, first frame after the cut at +{:.0} ms, {late} polls more than a frame late", distinct.len(), to - from, first_after * 1e3);
    assert!(first_after < 0.05, "the cut stalled for {first_after} s");
    assert!(late * 20 <= polls, "{late} of {polls} polls showed a picture more than one frame late");
    assert!(distinct.len() >= 50, "{} distinct frames", distinct.len());

    // Releasing stops the decoders; the next request brings the picture back.
    sv.release();
    assert_eq!(children_mentioning("hd-preview.mp4"), 0);
    let (f, _) = wait_for_frame(|| sv.request(5.0, false), Duration::from_secs(10));
    assert_eq!(hd_frame_number(&f), 150);
    drop(sv);
    assert_eq!(children_mentioning("hd-preview.mp4"), 0);
}

// --- LayerVideo ----------------------------------------------------------------------

#[test]
fn layer_video_follows_source_time() {
    let dir = tempfile::tempdir().unwrap();
    let video = private_copy(dir.path(), "layer.mp4");
    let lv = LayerVideo::new(&video, 160, 90);
    let fired = Arc::new(AtomicUsize::new(0));
    {
        let fired = fired.clone();
        lv.set_on_frame(Box::new(move || {
            fired.fetch_add(1, Ordering::SeqCst);
        }));
    }

    // Parked: the exact frame.
    for s in [1.5, 4.0 + 0.5 * FRAME, 0.0, 5.99] {
        let (f, _) = wait_for_frame(|| lv.request(s, false), Duration::from_secs(5));
        assert_eq!(frame_number(&f), frame_at(s), "paused at source {s}");
        assert_eq!((f.width, f.height), (160, 90));
    }
    assert!(fired.load(Ordering::SeqCst) >= 1);

    // A layer with src_in 1.0 playing at double speed, looping after 2 s of source.
    let (src_in, speed, loop_len) = (1.0, 2.0, 2.0);
    let started = Instant::now();
    let (mut shown, mut polls, mut late) = (0, 0, 0);
    let mut numbers = std::collections::BTreeSet::new();
    loop {
        let t = started.elapsed().as_secs_f64();
        if t > 2.2 {
            break;
        }
        let s = src_in + (t * speed) % loop_len;
        polls += 1;
        if let Picture::Frame(f) = lv.request(s, true) {
            let n = frame_number(&f);
            // At double speed the playhead moves two source frames per shown frame.
            if lateness(n, s).unwrap_or_else(|e| panic!("{e}")) > 2 {
                late += 1;
            }
            numbers.insert(n);
            shown += 1;
        }
        std::thread::sleep(Duration::from_micros(16_667));
    }
    assert!(shown * 10 >= polls * 8, "only {shown} of {polls} polls had a picture");
    // The decoder restarts at each loop point and needs a moment to catch up there.
    assert!(late * 10 <= polls, "{late} of {polls} polls showed a picture more than two frames late");
    assert!(numbers.iter().all(|n| (30..90).contains(n)), "only the looped range is shown: {numbers:?}");
    assert!(numbers.len() >= 40, "{} distinct frames", numbers.len());

    // Well past the end of the file there is no picture (the layer has run out of video).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match lv.request(60.0, false) {
            Picture::Blank => break,
            Picture::Pending => {}
            Picture::Frame(f) => panic!("frame {} shown a minute past the end", frame_number(&f)),
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(lv.take_error(), None, "running out of video is not an error");
    drop(lv);
    assert_eq!(children_mentioning("layer.mp4"), 0);

    let missing = LayerVideo::new(&dir.path().join("nope.mp4"), 160, 90);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !matches!(missing.request(0.0, true), Picture::Blank) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(missing.take_error().unwrap().contains("not found"));
}
