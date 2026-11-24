//! End-to-end export tests: real ffmpeg, real files, and assertions on what
//! is actually inside the exported file.

mod common;
use common::*;
use mango_core::render::*;
use mango_core::*;
use mango_media::export::{Progress, Stage, export, export_with_work_root};
use std::sync::atomic::{AtomicBool, Ordering};

const FRAME: f64 = 1.0 / 30.0;

/// Export settings at 320x180, big enough to locate graphics, small enough to be quick.
fn small(e: &Editor) -> ExportSettings {
    let mut s = settings(e);
    s.width = 320;
    s.height = 180;
    s
}

fn is_blue(p: [u8; 3]) -> bool {
    p[2] > 170 && p[0] < 90 && p[1] < 90
}
fn is_green(p: [u8; 3]) -> bool {
    p[1] > 170 && p[0] < 90 && p[2] < 90
}

/// Time at which the sound changes from unit `from` to unit `to`, searching
/// `[t0, t1)` with 1 ms resolution.
fn sound_change(a: &[f32], from: usize, to: usize, t0: f64, t1: f64) -> f64 {
    let win = 0.006;
    let mut t = t0;
    while t < t1 {
        if tone_level(a, unit_freq(to), t, t + win) > tone_level(a, unit_freq(from), t, t + win) {
            return t + win / 2.0;
        }
        t += 0.001;
    }
    panic!("sound never changed from unit {from} to unit {to} in {t0}..{t1}");
}

#[test]
fn uncut_export_has_exact_duration_frames_resolution_and_codecs() {
    let (e, _) = coded_editor(10);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.mp4");
    let report = export_to(&e, &settings(&e), &out);
    let i = info(&out);
    assert_eq!(report.path, out);
    assert_eq!(report.bytes, std::fs::metadata(&out).unwrap().len());
    assert!((report.duration - 5.0).abs() <= FRAME, "reported duration {}", report.duration);
    assert_eq!((i.width, i.height), (CODED_W, CODED_H));
    assert_eq!(i.vcodec.as_deref(), Some("h264"));
    assert_eq!(i.acodec.as_deref(), Some("aac"));
    assert_eq!(i.pix_fmt, "yuv420p");
    assert_eq!(i.frames, 150, "5 s at 30 fps");
    assert!((i.fps - 30.0).abs() < 0.01);
    assert!((i.duration - 5.0).abs() <= FRAME, "duration {}", i.duration);
    assert_eq!((i.sample_rate, i.channels), (48_000, 2));
    // Every unit plays, in order, in both picture and sound; cuts between
    // units land on the exact frame.
    let v = decode_video(&out);
    let a = decode_audio(&out);
    assert_eq!(v.frames.len(), 150);
    for k in 0..10 {
        let t = k as f64 * UNIT;
        for f in [k * 15, k * 15 + 7, k * 15 + 14] {
            assert_eq!(unit_of_color(v.mean(f)), Some(k), "picture at frame {f}");
        }
        assert_eq!(unit_heard(&a, t + 0.1, t + 0.4), Some(k), "sound at {t}s");
    }
    assert_eq!(listing(dir.path()), ["out.mp4"], "nothing but the export is left behind");
}

#[test]
fn deleting_a_word_removes_exactly_that_source_media() {
    let (mut e, _) = coded_editor(10);
    let i = word_index(&e, "w3");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cut.mp4");
    export_to(&e, &settings(&e), &out);
    let inf = info(&out);
    assert_eq!(inf.frames, 135, "4.5 s at 30 fps");
    assert!((inf.duration - 4.5).abs() <= FRAME, "duration {}", inf.duration);
    let v = decode_video(&out);
    let a = decode_audio(&out);
    assert!((a.len() as f64 / RATE as f64 - 4.5).abs() <= FRAME, "audio length {}", a.len() as f64 / RATE as f64);
    for j in 0..9 {
        let expect = if j < 3 { j } else { j + 1 };
        for f in [j * 15, j * 15 + 7, j * 15 + 14] {
            assert_eq!(unit_of_color(v.mean(f)), Some(expect), "picture at output frame {f}");
        }
        let t = j as f64 * UNIT;
        assert_eq!(unit_heard(&a, t + 0.1, t + 0.4), Some(expect), "sound at output {t}s");
    }
    // Unit 3 is nowhere in the output.
    assert!((0..v.frames.len()).all(|f| unit_of_color(v.mean(f)) != Some(3)), "the deleted unit's picture must be gone");
    assert!(tone_level(&a, unit_freq(3), 0.0, 4.5) < 0.01, "the deleted unit's sound must be gone");
}

#[test]
fn gap_clip_is_black_and_silent() {
    let (mut e, _) = coded_editor(10);
    let i = word_index(&e, "w4");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 2 }).unwrap(); // units 4 and 5 -> 1 s gap at 2.0
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("gap.mp4");
    export_to(&e, &settings(&e), &out);
    let inf = info(&out);
    assert_eq!(inf.frames, 150, "a gap keeps the duration");
    let v = decode_video(&out);
    let a = decode_audio(&out);
    for f in 60..90 {
        assert_color(v.mean(f), [0, 0, 0], 6, &format!("gap frame {f}"));
    }
    assert!(rms(&a, 2.02, 2.98) < 0.0005, "gap must be silent, rms {}", rms(&a, 2.02, 2.98));
    // Its neighbours are intact, right up to the edge.
    assert_eq!(unit_of_color(v.mean(59)), Some(3));
    assert_eq!(unit_of_color(v.mean(90)), Some(6));
    assert_eq!(unit_heard(&a, 1.6, 1.95), Some(3));
    assert_eq!(unit_heard(&a, 3.05, 3.4), Some(6));
}

#[test]
fn audio_and_video_stay_in_sync_after_sixty_cuts() {
    let (mut e, _) = coded_editor(124);
    // Delete every odd unit from 119 down to 1: 60 cuts.
    for k in (1..120).step_by(2).rev() {
        let i = word_index(&e, &format!("w{k}"));
        e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    }
    let comp = e.comp().unwrap();
    assert_eq!(comp.edit_boundaries().len(), 60);
    assert_eq!(comp.script_clips().len(), 61);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sync.mp4");
    export_to(&e, &settings(&e), &out);
    let inf = info(&out);
    assert_eq!(inf.frames, 64 * 15, "64 units of 15 frames survive");
    assert!((inf.duration - 32.0).abs() <= FRAME, "duration {}", inf.duration);
    let v = decode_video(&out);
    let a = decode_audio(&out);
    assert!((a.len() as f64 / RATE as f64 - 32.0).abs() <= FRAME, "audio length {}", a.len() as f64 / RATE as f64);
    // Kept units, in output order.
    let kept: Vec<usize> = (0..120).step_by(2).chain(120..124).collect();
    assert_eq!(kept.len(), 64);
    for (j, k) in kept.iter().enumerate() {
        assert_eq!(unit_of_color(v.mean(j * 15 + 7)), Some(*k), "picture in output unit {j}");
        let t = j as f64 * UNIT;
        assert_eq!(unit_heard(&a, t + 0.1, t + 0.4), Some(*k), "sound in output unit {j}");
    }
    // At every cut, and in particular the last ones, the picture and the
    // sound change within one frame of each other.
    for j in [1usize, 20, 40, 58, 59, 60, 61, 62, 63] {
        let (from, to) = (kept[j - 1], kept[j]);
        let first_frame = (0..v.frames.len()).find(|f| unit_of_color(v.mean(*f)) == Some(to)).expect("unit shows");
        let picture_t = first_frame as f64 / 30.0;
        let expected_t = j as f64 * UNIT;
        let sound_t = sound_change(&a, from, to, expected_t - 0.2, expected_t + 0.2);
        assert!((picture_t - expected_t).abs() < 1e-6, "cut {j}: picture changes at {picture_t}, expected {expected_t}");
        assert!((sound_t - picture_t).abs() <= FRAME, "cut {j}: sound changes at {sound_t}, picture at {picture_t}");
    }
}

