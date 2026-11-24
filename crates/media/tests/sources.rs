//! Exports from awkward sources: long GOPs, other frame rates, containers
//! whose timestamps do not start at zero, mixed sources in one script.

mod common;
use common::*;
use mango_core::*;

const FRAME: f64 = 1.0 / 30.0;

/// Deletes units 3, 4 and 9 and checks the output unit by unit, including
/// the first and last frame of every surviving unit.
fn cut_and_check(path: &std::path::Path, units: usize, what: &str, exact_edges: bool) {
    let (mut e, _) = editor_with_source(path, Some(unit_transcript(units)));
    for k in [9usize, 4, 3] {
        let i = word_index(&e, &format!("w{k}"));
        e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    }
    let kept: Vec<usize> = (0..units).filter(|k| ![3, 4, 9].contains(k)).collect();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let mut s = settings(&e);
    s.fps = 30.0;
    export_to(&e, &s, &out);
    let inf = info(&out);
    // Containers report slightly different lengths (codec padding); the
    // composition length is the source of truth for the export.
    let total = e.comp().unwrap().duration();
    assert!((total - kept.len() as f64 * UNIT).abs() < 0.08, "{what}: composition length {total}");
    assert_eq!(inf.frames as usize, (total * 30.0).round() as usize, "{what}: frame count");
    assert!((inf.duration - total).abs() <= FRAME, "{what}: duration {} vs composition {total}", inf.duration);
    let v = decode_video(&out);
    let a = decode_audio(&out);
    // No frame may show the background: every one is some unit's picture.
    for f in 0..v.frames.len() {
        assert!(unit_of_color(v.mean(f)).is_some(), "{what}: output frame {f} is not source picture: {:?}", v.mean(f));
    }
    for (j, k) in kept.iter().enumerate() {
        let frames: &[usize] = if exact_edges { &[0, 7, 14] } else { &[2, 7, 12] };
        for off in frames {
            let f = j * 15 + off;
            assert_eq!(unit_of_color(v.mean(f)), Some(*k), "{what}: picture at output frame {f} (unit slot {j})");
        }
        let t = j as f64 * UNIT;
        assert_eq!(unit_heard(&a, t + 0.1, t + 0.4), Some(*k), "{what}: sound in unit slot {j}");
    }
}

#[test]
fn cuts_are_frame_accurate_in_long_gop_video_with_b_frames() {
    // One keyframe for the whole file: every cut lands far from a keyframe.
    let src = coded_variant(16, "coded_16_longgop.mp4", &["-c:v", "libx264", "-preset", "medium", "-crf", "12", "-g", "600", "-bf", "3", "-pix_fmt", "yuv420p", "-c:a", "copy", "-f", "mp4"]);
    cut_and_check(&src, 16, "long GOP", true);
}

