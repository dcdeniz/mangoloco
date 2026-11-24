//! Probing, waveform peaks and still frames against real generated files.

mod common;
use common::*;
use mango_core::MediaKind;
use mango_media::frames::{frame_at, save_png};
use mango_media::probe::probe;
use mango_media::waveform::peaks_with_cache_dir;

fn testsrc() -> std::path::PathBuf {
    // testsrc2 burns a running frame counter and timestamp into the picture.
    lavfi(
        "testsrc2_320x240.mp4",
        &["-f", "lavfi", "-i", "testsrc2=s=320x240:r=30:d=3", "-f", "lavfi", "-i", "sine=f=440:d=3:r=44100", "-ac", "2", "-c:v", "libx264", "-crf", "14", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", "-f", "mp4"],
    )
}

#[test]
fn probes_a_video_with_audio() {
    let path = testsrc();
    let a = probe(&path).unwrap();
    assert_eq!(a.kind, MediaKind::Video);
    assert_eq!(a.name, "testsrc2_320x240.mp4");
    assert_eq!(a.path, path);
    assert_eq!((a.width, a.height), (320, 240));
    assert!((a.fps - 30.0).abs() < 0.01, "fps {}", a.fps);
    assert!((a.duration - 3.0).abs() < 0.05, "duration {}", a.duration);
    assert!(a.has_audio && a.has_video);
    assert_eq!((a.sample_rate, a.channels), (44_100, 2));
    assert!(a.transcript.is_none() && a.folder.is_none());
    // A fresh id every time.
    assert_ne!(a.id, probe(&path).unwrap().id);
}

#[test]
fn probes_rotated_phone_video_with_display_dimensions() {
    let src = testsrc();
    for (deg, expect) in [("90", (240, 320)), ("-90", (240, 320)), ("180", (320, 240)), ("270", (240, 320))] {
        let rotated = fixture(&format!("testsrc2_rot{deg}.mp4"), |out| {
            ff(&["-display_rotation", deg, "-i", src.to_str().unwrap(), "-c", "copy", "-f", "mp4", out.to_str().unwrap()]);
        });
        let a = probe(&rotated).unwrap();
        assert_eq!(a.kind, MediaKind::Video);
        assert_eq!((a.width, a.height), expect, "rotation {deg}");
        // The frame grabber shows the same orientation.
        let f = frame_at(&rotated, 0.5, 1000, 1000).unwrap();
        assert_eq!((f.width, f.height), expect, "frame size for rotation {deg}");
    }
}

#[test]
fn probes_anamorphic_video_with_display_width() {
    // 360x240 stored, 4:3 pixels -> displays as 480x240.
    let path = lavfi("anamorphic.mp4", &["-f", "lavfi", "-i", "testsrc2=s=360x240:r=25:d=1", "-vf", "setsar=4/3", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-f", "mp4"]);
    let a = probe(&path).unwrap();
    assert_eq!((a.width, a.height), (480, 240));
    assert!(!a.has_audio);
    assert_eq!((a.sample_rate, a.channels), (0, 0));
    let f = frame_at(&path, 0.0, 4000, 4000).unwrap();
    assert_eq!((f.width, f.height), (480, 240), "frames are returned with square pixels");
}

#[test]
fn audio_with_cover_art_is_audio_not_video() {
    let cover = solid_png("cover_300.png", "0xFF0000", 300, 300);
    let path = fixture("song_with_cover.mp3", |out| {
        ff(&[
            "-f", "lavfi", "-i", "sine=f=440:d=4:r=44100", "-i", cover.to_str().unwrap(), "-map", "0:a", "-map", "1:v", "-c:a", "libmp3lame", "-b:a", "128k", "-c:v", "mjpeg", "-disposition:v", "attached_pic", "-id3v2_version", "3", "-f", "mp3",
            out.to_str().unwrap(),
        ]);
    });
    // The file really does carry a picture stream.
    let j = ffprobe_json(&path, &[]);
    assert!(j["streams"].as_array().unwrap().iter().any(|s| s["codec_type"] == "video"), "fixture should contain cover art");
    let a = probe(&path).unwrap();
    assert_eq!(a.kind, MediaKind::Audio);
    assert!(a.has_audio);
    assert!(!a.has_video, "cover art is not video");
    assert_eq!((a.width, a.height), (0, 0));
    assert_eq!(a.fps, 0.0);
    assert!((a.duration - 4.0).abs() < 0.1, "duration {}", a.duration);
    assert_eq!((a.sample_rate, a.channels), (44_100, 1));
    // m4a with cover art too.
    let m4a = fixture("song_with_cover.m4a", |out| {
        ff(&["-f", "lavfi", "-i", "sine=f=440:d=2:r=48000", "-i", cover.to_str().unwrap(), "-map", "0:a", "-map", "1:v", "-c:a", "aac", "-c:v", "mjpeg", "-disposition:v", "attached_pic", "-f", "ipod", out.to_str().unwrap()]);
    });
    let a = probe(&m4a).unwrap();
    assert_eq!((a.kind, a.has_video), (MediaKind::Audio, false));
}

#[test]
fn plain_audio_files() {
    let wav = tone_wav("tone_440_full.wav", 440, 2.0, 8.0);
    let a = probe(&wav).unwrap();
    assert_eq!(a.kind, MediaKind::Audio);
    assert!((a.duration - 2.0).abs() < 0.001);
    assert_eq!((a.sample_rate, a.channels, a.has_video), (48_000, 1, false));
}

#[test]
fn still_images_are_images_with_zero_duration() {
    let png = solid_png("green_100.png", "0x00FF00", 100, 100);
    let jpg = lavfi("still_120x80.jpg", &["-f", "lavfi", "-i", "color=c=0x0000FF:s=120x80", "-frames:v", "1", "-update", "1", "-f", "image2", "-c:v", "mjpeg"]);
    let bmp = lavfi("still_50x70.bmp", &["-f", "lavfi", "-i", "color=c=0x0000FF:s=50x70", "-frames:v", "1", "-update", "1", "-f", "image2", "-c:v", "bmp"]);
    for (path, size) in [(&png, (100, 100)), (&jpg, (120, 80)), (&bmp, (50, 70))] {
        let a = probe(path).unwrap();
        assert_eq!(a.kind, MediaKind::Image, "{}", path.display());
        assert_eq!(a.duration, 0.0);
        assert_eq!((a.width, a.height), size);
        assert!(a.has_video && !a.has_audio);
    }
}

#[test]
fn heic_photo_is_an_image() {
    // macOS can write HEIC, the format iPhones shoot in.
    let png = solid_png("green_100.png", "0x00FF00", 100, 100);
    let dir = tempfile::tempdir().unwrap();
    let heic = dir.path().join("photo.heic");
    let ok = std::process::Command::new("/usr/bin/sips").args(["-s", "format", "heic"]).arg(&png).arg("--out").arg(&heic).output().map(|o| o.status.success()).unwrap_or(false);
    if !ok || !heic.is_file() {
        eprintln!("skipping: sips cannot write HEIC here");
        return;
    }
    let a = probe(&heic).unwrap();
    assert_eq!(a.kind, MediaKind::Image);
    assert_eq!((a.width, a.height, a.duration), (100, 100, 0.0));
    let f = frame_at(&heic, 0.0, 100, 100).unwrap();
    let p = f.pixel(50, 50);
    assert!(p[1] > 200 && p[0] < 60 && p[2] < 60, "HEIC decodes to its colour, got {p:?}");
}

#[test]
fn gifs_still_and_animated() {
    let still = lavfi("green_still.gif", &["-f", "lavfi", "-i", "color=c=0x00FF00:s=100x100", "-frames:v", "1", "-f", "gif"]);
    let a = probe(&still).unwrap();
    assert_eq!(a.kind, MediaKind::Image, "a one-frame GIF is a picture");
    assert_eq!((a.width, a.height, a.duration), (100, 100, 0.0));
    let animated = lavfi("anim_64x48.gif", &["-f", "lavfi", "-i", "testsrc2=s=64x48:r=10:d=2", "-f", "gif"]);
    let a = probe(&animated).unwrap();
    assert_eq!(a.kind, MediaKind::Video, "an animated GIF is a silent video");
    assert_eq!((a.width, a.height), (64, 48));
    assert!((a.duration - 2.0).abs() < 0.11, "duration {}", a.duration);
    assert!((a.fps - 10.0).abs() < 0.1, "fps {}", a.fps);
    assert!(a.has_video && !a.has_audio);
}

#[test]
fn unusable_files_give_readable_errors() {
    let dir = tempfile::tempdir().unwrap();
    // Missing.
    let missing = dir.path().join("holiday.mov");
    let msg = format!("{:#}", probe(&missing).unwrap_err());
    assert!(msg.contains("not found") && msg.contains("holiday.mov"), "{msg}");
    // A folder.
    let msg = format!("{:#}", probe(dir.path()).unwrap_err());
    assert!(msg.contains("folder"), "{msg}");
    // Empty.
    let empty = dir.path().join("empty.mp4");
    std::fs::write(&empty, b"").unwrap();
    let msg = format!("{:#}", probe(&empty).unwrap_err());
    assert!(msg.contains("empty.mp4") && msg.contains("empty"), "{msg}");
    // Text.
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "Remember to record the intro again.\nAnd the outro.\n").unwrap();
    let msg = format!("{:#}", probe(&text).unwrap_err());
    assert!(msg.contains("notes.txt") && msg.contains("not a media file"), "{msg}");
    // Garbage with a media extension.
    let junk = dir.path().join("broken.mp4");
    std::fs::write(&junk, (0..20_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect::<Vec<u8>>()).unwrap();
    let msg = format!("{:#}", probe(&junk).unwrap_err());
    assert!(msg.contains("broken.mp4") && msg.contains("not a media file"), "{msg}");
    assert!(!msg.contains('\n'), "one line, no ffprobe dump: {msg}");
    // Truncated real file: either probes with sane values or fails readably, never panics.
    let real = std::fs::read(testsrc()).unwrap();
    let cut = dir.path().join("truncated.mp4");
    std::fs::write(&cut, &real[..real.len() / 3]).unwrap();
    match probe(&cut) {
        Ok(a) => assert!(a.duration >= 0.0 && a.duration < 10.0),
        Err(e) => assert!(format!("{e:#}").contains("truncated.mp4"), "{e:#}"),
    }
    // Unreadable (no permission).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.path().join("locked.mp4");
        std::fs::copy(testsrc(), &locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&locked).is_err() {
            let msg = format!("{:#}", probe(&locked).unwrap_err());
            assert!(msg.contains("locked.mp4") && msg.to_lowercase().contains("cannot read"), "{msg}");
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}

#[test]
fn file_names_with_spaces_quotes_and_unicode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Çağrı's take #2 (final) — 100%.mp4");
    std::fs::copy(testsrc(), &path).unwrap();
    let a = probe(&path).unwrap();
    assert_eq!(a.name, "Çağrı's take #2 (final) — 100%.mp4");
    assert_eq!((a.width, a.height), (320, 240));
    let f = frame_at(&path, 1.0, 160, 160).unwrap();
    assert_eq!((f.width, f.height), (160, 120));
    let p = peaks_with_cache_dir(&path, 20, &dir.path().join("cache")).unwrap();
    assert!((p.len() as i64 - 60).abs() <= 1);
    // And such a file exports.
    let (e, _) = editor_with_source(&path, None);
    let out = dir.path().join("Çıktı 'export' #1.mp4");
    export_to(&e, &settings(&e), &out);
    assert_eq!(info(&out).frames, 90);
}

// ---------------------------------------------------------------------------
// Waveform peaks
// ---------------------------------------------------------------------------

#[test]
fn peaks_of_silence_are_zero() {
    let cache = tempfile::tempdir().unwrap();
    let path = lavfi("silence_3s.wav", &["-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", "3", "-c:a", "pcm_s16le", "-f", "wav"]);
    let p = peaks_with_cache_dir(&path, 100, cache.path()).unwrap();
    assert_eq!(p.per_second, 100);
    assert_eq!(p.len(), 300, "3 s at 100 peaks per second");
    assert_eq!(p.min.len(), p.max.len());
    assert!(p.max.iter().all(|v| v.abs() < 1e-4) && p.min.iter().all(|v| v.abs() < 1e-4));
    assert_eq!(p.amplitude(0.0, 3.0), 0.0);
}

#[test]
fn peaks_of_a_full_scale_sine_are_one() {
    let cache = tempfile::tempdir().unwrap();
    // lavfi's sine is 1/8 full scale; volume=8 makes it full scale.
    let path = tone_wav("tone_440_full.wav", 440, 2.0, 8.0);
    let p = peaks_with_cache_dir(&path, 50, cache.path()).unwrap();
    assert_eq!(p.len(), 100, "2 s at 50 peaks per second");
    for (i, (lo, hi)) in p.min.iter().zip(&p.max).enumerate() {
        assert!(*hi > 0.97 && *hi <= 1.0, "max[{i}] = {hi}");
        assert!(*lo < -0.97 && *lo >= -1.0, "min[{i}] = {lo}");
    }
    assert!((p.duration() - 2.0).abs() < 1e-9);
}

#[test]
fn peaks_follow_the_envelope_and_match_the_duration() {
    let cache = tempfile::tempdir().unwrap();
    // 1 s quiet (0.1), 1 s silent, 1.5 s loud (0.8): 3.5 s, in an AAC/MP4 video file.
    let path = lavfi(
        "envelope.mp4",
        &[
            "-f", "lavfi", "-i", "aevalsrc=exprs='if(lt(t,1),0.1,if(lt(t,2),0,0.8))*sin(2*PI*500*t)':s=44100:d=3.5",
            "-f", "lavfi", "-i", "color=c=black:s=64x36:r=30:d=3.5", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-f", "mp4",
        ],
    );
    let asset = probe(&path).unwrap();
    for per_second in [10u32, 100, 441] {
        let p = peaks_with_cache_dir(&path, per_second, cache.path()).unwrap();
        let expected = (asset.duration * per_second as f64).round() as i64;
        assert!((p.len() as i64 - expected).abs() <= 2.max(per_second as i64 / 20), "{per_second}/s: {} peaks for {} s", p.len(), asset.duration);
        assert!((p.amplitude(0.2, 0.8) - 0.1).abs() < 0.02, "{per_second}/s quiet part: {}", p.amplitude(0.2, 0.8));
        assert!(p.amplitude(1.2, 1.8) < 0.01, "{per_second}/s silent part: {}", p.amplitude(1.2, 1.8));
        assert!((p.amplitude(2.2, 3.3) - 0.8).abs() < 0.04, "{per_second}/s loud part: {}", p.amplitude(2.2, 3.3));
        assert!(p.min.iter().zip(&p.max).all(|(lo, hi)| lo <= hi && *lo >= -1.0 && *hi <= 1.0));
        assert_eq!(p.amplitude(10.0, 11.0), 0.0, "outside the file");
    }
}

#[test]
fn stereo_is_mixed_down_and_peaks_are_cached() {
    let cache = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    // Left 0.8, right silent -> mono mix 0.4.
    let path = dir.path().join("left_only.wav");
    ff(&["-f", "lavfi", "-i", "aevalsrc=exprs='0.8*sin(2*PI*300*t)|0':s=48000:d=2", "-c:a", "pcm_s16le", path.to_str().unwrap()]);
    let first = peaks_with_cache_dir(&path, 25, cache.path()).unwrap();
    assert!((first.amplitude(0.5, 1.5) - 0.4).abs() < 0.03, "mono mixdown level {}", first.amplitude(0.5, 1.5));
    let files = listing(cache.path());
    assert_eq!(files.len(), 1, "one cache file: {files:?}");
    // A second call is served from the cache: prove it by making the cache say something else.
    let cache_file = cache.path().join(&files[0]);
    let mut bytes = std::fs::read(&cache_file).unwrap();
    let n = bytes.len();
    bytes[n - 4..].copy_from_slice(&0.123f32.to_le_bytes());
    std::fs::write(&cache_file, &bytes).unwrap();
    let second = peaks_with_cache_dir(&path, 25, cache.path()).unwrap();
    assert_eq!(second.max.last().copied(), Some(0.123), "second call should come from the cache");
    assert_eq!(second.max[..49], first.max[..49]);
    // Another resolution is a different cache entry.
    let other = peaks_with_cache_dir(&path, 50, cache.path()).unwrap();
    assert_eq!(other.len(), 100);
    assert_eq!(listing(cache.path()).len(), 2);
    // Changing the file invalidates its entry.
    std::thread::sleep(std::time::Duration::from_millis(20));
    ff(&["-f", "lavfi", "-i", "aevalsrc=exprs='0.2*sin(2*PI*300*t)|0.2*sin(2*PI*300*t)':s=48000:d=3", "-c:a", "pcm_s16le", path.to_str().unwrap()]);
    let changed = peaks_with_cache_dir(&path, 25, cache.path()).unwrap();
    assert_eq!(changed.len(), 75);
    assert!((changed.amplitude(0.5, 2.5) - 0.2).abs() < 0.02);
    // A corrupt cache file is ignored and rebuilt, not trusted.
    for f in listing(cache.path()) {
        std::fs::write(cache.path().join(f), b"garbage").unwrap();
    }
    let rebuilt = peaks_with_cache_dir(&path, 25, cache.path()).unwrap();
    assert_eq!(rebuilt, changed);
}

#[test]
fn peaks_errors_are_readable() {
    let cache = tempfile::tempdir().unwrap();
    let png = solid_png("green_100.png", "0x00FF00", 100, 100);
    let msg = format!("{:#}", peaks_with_cache_dir(&png, 50, cache.path()).unwrap_err());
    assert!(msg.contains("green_100.png") && msg.contains("audio"), "{msg}");
    assert!(peaks_with_cache_dir(std::path::Path::new("/nonexistent/x.wav"), 50, cache.path()).is_err());
    assert!(peaks_with_cache_dir(&tone_wav("tone_440_full.wav", 440, 2.0, 8.0), 0, cache.path()).is_err());
    assert!(listing(cache.path()).is_empty(), "failures must not leave cache files");
}

#[test]
fn default_peaks_cache_lives_under_the_core_cache_dir() {
    let path = tone_wav("tone_440_full.wav", 440, 2.0, 8.0);
    let p = mango_media::waveform::peaks(&path, 30).unwrap();
    assert_eq!(p.len(), 60);
    let dir = mango_core::tools::cache_dir().join("waveforms");
    assert!(dir.is_dir());
    assert!(std::fs::read_dir(&dir).unwrap().count() >= 1);
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

#[test]
fn frame_at_returns_the_frame_for_that_time() {
    let src = coded_source(10);
    for k in 0..10 {
        for t in [k as f64 * UNIT + 0.01, k as f64 * UNIT + 0.25, k as f64 * UNIT + 0.48] {
            let f = frame_at(&src, t, 640, 360).unwrap();
            assert_eq!((f.width, f.height), (CODED_W, CODED_H), "never enlarged");
            assert_eq!(f.rgba.len(), (CODED_W * CODED_H * 4) as usize);
            let p = f.pixel(32, 18);
            assert_eq!(unit_of_color([p[0], p[1], p[2]]), Some(k), "frame at {t}s");
            assert_eq!(p[3], 255);
        }
    }
    // Past the end: the last frame, not an error.
    let f = frame_at(&src, 60.0, 640, 360).unwrap();
    let p = f.pixel(32, 18);
    assert_eq!(unit_of_color([p[0], p[1], p[2]]), Some(9));
    // Negative and NaN times are clamped to the start.
    for t in [-3.0, f64::NAN] {
        let p = frame_at(&src, t, 640, 360).unwrap().pixel(1, 1);
        assert_eq!(unit_of_color([p[0], p[1], p[2]]), Some(0));
    }
}

#[test]
fn frame_at_scales_to_fit_keeping_aspect() {
    let src = testsrc(); // 320x240
    for ((mw, mh), expect) in [((160, 160), (160, 120)), ((1000, 60), (80, 60)), ((320, 240), (320, 240)), ((4000, 4000), (320, 240)), ((1, 1), (1, 1))] {
        let f = frame_at(&src, 1.0, mw, mh).unwrap();
        assert_eq!((f.width, f.height), expect, "fit into {mw}x{mh}");
        assert_eq!(f.rgba.len(), (f.width * f.height * 4) as usize);
    }
    assert!(frame_at(&src, 1.0, 0, 100).is_err());
}

#[test]
fn frame_at_for_images_returns_the_image_with_alpha() {
    let dir = tempfile::tempdir().unwrap();
    // A PNG with a transparent left half, written with our own encoder.
    let mut img = mango_media::Frame::transparent(40, 20);
    for y in 0..20u32 {
        for x in 20..40u32 {
            let i = ((y * 40 + x) * 4) as usize;
            img.rgba[i..i + 4].copy_from_slice(&[10, 200, 30, 255]);
        }
    }
    let path = dir.path().join("half.png");
    save_png(&img, &path).unwrap();
    assert_eq!(probe(&path).unwrap().kind, MediaKind::Image);
    for t in [0.0, 5.0] {
        let f = frame_at(&path, t, 100, 100).unwrap();
        assert_eq!(f, img, "the image itself, whatever the time ({t})");
    }
    let half = frame_at(&path, 0.0, 20, 20).unwrap();
    assert_eq!((half.width, half.height), (20, 10));
    assert_eq!(half.pixel(2, 5)[3], 0, "transparency survives scaling");
    assert_eq!(half.pixel(17, 5), [10, 200, 30, 255]);
}

#[test]
fn save_png_round_trips_and_rejects_bad_frames() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = mango_media::Frame::transparent(3, 2);
    f.rgba.copy_from_slice(&[255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 1, 2, 3, 4, 250, 251, 252, 253, 9, 8, 7, 255]);
    let path = dir.path().join("rt.png");
    save_png(&f, &path).unwrap();
    // Decode with ffmpeg, independently of our code.
    let raw = ff_out(&["-i", path.to_str().unwrap(), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]);
    assert_eq!(raw, f.rgba, "straight (non-premultiplied) RGBA is written as is");
    let bad = mango_media::Frame { width: 3, height: 2, rgba: vec![0; 5] };
    assert!(save_png(&bad, &dir.path().join("bad.png")).is_err());
    assert!(!dir.path().join("bad.png").exists());
    assert!(save_png(&f, &dir.path().join("no/such/dir.png")).is_err());
}

#[test]
fn frame_errors_are_readable() {
    let wav = tone_wav("tone_440_full.wav", 440, 2.0, 8.0);
    let msg = format!("{:#}", frame_at(&wav, 0.5, 100, 100).unwrap_err());
    assert!(msg.contains("tone_440_full.wav"), "{msg}");
    let msg = format!("{:#}", frame_at(std::path::Path::new("/nonexistent/clip.mp4"), 0.0, 10, 10).unwrap_err());
    assert!(msg.contains("not found") && msg.contains("clip.mp4"), "{msg}");
}

#[test]
fn tools_are_found() {
    let ffmpeg = mango_media::ffmpeg::ffmpeg_path().unwrap();
    let ffprobe = mango_media::ffmpeg::ffprobe_path().unwrap();
    assert!(ffmpeg.is_file() && ffprobe.is_file());
    assert_eq!(ffmpeg.file_name().unwrap(), "ffmpeg");
    assert_eq!(ffprobe.file_name().unwrap(), "ffprobe");
}

#[test]
fn run_reports_progress_errors_and_cancel() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.mkv");
    let args = |secs: &str, out: &std::path::Path| -> Vec<String> {
        ["-hide_banner", "-y", "-f", "lavfi", "-i", &format!("testsrc2=s=640x360:r=30:d={secs}"), "-c:v", "libx264", "-preset", "ultrafast", out.to_str().unwrap()].iter().map(|s| s.to_string()).collect()
    };
    // Progress: increasing timestamps ending near the duration.
    let cancel = AtomicBool::new(false);
    let mut times = Vec::new();
    mango_media::ffmpeg::run(&args("20", &out), &mut |t| times.push(t), &cancel).unwrap();
    assert!(!times.is_empty(), "progress should be reported");
    assert!(times.windows(2).all(|w| w[1] >= w[0]), "timestamps increase: {times:?}");
    assert!((times.last().unwrap() - 20.0).abs() < 0.2, "last progress is the end of the output: {times:?}");
    // Failure: error carries ffmpeg's own message.
    let bad: Vec<String> = ["-hide_banner", "-y", "-i", "/nonexistent/input.mp4", out.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
    let err = mango_media::ffmpeg::run(&bad, &mut |_| {}, &cancel).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("No such file or directory") && msg.contains("/nonexistent/input.mp4"), "{msg}");
    assert!(!mango_media::is_cancelled(&err));
    // A lot of stderr output must not deadlock (the pipe is drained).
    let noisy: Vec<String> = ["-hide_banner", "-y", "-v", "debug", "-f", "lavfi", "-i", "testsrc2=s=64x36:r=30:d=20", "-c:v", "libx264", "-preset", "ultrafast", out.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
    mango_media::ffmpeg::run(&noisy, &mut |_| {}, &cancel).unwrap();
    // Cancel: stops a job that would take minutes, promptly, with the cancel error.
    let slow: Vec<String> = ["-hide_banner", "-y", "-f", "lavfi", "-i", "testsrc2=s=1920x1080:r=30:d=3600", "-c:v", "libx264", "-preset", "slow", out.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let err = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(700));
            cancel.store(true, Ordering::SeqCst);
        });
        mango_media::ffmpeg::run(&slow, &mut |_| {}, &cancel).unwrap_err()
    });
    assert!(mango_media::is_cancelled(&err), "{err:#}");
    assert!(started.elapsed().as_secs_f64() < 5.0, "cancel took {:?}", started.elapsed());
    // The child is really gone: its output file stops growing.
    let size = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let a = size(&out);
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert_eq!(a, size(&out), "ffmpeg must be dead after cancel");
}