#[test]
fn text_layer_shows_in_its_box_only_during_its_time_range() {
    let (mut e, _) = coded_editor(10);
    let mut layer = Layer::new(
        "Title",
        1.0,
        2.0,
        LayerContent::Text(TextStyle {
            text: "MANGO".into(),
            size: 0.22,
            color: Rgba::rgb(0, 0, 255),
            shadow: false,
            ..TextStyle::default()
        }),
    );
    layer.transform = Transform { x: 0.5, y: 0.25, w: 0.8, h: 0.3, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("text.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    assert_eq!(v.frames.len(), 150);
    let anywhere = |f: usize| v.count_in(f, 0.0, 0.0, 1.0, 1.0, is_blue);
    // Visible for exactly frames 30..90 (1.0 s to 3.0 s).
    for f in [0, 15, 29, 90, 91, 120, 149] {
        assert_eq!(anywhere(f), 0, "no text expected on frame {f}");
    }
    for f in [30, 31, 60, 88, 89] {
        let inside = v.count_in(f, 0.1, 0.1, 0.9, 0.4, is_blue);
        assert!(inside > 400, "text expected in its box on frame {f}, found {inside} px");
        assert_eq!(anywhere(f), inside, "text must stay inside its box on frame {f}");
    }
    // Horizontally centred on x = 0.5, vertically on y = 0.25.
    let (x0, y0, x1, y1) = v.bbox(60, is_blue).unwrap();
    let (cx, cy) = ((x0 + x1) as f64 / 2.0 / 320.0, (y0 + y1) as f64 / 2.0 / 180.0);
    assert!((cx - 0.5).abs() < 0.03, "text centre x {cx}");
    assert!((cy - 0.25).abs() < 0.05, "text centre y {cy}");
    // The script picture is untouched elsewhere.
    assert_eq!(unit_of_color(v.at(60, 0.5, 0.8)), Some(4));
}

#[test]
fn text_layer_fades_in() {
    let (mut e, _) = coded_editor(10);
    let mut layer = Layer::new(
        "Title",
        1.0,
        3.0,
        LayerContent::Text(TextStyle { text: "HHHH".into(), size: 0.3, color: Rgba::rgb(0, 0, 255), shadow: false, ..TextStyle::default() }),
    );
    layer.fade_in = 1.0;
    e.apply(Command::AddLayer { layer }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("fade.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    // The bluest pixel on the frame gets bluer as the fade progresses.
    let peak_blue = |f: usize| v.frames[f].chunks_exact(3).map(|p| p[2] as i32 - p[0] as i32).max().unwrap();
    let (early, mid, full) = (peak_blue(33), peak_blue(45), peak_blue(75));
    assert!(early < mid && mid < full, "fade should ramp: {early} < {mid} < {full}");
    assert!(full > 200, "fully faded in text is solid blue, got {full}");
    assert!(early < 80, "text is nearly transparent at the start of its fade, got {early}");
}

#[test]
fn captions_follow_word_times_and_highlight_the_active_word() {
    let (mut e, _) = coded_editor(10);
    // Units 4..6 become a 1.5 s gap: a pause that splits the captions.
    let i = word_index(&e, "w4");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 3 }).unwrap();
    let captions = CaptionSettings {
        enabled: true,
        size: 0.16,
        color: Rgba::rgb(0, 255, 0),
        highlight_color: Rgba::rgb(0, 0, 255),
        highlight: CaptionHighlight::ActiveWord,
        outline: None,
        y: 0.75,
        max_chars_per_line: 12,
        max_lines: 1,
        ..CaptionSettings::default()
    };
    e.apply(Command::SetCaptions { captions }).unwrap();
    let cues = mango_media::raster::caption_cues(e.comp().unwrap());
    assert!(cues.len() >= 2 && cues[0].words.len() >= 2, "test needs multi-word cues, got {cues:?}");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("captions.mp4");
    let mut s = settings(&e);
    s.width = 640;
    s.height = 360;
    export_to(&e, &s, &out);
    let v = decode_video(&out);
    let band = |f: usize, pred: fn([u8; 3]) -> bool| v.count_in(f, 0.0, 0.55, 1.0, 0.95, pred);
    for cue in &cues {
        let mut last_x: Option<f64> = None;
        for (wi, word) in cue.words.iter().enumerate() {
            let mid = (word.start + word.end) / 2.0;
            let f = (mid * 30.0) as usize;
            let active = band(f, is_blue);
            let others = band(f, is_green);
            assert!(active > 60, "active word {:?} should be highlighted at {mid}s (frame {f}), found {active} px", word.text);
            if cue.words.len() > 1 {
                assert!(others > 60, "the other words of the cue should show in the base colour at {mid}s, found {others} px");
            }
            // Captions only in their band.
            assert_eq!(v.count_in(f, 0.0, 0.0, 1.0, 0.5, |p| is_blue(p) || is_green(p)), 0, "no caption pixels in the top half");
            // The highlight moves along the line from word to word.
            let (x0, _, x1, _) = v.bbox(f, is_blue).unwrap();
            let cx = (x0 + x1) as f64 / 2.0;
            if let Some(prev) = last_x {
                assert!(cx > prev + 10.0, "highlight should move right within a line: word {wi} at x={cx}, previous at x={prev}");
            }
            last_x = Some(cx);
        }
    }
    // In the middle of the pause no caption shows (cues end 0.25 s after their last word).
    let gap_frame = (2.9 * 30.0) as usize;
    assert_eq!(band(gap_frame, is_blue) + band(gap_frame, is_green), 0, "no caption during the pause");
    // The first cue is up from the very first frame, the last one until the end.
    assert!(band(0, is_blue) > 60);
    assert!(band(v.frames.len() - 2, is_blue) > 60);
}

#[test]
fn reveal_captions_show_words_as_they_are_spoken() {
    let (mut e, _) = coded_editor(6);
    let captions = CaptionSettings {
        enabled: true,
        size: 0.16,
        color: Rgba::rgb(0, 255, 0),
        highlight: CaptionHighlight::Reveal,
        outline: None,
        y: 0.75,
        max_chars_per_line: 20,
        max_lines: 1,
        ..CaptionSettings::default()
    };
    e.apply(Command::SetCaptions { captions }).unwrap();
    let cues = mango_media::raster::caption_cues(e.comp().unwrap());
    assert!(cues[0].words.len() >= 3);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("reveal.mp4");
    let mut s = settings(&e);
    s.width = 640;
    s.height = 360;
    export_to(&e, &s, &out);
    let v = decode_video(&out);
    let mut last = 0usize;
    let mut last_right = 0usize;
    for word in &cues[0].words {
        let f = ((word.start + word.end) / 2.0 * 30.0) as usize;
        let n = v.count_in(f, 0.0, 0.5, 1.0, 1.0, is_green);
        let (_, _, right, _) = v.bbox(f, is_green).unwrap();
        assert!(n > last + 40, "each spoken word adds ink: {n} after {last}");
        assert!(right > last_right + 10, "each spoken word extends the line to the right");
        last = n;
        last_right = right;
    }
}

#[test]
fn image_and_video_layers_are_placed_where_their_transform_says() {
    let (mut e, _) = coded_editor(10);
    let png = solid_png("green_100.png", "0x00FF00", 100, 100);
    let clip = solid_clip("blue_160x90.mp4", "0x0000FF", 160, 90, 4.0, 1000);
    let img_id = add_asset(&mut e, &png);
    let vid_id = add_asset(&mut e, &clip);
    assert_eq!(e.project.asset(img_id).unwrap().kind, MediaKind::Image);

    // Image: box 80x90 centred at (80, 90); a square image fits as 80x80.
    let mut image = Layer::new("Logo", 1.0, 2.0, LayerContent::Image(ImageClip { asset: img_id, crop: Crop::default(), fit: FitMode::Fit, effects: vec![], zoom_to: 1.0 }));
    image.transform = Transform { x: 0.25, y: 0.5, w: 0.25, h: 0.5, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer: image }).unwrap();
    // Video: box 128x72 centred at (240, 45); 16:9 fills it exactly.
    let mut clip_settings = MediaClip::new(vid_id);
    clip_settings.audio.muted = true;
    let mut video = Layer::new("B-roll", 1.0, 2.0, LayerContent::Video(clip_settings));
    video.transform = Transform { x: 0.75, y: 0.25, w: 0.4, h: 0.4, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer: video }).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("layers.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    let near = |a: usize, b: usize| (a as i64 - b as i64).abs() <= 2;
    for f in [30, 60, 89] {
        let (x0, y0, x1, y1) = v.bbox(f, is_green).unwrap_or_else(|| panic!("image layer missing on frame {f}"));
        assert!(near(x0, 40) && near(x1, 119) && near(y0, 50) && near(y1, 129), "image box on frame {f}: {:?}", (x0, y0, x1, y1));
        let (x0, y0, x1, y1) = v.bbox(f, is_blue).unwrap_or_else(|| panic!("video layer missing on frame {f}"));
        assert!(near(x0, 176) && near(x1, 303) && near(y0, 9) && near(y1, 80), "video box on frame {f}: {:?}", (x0, y0, x1, y1));
        // Solid inside.
        assert!(is_green(v.pixel(f, 80, 90)));
        assert!(is_blue(v.pixel(f, 240, 45)));
    }
    for f in [0, 29, 90, 149] {
        assert_eq!(v.count_in(f, 0.0, 0.0, 1.0, 1.0, |p| is_green(p) || is_blue(p)), 0, "layers must not show on frame {f}");
    }
    // The muted b-roll's 1 kHz tone is not in the mix.
    let a = decode_audio(&out);
    assert!(tone_level(&a, 1000.0, 1.2, 2.8) < 0.005);
}

#[test]
fn stretch_and_fill_fit_modes_cover_the_box() {
    let (mut e, _) = coded_editor(4);
    let png = solid_png("green_100.png", "0x00FF00", 100, 100);
    let id = add_asset(&mut e, &png);
    for (fit, x) in [(FitMode::Stretch, 0.25), (FitMode::Fill, 0.75)] {
        let mut l = Layer::new("img", 0.0, 2.0, LayerContent::Image(ImageClip { asset: id, crop: Crop::default(), fit, effects: vec![], zoom_to: 1.0 }));
        l.transform = Transform { x, y: 0.5, w: 0.25, h: 0.5, rotation_deg: 0.0, opacity: 1.0 };
        e.apply(Command::AddLayer { layer: l }).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("fit.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    let near = |a: usize, b: usize| (a as i64 - b as i64).abs() <= 2;
    // Both boxes are 80x90: at x 40..120 and 200..280, y 45..135.
    let left = v.count_in(30, 0.0, 0.0, 0.5, 1.0, is_green);
    let right = v.count_in(30, 0.5, 0.0, 1.0, 1.0, is_green);
    assert!((left as i64 - 80 * 90).abs() < 500, "stretched image should cover its 80x90 box, covers {left} px");
    assert!((right as i64 - 80 * 90).abs() < 500, "filled image should cover its 80x90 box, covers {right} px");
    let (x0, y0, x1, y1) = v.bbox(30, is_green).unwrap();
    assert!(near(x0, 40) && near(x1, 279) && near(y0, 45) && near(y1, 134), "{:?}", (x0, y0, x1, y1));
}

#[test]
fn static_gif_image_layer_exports() {
    // ffmpeg cannot loop a GIF with `-loop 1`; the exporter converts it.
    let (mut e, _) = coded_editor(4);
    let gif = lavfi("green_still.gif", &["-f", "lavfi", "-i", "color=c=0x00FF00:s=100x100", "-frames:v", "1", "-f", "gif"]);
    let id = add_asset(&mut e, &gif);
    assert_eq!(e.project.asset(id).unwrap().kind, MediaKind::Image);
    let mut l = Layer::new("gif", 0.5, 1.0, LayerContent::Image(ImageClip { asset: id, crop: Crop::default(), fit: FitMode::Stretch, effects: vec![], zoom_to: 1.0 }));
    l.transform = Transform { x: 0.5, y: 0.5, w: 0.5, h: 0.5, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer: l }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("gif_layer.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    let n = v.count_in(30, 0.0, 0.0, 1.0, 1.0, is_green);
    assert!((n as i64 - 160 * 90).abs() < 700, "GIF should cover its 160x90 box, covers {n} px");
    assert_eq!(v.count_in(5, 0.0, 0.0, 1.0, 1.0, is_green), 0);
    assert_eq!(listing(dir.path()), ["gif_layer.mp4"]);
}

#[test]
fn scene_transitions_keep_the_total_duration_and_blend_scenes() {
    for transition in Transition::ALL {
        let (mut e, _) = coded_editor(10);
        let i = word_index(&e, "w5");
        e.apply(Command::SetSceneBreak { at: i, on: true }).unwrap();
        e.apply(Command::SetScene { scene: 1, settings: SceneSettings { transition, transition_secs: 0.5, ..SceneSettings::default() } }).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("scenes.mp4");
        export_to(&e, &settings(&e), &out);
        let inf = info(&out);
        assert_eq!(inf.frames, 150, "{transition:?} must not change the frame count");
        assert!((inf.duration - 5.0).abs() <= FRAME, "{transition:?}: duration {}", inf.duration);
        let v = decode_video(&out);
        let a = decode_audio(&out);
        assert!((a.len() as f64 / RATE as f64 - 5.0).abs() <= FRAME, "{transition:?}: audio length");
        // Before the transition window and after the scene start the picture is pure.
        assert_eq!(unit_of_color(v.mean(52)), Some(3), "{transition:?}");
        assert_eq!(unit_of_color(v.mean(59)), Some(3), "{transition:?}: last frame before the transition window");
        assert_eq!(unit_of_color(v.mean(76)), Some(5), "{transition:?}: scene 2 is fully in after its start");
        assert_eq!(unit_of_color(v.mean(149)), Some(9), "{transition:?}");
        // The sound is not shifted by the transition.
        assert_eq!(unit_heard(&a, 2.1, 2.4), Some(4), "{transition:?}");
        assert_eq!(unit_heard(&a, 2.6, 2.9), Some(5), "{transition:?}");
        let mid = v.mean(67); // 2.25 s: halfway through the transition into scene 2
        let (from, to) = (unit_color(4), unit_color(5));
        match transition {
            Transition::Cut => assert_eq!(unit_of_color(mid), Some(4), "a cut has no transition window"),
            Transition::CrossDissolve => {
                let avg = [((from[0] as u16 + to[0] as u16) / 2) as u8, ((from[1] as u16 + to[1] as u16) / 2) as u8, ((from[2] as u16 + to[2] as u16) / 2) as u8];
                assert_color(mid, avg, 14, "cross dissolve midpoint");
            }
            Transition::FadeThroughBlack => {
                let lum = |c: [u8; 3]| c[0] as u32 + c[1] as u32 + c[2] as u32;
                assert!(lum(mid) < lum(from) / 2 && lum(mid) < lum(to) / 2, "fade through black is dark at its midpoint: {mid:?}");
            }
            Transition::WipeLeft | Transition::WipeRight | Transition::SlideLeft | Transition::SlideUp => {
                // Half the frame shows each scene.
                let n_from = v.count_in(67, 0.0, 0.0, 1.0, 1.0, |p| unit_of_color(p) == Some(4));
                let n_to = v.count_in(67, 0.0, 0.0, 1.0, 1.0, |p| unit_of_color(p) == Some(5));
                let total = (CODED_W * CODED_H) as usize;
                assert!(n_from > total / 4 && n_to > total / 4, "{transition:?}: both scenes visible mid-transition ({n_from} + {n_to} of {total})");
            }
        }
    }
}

fn music_project(duck: Option<Ducking>) -> Editor {
    let (mut e, _) = coded_editor(12);
    // Units 4..8 become 2 s of silence at 2.0..4.0.
    let i = word_index(&e, "w4");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 4 }).unwrap();
    let music = tone_wav("tone_2000_quarter.wav", 2000, 8.0, 0.25);
    let id = add_asset(&mut e, &music);
    let mut clip = MediaClip::new(id);
    clip.duck = duck;
    let layer = Layer::new("Music", 0.5, 5.0, LayerContent::Audio(clip));
    e.apply(Command::AddLayer { layer }).unwrap();
    e
}

#[test]
fn layer_audio_is_mixed_at_its_position() {
    let e = music_project(None);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("mix.m4a");
    let mut s = settings(&e);
    s.video = None;
    export_to(&e, &s, &out);
    let a = decode_audio(&out);
    assert!((a.len() as f64 / RATE as f64 - 6.0).abs() < 0.03);
    // The source file's sine is 0.25 * 1/8 full scale (lavfi sine is 1/8) => measure relative to itself.
    let level = tone_level(&a, 2000.0, 1.0, 1.9);
    assert!(level > 0.01, "music should be audible under speech, level {level}");
    assert!(tone_level(&a, 2000.0, 0.05, 0.45) < level * 0.02, "music must not start before its layer does");
    assert!(tone_level(&a, 2000.0, 5.6, 5.95) < level * 0.02, "music must stop when its layer ends");
    let in_gap = tone_level(&a, 2000.0, 2.5, 3.9);
    assert!((in_gap / level - 1.0).abs() < 0.05, "without ducking the music level is constant: {level} vs {in_gap}");
    // Speech is still there, and the gap has only music.
    assert_eq!(unit_of_freq(unit_freq(2)), Some(2));
    assert!(tone_level(&a, unit_freq(2), 1.05, 1.45) > 0.3);
    assert!(tone_level(&a, unit_freq(5), 2.5, 3.0) < 0.005);
}

#[test]
fn ducking_lowers_layer_audio_under_speech() {
    let dir = tempfile::tempdir().unwrap();
    let level = |duck: Option<Ducking>, name: &str| {
        let e = music_project(duck);
        let out = dir.path().join(name);
        let mut s = settings(&e);
        s.video = None;
        s.audio = Some(AudioCodec::Wav);
        export_to(&e, &s, &out);
        let a = decode_audio(&out);
        (tone_level(&a, 2000.0, 1.0, 1.9), tone_level(&a, 2000.0, 3.2, 3.9))
    };
    let (plain_speech, plain_gap) = level(None, "plain.wav");
    let (duck_speech, duck_gap) = level(Some(Ducking { amount_db: 12.0, attack_ms: 20.0, release_ms: 200.0 }), "duck.wav");
    let db = |x: f64, y: f64| 20.0 * (x / y).log10();
    let reduction = db(plain_speech, duck_speech);
    assert!(reduction > 6.0, "music should drop clearly under speech, dropped {reduction:.1} dB");
    assert!(db(plain_gap, duck_gap).abs() < 1.5, "music returns to full level in silence: {:.1} dB off", db(plain_gap, duck_gap));
    assert!(db(duck_gap, duck_speech) > 6.0, "ducked music is lower under speech than in the pause");
}

#[test]
fn audio_only_exports_produce_the_right_kind_of_file() {
    let (mut e, _) = coded_editor(10);
    let i = word_index(&e, "w3");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    for (codec, ext, codec_name, format, tolerance) in [
        (AudioCodec::Mp3, "mp3", "mp3", "mp3", 0.06),
        (AudioCodec::Wav, "wav", "pcm_s16le", "wav", 0.001),
        (AudioCodec::Aac, "m4a", "aac", "mov,mp4,m4a,3gp,3g2,mj2", 0.03),
        (AudioCodec::Flac, "flac", "flac", "flac", 0.001),
        (AudioCodec::Opus, "opus", "opus", "ogg", 0.03),
    ] {
        let mut s = settings(&e);
        s.video = None;
        s.audio = Some(codec);
        assert_eq!(s.extension(), ext);
        let out = dir.path().join(format!("audio.{ext}"));
        export_to(&e, &s, &out);
        let inf = info(&out);
        assert_eq!(inf.vcodec, None, "{ext}: audio-only export must have no video stream");
        assert_eq!(inf.acodec.as_deref(), Some(codec_name), "{ext}");
        assert_eq!(inf.format, format, "{ext}");
        assert!((inf.duration - 4.5).abs() <= tolerance, "{ext}: duration {}", inf.duration);
        assert_eq!(inf.channels, 2, "{ext}");
        // Playable, and the cut is in it.
        let a = decode_audio(&out);
        assert!((a.len() as f64 / RATE as f64 - 4.5).abs() <= tolerance + 0.03, "{ext}: decoded length {}", a.len() as f64 / RATE as f64);
        assert_eq!(unit_heard(&a, 1.1, 1.4), Some(2), "{ext}");
        assert_eq!(unit_heard(&a, 1.6, 1.9), Some(4), "{ext}: unit 3 was cut");
    }
}

#[test]
fn video_format_exports_produce_the_right_kind_of_file() {
    let (e, _) = coded_editor(4); // 2 s
    let dir = tempfile::tempdir().unwrap();
    struct Case {
        video: VideoCodec,
        ext: &'static str,
        vcodec: &'static str,
        acodec: Option<&'static str>,
        format: &'static str,
        pix_fmt: &'static str,
    }
    let cases = [
        Case { video: VideoCodec::H264, ext: "mp4", vcodec: "h264", acodec: Some("aac"), format: "mov,mp4,m4a,3gp,3g2,mj2", pix_fmt: "yuv420p" },
        Case { video: VideoCodec::Hevc, ext: "mp4", vcodec: "hevc", acodec: Some("aac"), format: "mov,mp4,m4a,3gp,3g2,mj2", pix_fmt: "yuv420p" },
        Case { video: VideoCodec::ProRes, ext: "mov", vcodec: "prores", acodec: Some("aac"), format: "mov,mp4,m4a,3gp,3g2,mj2", pix_fmt: "yuv422p10le" },
        Case { video: VideoCodec::Vp9, ext: "webm", vcodec: "vp9", acodec: Some("opus"), format: "matroska,webm", pix_fmt: "yuv420p" },
        Case { video: VideoCodec::Gif, ext: "gif", vcodec: "gif", acodec: None, format: "gif", pix_fmt: "" },
    ];
    for c in cases {
        let mut s = small(&e);
        s.video = Some(c.video);
        assert_eq!(s.extension(), c.ext);
        let out = dir.path().join(format!("{:?}.{}", c.video, c.ext));
        export_to(&e, &s, &out);
        let inf = info(&out);
        assert_eq!(inf.vcodec.as_deref(), Some(c.vcodec), "{:?}", c.video);
        assert_eq!(inf.acodec.as_deref(), c.acodec, "{:?}", c.video);
        assert_eq!(inf.format, c.format, "{:?}", c.video);
        assert_eq!((inf.width, inf.height), (320, 180), "{:?}", c.video);
        if !c.pix_fmt.is_empty() {
            assert_eq!(inf.pix_fmt, c.pix_fmt, "{:?}", c.video);
        }
        // Playable: every frame decodes and shows the right unit.
        let v = decode_video(&out);
        let fps = if c.video == VideoCodec::Gif { 20.0 } else { 30.0 };
        assert_eq!(v.frames.len(), (2.0 * fps) as usize, "{:?}: frame count", c.video);
        assert!((inf.duration - 2.0).abs() <= 1.0 / fps + 0.03, "{:?}: duration {}", c.video, inf.duration);
        for k in 0..4 {
            let f = ((k as f64 * UNIT + 0.25) * fps) as usize;
            let got = v.at(f, 0.5, 0.5);
            assert!(close(got, unit_color(k), if c.video == VideoCodec::Gif { 40 } else { 14 }), "{:?}: unit {k} colour, got {got:?}", c.video);
        }
        if c.acodec.is_some() {
            let a = decode_audio(&out);
            assert_eq!(unit_heard(&a, 1.1, 1.4), Some(2), "{:?}", c.video);
        }
        if c.video == VideoCodec::Hevc {
            let j = ffprobe_json(&out, &[]);
            let tag = j["streams"].as_array().unwrap().iter().find(|s| s["codec_type"] == "video").unwrap()["codec_tag_string"].as_str().unwrap().to_string();
            assert_eq!(tag, "hvc1", "HEVC must be tagged hvc1 so QuickTime plays it");
        }
    }
}

#[test]
fn video_without_audio_has_no_audio_stream() {
    let (e, _) = coded_editor(4);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("silent.mp4");
    let mut s = settings(&e);
    s.audio = None;
    export_to(&e, &s, &out);
    let inf = info(&out);
    assert_eq!(inf.acodec, None);
    assert_eq!(inf.frames, 60);
}

#[test]
fn range_exports_just_that_range() {
    let (mut e, _) = coded_editor(124);
    let mut layer = Layer::new("Bar", 0.0, 62.0, LayerContent::Progress { color: Rgba::rgb(0, 0, 255), track_color: Rgba::rgb(0, 255, 0) });
    layer.transform = Transform { x: 0.5, y: 0.9, w: 1.0, h: 0.1, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("range.mp4");
    let mut s = small(&e);
    s.range = Some((30.0, 32.0));
    let started = std::time::Instant::now();
    let report = export_to(&e, &s, &out);
    let took = started.elapsed();
    let inf = info(&out);
    assert_eq!(inf.frames, 60);
    assert!((inf.duration - 2.0).abs() <= FRAME, "duration {}", inf.duration);
    assert!((report.duration - 2.0).abs() <= FRAME);
    let v = decode_video(&out);
    let a = decode_audio(&out);
    assert!((a.len() as f64 / RATE as f64 - 2.0).abs() <= FRAME);
    // 30.0 s is the start of unit 60.
    for j in 0..4 {
        assert_eq!(unit_of_color(v.at(j * 15, 0.5, 0.3)), Some(60 + j), "first frame of range unit {j}");
        assert_eq!(unit_of_color(v.at(j * 15 + 14, 0.5, 0.3)), Some(60 + j), "last frame of range unit {j}");
        assert_eq!(unit_heard(&a, j as f64 * UNIT + 0.1, j as f64 * UNIT + 0.4), Some(60 + j));
    }
    // The progress bar shows composition progress (about half), not range progress.
    assert!(is_blue(v.at(30, 0.40, 0.9)), "bar is filled left of the playhead: {:?}", v.at(30, 0.40, 0.9));
    assert!(is_green(v.at(30, 0.60, 0.9)), "bar is empty right of the playhead: {:?}", v.at(30, 0.60, 0.9));
    assert!(took.as_secs_f64() < 30.0, "a 2 s range export should not take {took:?}");
}

#[test]
fn progress_bar_layer_advances_with_time() {
    let (mut e, _) = coded_editor(10);
    let mut layer = Layer::new("Bar", 0.0, 5.0, LayerContent::Progress { color: Rgba::rgb(0, 0, 255), track_color: Rgba::rgb(0, 255, 0) });
    layer.transform = Transform { x: 0.5, y: 0.9, w: 1.0, h: 0.1, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bar.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    assert_eq!(v.frames.len(), 150);
    for f in [15usize, 45, 75, 105, 135, 148] {
        // Right edge of the filled part along the bar's centre line.
        let y = (0.9 * 180.0) as usize;
        let edge = (0..320).rev().find(|x| is_blue(v.pixel(f, *x, y))).map(|x| x + 1).unwrap_or(0);
        let expected = f as f64 / 150.0 * 320.0;
        assert!((edge as f64 - expected).abs() <= 4.0, "frame {f}: fill ends at x={edge}, expected about {expected:.0}");
        // The rest of the bar is track colour; above the bar is the script picture.
        if edge + 8 < 312 {
            assert!(is_green(v.pixel(f, edge + 8, y)), "frame {f}: track colour right of the fill");
        }
        assert_eq!(unit_of_color(v.at(f, 0.5, 0.4)), Some(f / 15));
    }
}

#[test]
fn waveform_layer_follows_the_audio() {
    let (mut e, _) = coded_editor(16); // 8 s
    let i = word_index(&e, "w6");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 6 }).unwrap(); // silence 3.0..6.0
    let mut layer = Layer::new("Wave", 0.0, 8.0, LayerContent::Waveform(WaveformStyle { kind: WaveformKind::Mirror, color: Rgba::rgb(0, 0, 255) }));
    layer.transform = Transform { x: 0.5, y: 0.5, w: 1.0, h: 0.8, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("wave.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    // Height of the waveform in the middle of the frame (= "now").
    let height_now = |f: usize| (150..170).map(|x| (0..180).filter(|y| is_blue(v.pixel(f, x, *y))).count()).max().unwrap();
    let loud = height_now(45); // 1.5 s: tone at amplitude 0.5 -> half the 144 px box
    let quiet = height_now(135); // 4.5 s: middle of the silence
    assert!((loud as i64 - 72).abs() <= 8, "bars should be about half the box high during the tone, got {loud} px");
    // The composition is 36 px high and exported at 5x; silent bars are 2 composition px.
    assert!(quiet <= 11, "bars should be flat during silence, got {quiet} px");
    // At 2.0 s the right half of the window (the future, from 3.0 s) is already silent.
    let right_edge = (300..318).map(|x| (0..180).filter(|y| is_blue(v.pixel(60, x, *y))).count()).max().unwrap();
    assert!(right_edge <= 11, "upcoming silence should be visible at the right edge, got {right_edge} px");
}

#[test]
fn chapters_and_title_are_embedded() {
    let (mut e, _) = coded_editor(10);
    let i = word_index(&e, "w4");
    e.apply(Command::SetMarker { at: i, name: Some("Middle = part; #2 \\ two".into()) }).unwrap();
    let i = word_index(&e, "w8");
    e.apply(Command::SetMarker { at: i, name: Some("End".into()) }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut s = settings(&e);
    s.title = Some("My Episode".into());
    let out = dir.path().join("chapters.mp4");
    export_to(&e, &s, &out);
    let inf = info(&out);
    let near = |a: f64, b: f64| (a - b).abs() < 0.002;
    assert_eq!(inf.chapters.len(), 3, "{:?}", inf.chapters);
    assert!(near(inf.chapters[0].0, 0.0) && near(inf.chapters[0].1, 2.0) && inf.chapters[0].2 == "Intro", "{:?}", inf.chapters);
    assert!(near(inf.chapters[1].0, 2.0) && near(inf.chapters[1].1, 4.0) && inf.chapters[1].2 == "Middle = part; #2 \\ two", "{:?}", inf.chapters);
    assert!(near(inf.chapters[2].0, 4.0) && near(inf.chapters[2].1, 5.0) && inf.chapters[2].2 == "End", "{:?}", inf.chapters);
    assert_eq!(inf.title.as_deref(), Some("My Episode"));
    assert_eq!(inf.frames, 150, "chapters must not disturb the video");

    // Audio-only m4a carries them too.
    let mut s2 = s.clone();
    s2.video = None;
    let out2 = dir.path().join("chapters.m4a");
    export_to(&e, &s2, &out2);
    assert_eq!(info(&out2).chapters.len(), 3);

    // Not when switched off.
    let mut s3 = s.clone();
    s3.embed_chapters = false;
    let out3 = dir.path().join("nochapters.mp4");
    export_to(&e, &s3, &out3);
    assert!(info(&out3).chapters.is_empty());

    // A range export shifts and clips the chapters to the range.
    let mut s4 = s.clone();
    s4.range = Some((1.0, 4.5));
    let out4 = dir.path().join("range_chapters.mp4");
    export_to(&e, &s4, &out4);
    let c = info(&out4).chapters;
    assert_eq!(c.len(), 3, "{c:?}");
    assert!((info(&out4).duration - 3.5).abs() <= FRAME);
    assert!(near(c[0].0, 0.0) && near(c[0].1, 1.0) && c[0].2 == "Intro", "{c:?}");
    assert!(near(c[1].0, 1.0) && near(c[1].1, 3.0), "{c:?}");
    assert!(near(c[2].0, 3.0) && near(c[2].1, 3.5) && c[2].2 == "End", "{c:?}");
}

#[test]
fn export_without_markers_has_no_chapters() {
    let (e, _) = coded_editor(4);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("plain.mp4");
    export_to(&e, &settings(&e), &out);
    assert!(info(&out).chapters.is_empty());
}

fn big_job() -> (Editor, ExportSettings) {
    let (mut e, _) = coded_editor(124);
    for k in (1..120).step_by(2).rev() {
        let i = word_index(&e, &format!("w{k}"));
        e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    }
    let mut s = settings(&e);
    s.width = 1280;
    s.height = 720;
    (e, s)
}

fn cancel_when(reached: impl Fn(&Progress) -> bool) {
    let (e, s) = big_job();
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let out = dir.path().join("cancelled.mp4");
    let cancel = AtomicBool::new(false);
    let mut cancelled_at: Option<std::time::Instant> = None;
    let mut seen_after_cancel = 0usize;
    let mut work_was_used = false;
    let mut partial_seen = false;
    let result = export_with_work_root(
        &e.project,
        e.comp().unwrap(),
        &s,
        &out,
        work.path(),
        &mut |p| {
            if cancelled_at.is_some() {
                seen_after_cancel += 1;
            } else if reached(&p) {
                // Mid-export the intermediates and the partial output exist.
                work_was_used = !listing(work.path()).is_empty();
                partial_seen = listing(dir.path()).iter().any(|n| n.starts_with('.') && n.ends_with(".mp4"));
                cancel.store(true, Ordering::SeqCst);
                cancelled_at = Some(std::time::Instant::now());
            }
        },
        &cancel,
    );
    let err = result.expect_err("a cancelled export must not succeed");
    assert!(mango_media::is_cancelled(&err), "expected the cancel error, got: {err:#}");
    assert!(err.downcast_ref::<mango_media::Cancelled>().is_some(), "the cancel error is the error itself, not buried in context");
    let waited = cancelled_at.expect("the export got far enough to cancel").elapsed();
    assert!(waited.as_secs_f64() < 3.0, "cancel should take effect promptly, took {waited:?}");
    assert!(seen_after_cancel <= 2, "progress should stop after cancel, got {seen_after_cancel} more callbacks");
    assert!(work_was_used, "the export should have been using its work folder when cancelled");
    assert!(partial_seen, "the output is written under a hidden temporary name next to the destination");
    assert!(!out.exists(), "a cancelled export must not leave a file at the destination");
    assert!(listing(dir.path()).is_empty(), "a cancelled export must leave nothing next to the destination, found {:?}", listing(dir.path()));
    assert!(listing(work.path()).is_empty(), "a cancelled export must remove its work folder, found {:?}", listing(work.path()));
}

#[test]
fn cancel_during_chunks_leaves_nothing_behind() {
    cancel_when(|p| matches!(p.stage, Stage::Chunk { .. }) && p.fraction > 0.2);
}

#[test]
fn cancel_during_final_pass_leaves_nothing_behind() {
    cancel_when(|p| p.stage == Stage::Final && p.fraction > 0.7);
}

#[test]
fn cancel_before_start_does_nothing() {
    let (e, s) = big_job();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("never.mp4");
    let cancel = AtomicBool::new(true);
    let err = export(&e.project, e.comp().unwrap(), &s, &out, &mut |_| {}, &cancel).unwrap_err();
    assert!(mango_media::is_cancelled(&err));
    assert!(listing(dir.path()).is_empty());
}

#[test]
fn cancelled_export_does_not_clobber_an_existing_file() {
    let (e, s) = big_job();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("keep.mp4");
    std::fs::write(&out, b"previous export").unwrap();
    let cancel = AtomicBool::new(false);
    let err = export(
        &e.project,
        e.comp().unwrap(),
        &s,
        &out,
        &mut |p| {
            if p.fraction > 0.3 {
                cancel.store(true, Ordering::SeqCst);
            }
        },
        &cancel,
    )
    .unwrap_err();
    assert!(mango_media::is_cancelled(&err));
    assert_eq!(std::fs::read(&out).unwrap(), b"previous export");
    assert_eq!(listing(dir.path()), ["keep.mp4"]);
}

#[test]
fn missing_source_file_gives_a_readable_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("interview take 2.mp4");
    std::fs::copy(coded_source(10), &src).unwrap();
    let (e, _) = editor_with_source(&src, Some(unit_transcript(10)));
    std::fs::remove_file(&src).unwrap();
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("out.mp4");
    let cancel = AtomicBool::new(false);
    let err = export(&e.project, e.comp().unwrap(), &settings(&e), &out, &mut |_| {}, &cancel).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("interview take 2.mp4"), "the error should name the file: {msg}");
    assert!(msg.contains(src.to_str().unwrap()), "the error should give the full path: {msg}");
    assert!(msg.to_lowercase().contains("missing"), "the error should say what is wrong: {msg}");
    assert!(!mango_media::is_cancelled(&err));
    assert!(!out.exists());
    assert!(listing(out_dir.path()).is_empty(), "left behind: {:?}", listing(out_dir.path()));
}

#[test]
fn missing_layer_file_gives_a_readable_error_naming_the_layer() {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("theme.wav");
    std::fs::copy(tone_wav("tone_2000_quarter.wav", 2000, 8.0, 0.25), &music).unwrap();
    let (mut e, _) = coded_editor(4);
    let id = add_asset(&mut e, &music);
    e.apply(Command::AddLayer { layer: Layer::new("Music bed", 0.0, 2.0, LayerContent::Audio(MediaClip::new(id))) }).unwrap();
    std::fs::remove_file(&music).unwrap();
    let out = dir.path().join("out.mp4");
    let cancel = AtomicBool::new(false);
    let err = export(&e.project, e.comp().unwrap(), &settings(&e), &out, &mut |_| {}, &cancel).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("theme.wav") && msg.contains("Music bed"), "{msg}");
    assert!(listing(dir.path()).is_empty(), "left behind: {:?}", listing(dir.path()));
}

#[test]
fn a_failure_inside_ffmpeg_is_reported_and_cleaned_up() {
    // The file exists when checked but is not decodable: ffmpeg itself fails.
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let src = dir.path().join("take.mp4");
    std::fs::copy(coded_source(10), &src).unwrap();
    let (e, _) = editor_with_source(&src, Some(unit_transcript(10)));
    std::fs::write(&src, vec![0x55u8; 50_000]).unwrap();
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("out.mp4");
    std::fs::write(&out, b"previous export").unwrap();
    let cancel = AtomicBool::new(false);
    let err = export_with_work_root(&e.project, e.comp().unwrap(), &settings(&e), &out, work.path(), &mut |_| {}, &cancel).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("part 1 of 1"), "says which step failed: {msg}");
    assert!(msg.contains("take.mp4"), "carries ffmpeg's own message naming the file: {msg}");
    assert!(!mango_media::is_cancelled(&err));
    assert_eq!(std::fs::read(&out).unwrap(), b"previous export", "a failed export must not touch the previous file");
    assert_eq!(listing(out_dir.path()), ["out.mp4"], "no partial file left");
    assert!(listing(work.path()).is_empty(), "work folder removed after a failure: {:?}", listing(work.path()));
}

#[test]
fn successful_export_removes_its_work_folder() {
    let (e, _) = coded_editor(4);
    let dir = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let out = dir.path().join("ok.mp4");
    let cancel = AtomicBool::new(false);
    let mut work_used = false;
    export_with_work_root(&e.project, e.comp().unwrap(), &settings(&e), &out, work.path(), &mut |p| {
        if p.stage == Stage::Final && p.fraction < 1.0 {
            work_used |= listing(work.path()).iter().any(|n| n.starts_with("mango-export-"));
        }
    }, &cancel)
    .unwrap();
    assert!(work_used, "intermediates live in the work root during the export");
    assert!(listing(work.path()).is_empty(), "and are gone afterwards: {:?}", listing(work.path()));
    assert_eq!(listing(dir.path()), ["ok.mp4"]);
    assert_eq!(info(&out).frames, 60);
    // Exporting again over the same file replaces it.
    let (e2, _) = coded_editor(6);
    export_with_work_root(&e2.project, e2.comp().unwrap(), &settings(&e2), &out, work.path(), &mut |_| {}, &cancel).unwrap();
    assert_eq!(info(&out).frames, 90);
    assert_eq!(listing(dir.path()), ["ok.mp4"]);
}

#[test]
fn bad_requests_fail_cleanly() {
    let (e, _) = coded_editor(4);
    let dir = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let run = |s: &ExportSettings, out: &std::path::Path| export(&e.project, e.comp().unwrap(), s, out, &mut |_| {}, &cancel);
    // Destination folder does not exist.
    let err = run(&settings(&e), &dir.path().join("nope/out.mp4")).unwrap_err();
    assert!(format!("{err:#}").contains("does not exist"), "{err:#}");
    // Neither video nor audio.
    let mut s = settings(&e);
    s.video = None;
    s.audio = None;
    assert!(run(&s, &dir.path().join("a.mp4")).is_err());
    // Range outside the composition.
    let mut s = settings(&e);
    s.range = Some((10.0, 12.0));
    let err = run(&s, &dir.path().join("b.mp4")).unwrap_err();
    assert!(format!("{err:#}").contains("range"), "{err:#}");
    // Empty composition.
    let empty = Editor::new_project("Empty");
    let err = export(&empty.project, empty.comp().unwrap(), &settings(&empty), &dir.path().join("c.mp4"), &mut |_| {}, &cancel).unwrap_err();
    assert!(format!("{err:#}").contains("empty"), "{err:#}");
    assert!(listing(dir.path()).is_empty(), "left behind: {:?}", listing(dir.path()));
}

#[test]
fn progress_is_monotonic_and_covers_all_stages() {
    let (mut e, _) = coded_editor(124);
    // 40 cuts -> 41 clips -> two chunks.
    for k in (1..80).step_by(2).rev() {
        let i = word_index(&e, &format!("w{k}"));
        e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("progress.mp4");
    let cancel = AtomicBool::new(false);
    let mut seen: Vec<Progress> = Vec::new();
    export(&e.project, e.comp().unwrap(), &settings(&e), &out, &mut |p| seen.push(p), &cancel).unwrap();
    assert!(seen.len() >= 6, "expected regular progress, got {} updates", seen.len());
    assert_eq!(seen[0].stage, Stage::Preparing);
    assert_eq!(seen[0].fraction, 0.0);
    for w in seen.windows(2) {
        assert!(w[1].fraction >= w[0].fraction, "progress went backwards: {:?} -> {:?}", w[0], w[1]);
    }
    assert!(seen.iter().all(|p| (0.0..=1.0).contains(&p.fraction)));
    let last = seen.last().unwrap();
    assert_eq!((last.stage, last.fraction), (Stage::Final, 1.0));
    assert!(seen.iter().any(|p| p.stage == Stage::Chunk { index: 0, of: 2 }));
    assert!(seen.iter().any(|p| p.stage == Stage::Chunk { index: 1, of: 2 }));
    // Stages come in order.
    let order = |s: Stage| match s {
        Stage::Preparing => 0,
        Stage::Chunk { index, .. } => 1 + index,
        Stage::Final => 1000,
    };
    for w in seen.windows(2) {
        assert!(order(w[1].stage) >= order(w[0].stage), "stage went backwards: {:?} -> {:?}", w[0], w[1]);
    }
    // Progress is spread out, not one jump at the end.
    assert!(seen.iter().any(|p| p.fraction > 0.2 && p.fraction < 0.5));
    assert!(seen.iter().any(|p| p.fraction > 0.6 && p.fraction < 0.99));
}

#[test]
fn audio_only_source_exports_video_with_background() {
    let (mut e, _) = editor_with_source(&coded_audio(6), Some(unit_transcript(6)));
    e.apply(Command::SetBackground { color: Rgba::rgb(0, 0, 255) }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("podcast.mp4");
    export_to(&e, &small(&e), &out);
    let inf = info(&out);
    assert_eq!(inf.frames, 90);
    let v = decode_video(&out);
    for f in [0, 45, 89] {
        assert!(is_blue(v.at(f, 0.5, 0.5)), "background colour on frame {f}: {:?}", v.at(f, 0.5, 0.5));
    }
    let a = decode_audio(&out);
    for k in 0..6 {
        assert_eq!(unit_heard(&a, k as f64 * UNIT + 0.1, k as f64 * UNIT + 0.4), Some(k));
    }
}

#[test]
fn export_at_a_different_frame_rate_and_odd_size() {
    let (e, _) = coded_editor(10);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("24.mp4");
    let mut s = settings(&e);
    s.fps = 24.0;
    s.width = 321; // encoders need even sizes; rounded down
    s.height = 181;
    export_to(&e, &s, &out);
    let inf = info(&out);
    assert_eq!((inf.width, inf.height), (320, 180));
    assert_eq!(inf.frames, 120);
    assert!((inf.fps - 24.0).abs() < 0.01);
    assert!((inf.duration - 5.0).abs() <= 1.0 / 24.0);
    let v = decode_video(&out);
    for k in 0..10 {
        assert_eq!(unit_of_color(v.at(k * 12 + 6, 0.5, 0.5)), Some(k));
    }
}

#[test]
fn loudness_normalisation_changes_the_level() {
    let (e, _) = coded_editor(20); // 10 s of -9 dBFS tone
    let dir = tempfile::tempdir().unwrap();
    let level = |normalize: Normalize, name: &str| {
        let mut s = settings(&e);
        s.video = None;
        s.audio = Some(AudioCodec::Wav);
        s.normalize = normalize;
        let out = dir.path().join(name);
        export_to(&e, &s, &out);
        let inf = info(&out);
        assert!((inf.duration - 10.0).abs() < 0.01, "normalising must not change the length: {}", inf.duration);
        assert_eq!(inf.sample_rate, 48_000);
        rms(&decode_audio(&out), 5.0, 9.5)
    };
    let off = level(Normalize::Off, "off.wav");
    let quiet = level(Normalize::Lufs(-30.0), "quiet.wav");
    let drop_db = 20.0 * (off / quiet).log10();
    assert!((off - 0.5 / 2f64.sqrt()).abs() < 0.02, "un-normalised level should be the source's, got {off}");
    assert!(drop_db > 10.0 && drop_db < 30.0, "normalising a loud tone to -30 LUFS should lower it by roughly 18 dB, got {drop_db:.1} dB");
}

#[test]
fn hidden_layers_are_not_exported() {
    let (mut e, _) = coded_editor(4);
    let mut layer = Layer::new("Box", 0.0, 2.0, LayerContent::Shape(Shape { kind: ShapeKind::Rectangle, fill: Rgba::rgb(0, 0, 255), stroke: None, corner_radius: 0.0 }));
    layer.transform = Transform { x: 0.5, y: 0.5, w: 0.5, h: 0.5, rotation_deg: 0.0, opacity: 1.0 };
    layer.hidden = true;
    let id = layer.id;
    e.apply(Command::AddLayer { layer: layer.clone() }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("hidden.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    assert_eq!(v.count_in(30, 0.0, 0.0, 1.0, 1.0, is_blue), 0);
    // Shown again, the shape fills exactly its box.
    layer.hidden = false;
    assert_eq!(layer.id, id);
    e.apply(Command::UpdateLayer { layer }).unwrap();
    let out = dir.path().join("shown.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    let n = v.count_in(30, 0.0, 0.0, 1.0, 1.0, is_blue);
    assert!((n as i64 - 160 * 90).abs() < 600, "shape should cover its 160x90 box, covers {n}");
    let (x0, y0, x1, y1) = v.bbox(30, is_blue).unwrap();
    let near = |a: usize, b: usize| (a as i64 - b as i64).abs() <= 2;
    assert!(near(x0, 80) && near(x1, 239) && near(y0, 45) && near(y1, 134), "{:?}", (x0, y0, x1, y1));
}

#[test]
fn layer_track_order_decides_what_is_on_top() {
    let (mut e, _) = coded_editor(4);
    let shape = |color: Rgba, track: i32| {
        let mut l = Layer::new("Box", 0.0, 2.0, LayerContent::Shape(Shape { kind: ShapeKind::Rectangle, fill: color, stroke: None, corner_radius: 0.0 }));
        l.transform = Transform { x: 0.5, y: 0.5, w: 0.5, h: 0.5, rotation_deg: 0.0, opacity: 1.0 };
        l.track = track;
        l
    };
    // Added top-first, so only the track number can put blue above green.
    e.apply(Command::AddLayer { layer: shape(Rgba::rgb(0, 0, 255), 5) }).unwrap();
    e.apply(Command::AddLayer { layer: shape(Rgba::rgb(0, 255, 0), 2) }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("order.mp4");
    export_to(&e, &small(&e), &out);
    let v = decode_video(&out);
    assert!(is_blue(v.at(30, 0.5, 0.5)), "the higher track should be on top, got {:?}", v.at(30, 0.5, 0.5));
    // The preview compositor agrees.
    let frame = mango_media::raster::render_overlay(&e.project, e.comp().unwrap(), &[], 1.0, 320, 180);
    let p = frame.pixel(160, 90);
    assert_eq!([p[0], p[1], p[2], p[3]], [0, 0, 255, 255]);
}