#[test]
fn cuts_are_accurate_in_mpeg_ts_with_a_start_offset() {
    // Transport streams start at a non-zero timestamp (1.4 s by default).
    let src = coded_variant(16, "coded_16.ts", &["-c:v", "libx264", "-preset", "veryfast", "-crf", "12", "-g", "30", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-f", "mpegts"]);
    let j = ffprobe_json(&src, &[]);
    let start: f64 = j["format"]["start_time"].as_str().unwrap().parse().unwrap();
    assert!(start > 1.0, "fixture should have a start offset, has {start}");
    let asset = mango_media::probe::probe(&src).unwrap();
    assert!((asset.duration - 8.0).abs() < 0.1, "duration is the playable length, not the end timestamp: {}", asset.duration);
    // In this file the picture starts 21 ms after time zero (the AAC encoder
    // delay), so unit edges are not on output frames: check inside the units.
    cut_and_check(&src, 16, "MPEG-TS", false);
}

#[test]
fn cuts_are_accurate_in_matroska_and_mov() {
    let mkv = coded_variant(16, "coded_16.mkv", &["-c:v", "libx264", "-preset", "veryfast", "-crf", "12", "-pix_fmt", "yuv420p", "-c:a", "libopus", "-b:a", "128k", "-f", "matroska"]);
    cut_and_check(&mkv, 16, "Matroska/Opus", true);
    let mov = coded_variant(16, "coded_16_prores.mov", &["-c:v", "prores_ks", "-profile:v", "1", "-c:a", "pcm_s16le", "-f", "mov"]);
    cut_and_check(&mov, 16, "ProRes/PCM", true);
}

#[test]
fn source_at_another_frame_rate_keeps_duration_and_content() {
    // 25 fps source exported at 30: unit edges fall between output frames, so
    // only check frames safely inside each unit.
    let src = coded_variant(16, "coded_16_25fps.mp4", &["-vf", "fps=25", "-c:v", "libx264", "-preset", "veryfast", "-crf", "12", "-pix_fmt", "yuv420p", "-c:a", "copy", "-f", "mp4"]);
    let asset = mango_media::probe::probe(&src).unwrap();
    assert!((asset.fps - 25.0).abs() < 0.01);
    cut_and_check(&src, 16, "25 fps source", false);
}

#[test]
fn source_with_44k_stereo_audio_is_resampled_without_drift() {
    let src = coded_variant(124, "coded_124_44k.mp4", &["-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-ar", "44100", "-ac", "2", "-f", "mp4"]);
    let asset = mango_media::probe::probe(&src).unwrap();
    assert_eq!((asset.sample_rate, asset.channels), (44_100, 2));
    let (mut e, _) = editor_with_source(&src, Some(unit_transcript(124)));
    for k in (1..120).step_by(2).rev() {
        let i = word_index(&e, &format!("w{k}"));
        e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    export_to(&e, &settings(&e), &out);
    let a = decode_audio(&out);
    assert!((a.len() as f64 / RATE as f64 - 32.0).abs() <= FRAME, "audio length {}", a.len() as f64 / RATE as f64);
    let v = decode_video(&out);
    assert_eq!(v.frames.len(), 960);
    // After 60 cuts the last units are still where they belong, to the frame.
    for (j, k) in [(58usize, 116usize), (59, 118), (60, 120), (63, 123)] {
        let t = j as f64 * UNIT;
        assert_eq!(unit_heard(&a, t + 0.02, t + 0.12), Some(k), "start of slot {j}");
        assert_eq!(unit_heard(&a, t + 0.38, t + 0.48), Some(k), "end of slot {j}");
        assert_eq!(unit_of_color(v.mean(j * 15)), Some(k));
        assert_eq!(unit_of_color(v.mean(j * 15 + 14)), Some(k));
    }
}

#[test]
fn two_different_sources_in_one_script() {
    // First the coded source (64x36), then a 160x88 clip appended to the script.
    let (mut e, _) = coded_editor(4);
    let other = lavfi(
        "blue_160x88_tone.mp4",
        &["-f", "lavfi", "-i", "color=c=0x0000FF:s=160x88:r=24:d=2", "-f", "lavfi", "-i", "sine=f=3000:d=2:r=44100", "-c:v", "libx264", "-crf", "12", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest", "-f", "mp4"],
    );
    let id = add_asset(&mut e, &other);
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    let comp = e.comp().unwrap();
    assert_eq!((comp.width, comp.height), (64, 36), "the canvas stays as the first source set it");
    assert!((comp.duration() - 4.0).abs() < 0.05);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("two.mp4");
    export_to(&e, &settings(&e), &out);
    let inf = info(&out);
    assert_eq!((inf.width, inf.height), (64, 36));
    assert!((inf.duration - comp.duration()).abs() <= FRAME);
    let v = decode_video(&out);
    let a = decode_audio(&out);
    assert_eq!(unit_of_color(v.mean(52)), Some(3));
    let p = v.at(90, 0.5, 0.5);
    assert!(p[2] > 170 && p[0] < 90 && p[1] < 90, "second source's picture after the first ends, got {p:?}");
    assert!(tone_level(&a, 3000.0, 2.2, 3.8) > 0.05, "second source's sound");
    assert!(tone_level(&a, 3000.0, 0.2, 1.8) < 0.002, "and not before");
    assert_eq!(unit_heard(&a, 1.6, 1.9), Some(3));
}

#[test]
fn every_unit_cut_separately_spans_several_chunks() {
    // Reverse the order of 70 units: 70 clips, none contiguous, three chunks.
    let (mut e, _) = coded_editor(124);
    let n = e.comp().unwrap().script.len();
    e.apply(Command::DeleteRange { start: 70, end: n }).unwrap();
    for k in 0..69 {
        // Move the last token to position k.
        e.apply(Command::MoveRange { start: 69, end: 70, dest: k }).unwrap();
    }
    let order: Vec<String> = e.comp().unwrap().script.iter().map(|t| t.text.clone()).collect();
    assert_eq!(order[0], "w69");
    assert_eq!(order[69], "w0");
    let chunks = mango_core::render::plan_chunks(e.comp().unwrap(), &settings(&e));
    assert_eq!(chunks.len(), 3);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("reversed.mp4");
    export_to(&e, &settings(&e), &out);
    let v = decode_video(&out);
    let a = decode_audio(&out);
    assert_eq!(v.frames.len(), 70 * 15);
    for j in 0..70 {
        let k = 69 - j;
        assert_eq!(unit_of_color(v.mean(j * 15)), Some(k), "first frame of slot {j}");
        assert_eq!(unit_of_color(v.mean(j * 15 + 14)), Some(k), "last frame of slot {j}");
        assert_eq!(unit_heard(&a, j as f64 * UNIT + 0.1, j as f64 * UNIT + 0.4), Some(k), "sound of slot {j}");
    }
}

#[test]
fn cuts_between_frames_show_the_frame_that_was_on_screen() {
    // Keep 0 .. 1.21 s and 3.74 s .. end: neither cut is on a frame boundary.
    let (mut e, _) = coded_editor(16);
    e.apply(Command::SplitAt { time: 1.21 }).unwrap();
    e.apply(Command::SplitAt { time: 3.74 }).unwrap();
    let comp = e.comp().unwrap();
    let (a, b) = (comp.token_at_time(1.215).unwrap(), comp.token_at_time(3.745).unwrap());
    e.apply(Command::DeleteRange { start: a, end: b }).unwrap();
    let comp = e.comp().unwrap();
    assert!((comp.duration() - (8.0 - 2.53)).abs() < 1e-6, "{}", comp.duration());
    let clips = comp.script_clips();
    assert_eq!(clips.len(), 2);
    assert!((clips[1].src_start - 3.74).abs() < 1e-9);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("offgrid.mp4");
    export_to(&e, &settings(&e), &out);
    let v = decode_video(&out);
    let au = decode_audio(&out);
    assert_eq!(v.frames.len(), (5.47f64 * 30.0).round() as usize);
    // Clip 1 owns output frames 0..36, clip 2 starts on frame 36 with the
    // source frame on screen at 3.74 s (unit 7); unit 8 arrives 0.26 s later.
    let expected = |f: usize| -> usize {
        if f < 36 {
            f / 15
        } else {
            // Source frame shown: the one nearest at-or-before 3.74 s + (f - 36) frames.
            let src_frame = 112 + (f - 36);
            src_frame / 15
        }
    };
    for f in 0..v.frames.len() {
        assert_eq!(unit_of_color(v.mean(f)), Some(expected(f)), "output frame {f}");
    }
    // The sound cuts at exactly 1.21 s, from unit 2 to unit 7, and moves on to unit 8 at 1.47 s.
    assert_eq!(unit_heard(&au, 1.05, 1.2), Some(2));
    assert_eq!(unit_heard(&au, 1.22, 1.46), Some(7));
    assert_eq!(unit_heard(&au, 1.48, 1.9), Some(8));
    let level = |k: usize, t0: f64, t1: f64| tone_level(&au, unit_freq(k), t0, t1);
    assert!(level(2, 1.16, 1.205) > 0.3 && level(7, 1.215, 1.26) > 0.3, "cut is at 1.21 s");
    assert!(level(7, 1.16, 1.205) < 0.1 && level(2, 1.215, 1.26) < 0.1, "no bleed across the cut");
}

#[test]
fn audio_that_starts_late_in_its_file_stays_in_sync() {
    // Video from 0, audio delayed by 0.7 s inside the container.
    let src = fixture("coded_16_late_audio.mkv", |out| {
        let base = coded_source(16);
        ff(&["-i", base.to_str().unwrap(), "-itsoffset", "0.7", "-i", base.to_str().unwrap(), "-map", "0:v", "-map", "1:a", "-c", "copy", "-f", "matroska", out.to_str().unwrap()]);
    });
    let j = ffprobe_json(&src, &[]);
    let astart: f64 = j["streams"].as_array().unwrap().iter().find(|s| s["codec_type"] == "audio").unwrap()["start_time"].as_str().unwrap().parse().unwrap();
    assert!((astart - 0.7).abs() < 0.03, "fixture audio should start at 0.7 s, starts at {astart}");
    // No edits and one cut: in both, sound k is heard 0.7 s after picture k started.
    let (mut e, _) = editor_with_source(&src, None);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("late.mp4");
    export_to(&e, &settings(&e), &out);
    let v = decode_video(&out);
    let a = decode_audio(&out);
    assert_eq!(unit_of_color(v.mean(7)), Some(0));
    assert!(rms(&a, 0.05, 0.65) < 0.002, "silence until the audio stream starts, rms {}", rms(&a, 0.05, 0.65));
    assert_eq!(unit_heard(&a, 0.8, 1.1), Some(0), "unit 0's sound starts at 0.7 s");
    assert_eq!(unit_heard(&a, 3.3, 3.6), Some(5));
    // Cut out source 1.0 .. 4.0; what follows must keep the same relation.
    e.apply(Command::SplitAt { time: 1.0 }).unwrap();
    e.apply(Command::SplitAt { time: 4.0 }).unwrap();
    let comp = e.comp().unwrap();
    let (x, y) = (comp.token_at_time(1.01).unwrap(), comp.token_at_time(4.01).unwrap());
    e.apply(Command::DeleteRange { start: x, end: y }).unwrap();
    let out = dir.path().join("late_cut.mp4");
    export_to(&e, &settings(&e), &out);
    let v = decode_video(&out);
    let a = decode_audio(&out);
    // Output 1.0 s == source 4.0 s: picture unit 8; sound is source audio at 4.0 s = unit (4.0 - 0.7) / 0.5 = 6.
    assert_eq!(unit_of_color(v.mean(30)), Some(8));
    assert_eq!(unit_heard(&a, 1.02, 1.28), Some(6));
    assert_eq!(unit_heard(&a, 1.32, 1.78), Some(7));
}

/// Smallest and typical short-window level around `t`.
fn dip_at(a: &[f32], t: f64) -> (f64, f64) {
    let typical = rms(a, t - 0.3, t - 0.1);
    let mut lowest = f64::INFINITY;
    let mut x = t - 0.02;
    while x < t + 0.02 {
        lowest = lowest.min(rms(a, x, x + 0.002));
        x += 0.0005;
    }
    (lowest, typical)
}

#[test]
fn scene_breaks_that_are_not_cuts_do_not_dip_the_audio() {
    // A steady 1 kHz tone; a scene break at 1.0 s without removing anything.
    let src = solid_clip("blue_160x90.mp4", "0x0000FF", 160, 90, 4.0, 1000);
    let (mut e, _) = editor_with_source(&src, None);
    e.apply(Command::SplitAt { time: 1.0 }).unwrap();
    let at = e.comp().unwrap().token_at_time(1.01).unwrap();
    e.apply(Command::SetSceneBreak { at, on: true }).unwrap();
    assert_eq!(mango_core::render::plan_chunks(e.comp().unwrap(), &settings(&e)).len(), 2, "the scene break splits the export into two chunks");
    assert!(e.comp().unwrap().edit_boundaries().is_empty(), "there is no cut");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("scene.wav");
    let mut s = settings(&e);
    s.video = None;
    s.audio = Some(mango_core::render::AudioCodec::Wav);
    export_to(&e, &s, &out);
    let a = decode_audio(&out);
    let (lowest, typical) = dip_at(&a, 1.0);
    assert!(typical > 0.05);
    assert!(lowest > typical * 0.9, "the tone must run straight through the scene break: level falls to {lowest:.4} from {typical:.4}");
    // A real cut at 2.0 s (0.5 s removed) still gets its click-preventing fade.
    e.apply(Command::SplitAt { time: 2.0 }).unwrap();
    e.apply(Command::SplitAt { time: 2.5 }).unwrap();
    let comp = e.comp().unwrap();
    let (x, y) = (comp.token_at_time(2.01).unwrap(), comp.token_at_time(2.51).unwrap());
    e.apply(Command::DeleteRange { start: x, end: y }).unwrap();
    export_to(&e, &s, &out);
    let a = decode_audio(&out);
    let (at_break, typical) = dip_at(&a, 1.0);
    let (at_cut, _) = dip_at(&a, 2.0);
    assert!(at_break > typical * 0.9, "still no dip at the scene break: {at_break:.4} of {typical:.4}");
    assert!(at_cut < typical * 0.5, "a real cut is faded: {at_cut:.4} of {typical:.4}");
}

#[test]
fn mono_sources_keep_their_level_in_stereo_exports() {
    // The coded source is mono at amplitude 0.5; a mono music layer at 0.25.
    let (mut e, _) = coded_editor(8);
    assert_eq!(e.project.assets[0].channels, 1);
    let i = word_index(&e, "w4");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 4 }).unwrap();
    let music = lavfi("tone_2000_mono_quarter.wav", &["-f", "lavfi", "-i", "aevalsrc=0.25*sin(2*PI*2000*t):s=48000:d=4", "-c:a", "pcm_s16le", "-f", "wav"]);
    let id = add_asset(&mut e, &music);
    assert_eq!(e.project.asset(id).unwrap().channels, 1);
    e.apply(Command::AddLayer { layer: Layer::new("Music", 2.0, 2.0, LayerContent::Audio(MediaClip::new(id))) }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("mono.wav");
    let mut s = settings(&e);
    s.video = None;
    s.audio = Some(mango_core::render::AudioCodec::Wav);
    export_to(&e, &s, &out);
    for ch in 0..2 {
        let a = decode_audio_channel(&out, ch);
        let speech = tone_level(&a, unit_freq(1), 0.55, 0.95);
        let layer = tone_level(&a, 2000.0, 2.2, 3.8);
        assert!((speech - 0.5).abs() < 0.015, "channel {ch}: script audio at source level 0.5, got {speech:.3}");
        assert!((layer - 0.25).abs() < 0.01, "channel {ch}: layer audio at source level 0.25, got {layer:.3}");
    }
    // A stereo source is passed through channel for channel.
    let stereo = lavfi("left_only_stereo.wav", &["-f", "lavfi", "-i", "aevalsrc=exprs='0.4*sin(2*PI*700*t)|0':s=48000:d=2", "-c:a", "pcm_s16le", "-f", "wav"]);
    let (e2, _) = editor_with_source(&stereo, None);
    let out2 = dir.path().join("stereo.wav");
    let mut s2 = settings(&e2);
    s2.video = None;
    s2.audio = Some(mango_core::render::AudioCodec::Wav);
    export_to(&e2, &s2, &out2);
    let (l, r) = (decode_audio_channel(&out2, 0), decode_audio_channel(&out2, 1));
    assert!((tone_level(&l, 700.0, 0.5, 1.5) - 0.4).abs() < 0.01);
    assert!(tone_level(&r, 700.0, 0.5, 1.5) < 0.002);
}
