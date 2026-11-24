//! Export tests for the less-travelled features: effects, speed, loops,
//! animations, crops, placement overrides, layers outside the script, and
//! odd inputs. Real ffmpeg, assertions on the decoded output.

mod common;
use common::*;
use mango_core::render::*;
use mango_core::*;
use mango_media::export::export;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

const FRAME: f64 = 1.0 / 30.0;

fn small(e: &Editor) -> ExportSettings {
    let mut s = settings(e);
    s.width = 320;
    s.height = 180;
    s
}

/// 160x88, 3 s, 30 fps: top-left red, top-right green, bottom-left blue,
/// bottom-right white; 1 kHz tone.
fn quadrants() -> PathBuf {
    lavfi(
        "quadrants.mp4",
        &[
            "-f", "lavfi", "-i",
            "color=c=0xFF0000:s=80x44:r=30:d=3[a];color=c=0x00FF00:s=80x44:r=30:d=3[b];color=c=0x0000FF:s=80x44:r=30:d=3[c];color=c=0xFFFFFF:s=80x44:r=30:d=3[d];[a][b]hstack[t];[c][d]hstack[u];[t][u]vstack",
            "-f", "lavfi", "-i", "sine=f=1000:d=3:r=48000",
            "-c:v", "libx264", "-crf", "10", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k", "-shortest", "-f", "mp4",
        ],
    )
}

fn quad(p: [u8; 3]) -> char {
    let hi = |v: u8| v > 170;
    let lo = |v: u8| v < 90;
    match p {
        [r, g, b] if hi(r) && hi(g) && hi(b) => 'W',
        [r, g, b] if hi(r) && lo(g) && lo(b) => 'R',
        [r, g, b] if lo(r) && hi(g) && lo(b) => 'G',
        [r, g, b] if lo(r) && lo(g) && hi(b) => 'B',
        [r, g, b] if lo(r) && lo(g) && lo(b) => 'K',
        _ => '?',
    }
}

fn is_blue(p: [u8; 3]) -> bool {
    quad(p) == 'B'
}
fn is_green(p: [u8; 3]) -> bool {
    quad(p) == 'G'
}

fn quad_editor() -> (Editor, AssetId) {
    editor_with_source(&quadrants(), None)
}

fn export_video(e: &Editor, s: &ExportSettings, name: &str) -> (Video, Vec<f32>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join(name);
    export_to(e, s, &out);
    let v = decode_video(&out);
    let a = if s.audio.is_some() && s.video != Some(VideoCodec::Gif) { decode_audio(&out) } else { Vec::new() };
    (v, a, dir)
}

#[test]
fn source_without_transcript_exports_whole() {
    let (e, _) = quad_editor();
    let (v, a, _d) = export_video(&e, &settings(&e), "q.mp4");
    assert_eq!((v.w, v.h), (160, 88));
    assert_eq!(v.frames.len(), 90);
    for f in [0, 45, 89] {
        assert_eq!([quad(v.at(f, 0.25, 0.25)), quad(v.at(f, 0.75, 0.25)), quad(v.at(f, 0.25, 0.75)), quad(v.at(f, 0.75, 0.75))], ['R', 'G', 'B', 'W']);
    }
    assert!(tone_level(&a, 1000.0, 0.5, 2.5) > 0.05);
}

#[test]
fn every_video_effect_is_accepted_and_does_its_job() {
    let effects: Vec<(VideoEffect, &str)> = vec![
        (VideoEffect::ChromaKey { color: Rgba::rgb(0, 255, 0), similarity: 0.3, blend: 0.0 }, "chroma"),
        (VideoEffect::Blur { radius: 4.0 }, "blur"),
        (VideoEffect::Brightness { value: 0.3 }, "brightness"),
        (VideoEffect::Contrast { value: 1.5 }, "contrast"),
        (VideoEffect::Saturation { value: 0.0 }, "saturation0"),
        (VideoEffect::Grayscale, "gray"),
        (VideoEffect::Flip, "flip"),
        (VideoEffect::FlipVertical, "vflip"),
        (VideoEffect::CornerRadius { fraction: 0.5 }, "corner"),
        (VideoEffect::Vignette, "vignette"),
        (VideoEffect::Invert, "invert"),
        (VideoEffect::Pixelate { size: 8 }, "pixelate"),
        (VideoEffect::Grain { amount: 0.5 }, "grain"),
        (VideoEffect::Temperature { kelvin: 3000.0 }, "temperature"),
        (VideoEffect::Exposure { stops: 1.0 }, "exposure"),
    ];
    for (effect, name) in effects {
        let (mut e, _) = quad_editor();
        e.apply(Command::SetBackground { color: Rgba::rgb(0, 0, 0) }).unwrap();
        let placement = VideoPlacement { effects: vec![effect.clone()], ..VideoPlacement::default() };
        e.apply(Command::SetScriptVideo { placement }).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join(format!("{name}.mp4"));
        let cancel = AtomicBool::new(false);
        if let Err(err) = export(&e.project, e.comp().unwrap(), &settings(&e), &out, &mut |_| {}, &cancel) {
            panic!("{name}: export failed: {err:#}");
        }
        let v = decode_video(&out);
        assert_eq!(v.frames.len(), 90, "{name}");
        let q = |fx: f64, fy: f64| quad(v.at(45, fx, fy));
        let corners = [q(0.25, 0.25), q(0.75, 0.25), q(0.25, 0.75), q(0.75, 0.75)];
        match effect {
            VideoEffect::Flip => assert_eq!(corners, ['G', 'R', 'W', 'B'], "{name}: mirrored left-right"),
            VideoEffect::FlipVertical => assert_eq!(corners, ['B', 'W', 'R', 'G'], "{name}: mirrored top-bottom"),
            VideoEffect::Invert => {
                assert_color(v.at(45, 0.25, 0.25), [0, 255, 255], 30, "inverted red");
                assert_color(v.at(45, 0.75, 0.75), [0, 0, 0], 30, "inverted white");
            }
            VideoEffect::Grayscale | VideoEffect::Saturation { .. } => {
                for (fx, fy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75)] {
                    let p = v.at(45, fx, fy);
                    let spread = *p.iter().max().unwrap() as i32 - *p.iter().min().unwrap() as i32;
                    assert!(spread < 24, "{name}: colour should be gone, got {p:?}");
                }
            }
            VideoEffect::ChromaKey { .. } => {
                assert_eq!(corners[0], 'R', "{name}");
                assert_eq!(corners[1], 'K', "{name}: green is keyed out to the black background, got {:?}", v.at(45, 0.75, 0.25));
                assert_eq!(corners[2], 'B', "{name}");
            }
            VideoEffect::CornerRadius { .. } => {
                assert_eq!(quad(v.at(45, 0.01, 0.02)), 'K', "{name}: corners are cut away, got {:?}", v.at(45, 0.01, 0.02));
                assert_eq!(quad(v.at(45, 0.99, 0.98)), 'K', "{name}");
                assert_eq!(corners, ['R', 'G', 'B', 'W'], "{name}: the middle is untouched");
            }
            VideoEffect::Brightness { .. } | VideoEffect::Exposure { .. } => {
                let before = [0u8, 0, 255];
                let p = v.at(45, 0.25, 0.75);
                assert!(p[0] as i32 + p[1] as i32 > before[0] as i32 + before[1] as i32 + 20, "{name}: blue should get lighter, got {p:?}");
            }
            VideoEffect::Blur { .. } => {
                // The hard edge between red and green becomes a gradient.
                let edge = v.at(45, 0.5, 0.25);
                assert_eq!(quad(edge), '?', "{name}: edge should be mixed, got {edge:?}");
                assert_eq!(corners[2], 'B', "{name}");
            }
            VideoEffect::Temperature { .. } => {
                let w = v.at(45, 0.75, 0.75);
                assert!(w[0] as i32 > w[2] as i32 + 30, "{name}: white should turn warm, got {w:?}");
            }
            VideoEffect::Contrast { .. } | VideoEffect::Vignette | VideoEffect::Pixelate { .. } | VideoEffect::Grain { .. } => {
                assert_eq!(corners[2], 'B', "{name}: still recognisably the source");
            }
        }
    }
}

#[test]
fn every_audio_effect_is_accepted_and_keeps_timing() {
    let effects: Vec<(AudioEffect, &str)> = vec![
        (AudioEffect::SpeechEnhance { intensity: 0.5 }, "speech"),
        (AudioEffect::Denoise { strength_db: 12.0 }, "denoise"),
        (AudioEffect::HighPass { hz: 2000.0 }, "highpass"),
        (AudioEffect::LowPass { hz: 100.0 }, "lowpass"),
        (AudioEffect::Eq { hz: 500.0, gain_db: -12.0, q: 1.0 }, "eq"),
        (AudioEffect::Compressor { threshold_db: -20.0, ratio: 4.0, attack_ms: 5.0, release_ms: 100.0, makeup_db: 0.0 }, "compressor"),
        (AudioEffect::Limiter { ceiling_db: -20.0 }, "limiter"),
        (AudioEffect::DeEsser { intensity: 0.5 }, "deesser"),
        (AudioEffect::Gate { threshold_db: -3.0 }, "gate"),
        (AudioEffect::LowShelf { hz: 2000.0, gain_db: -12.0 }, "lowshelf"),
        (AudioEffect::HighShelf { hz: 100.0, gain_db: -12.0 }, "highshelf"),
        (AudioEffect::Reverb { mix: 0.5, room: 0.5 }, "reverb"),
        (AudioEffect::Distortion { drive: 0.5 }, "distortion"),
        (AudioEffect::Bitcrusher { bits: 4 }, "bitcrusher"),
        (AudioEffect::Flanger { depth: 2.0, rate_hz: 0.5 }, "flanger"),
        (AudioEffect::Loudness { target_lufs: -30.0 }, "loudness"),
    ];
    // Reference: 4 s of unit tones at amplitude 0.5.
    let reference = {
        let (e, _) = coded_editor(8);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("ref.wav");
        let mut s = settings(&e);
        s.video = None;
        s.audio = Some(AudioCodec::Wav);
        export_to(&e, &s, &out);
        tone_level(&decode_audio(&out), unit_freq(5), 2.6, 2.95)
    };
    assert!((reference - 0.5).abs() < 0.03, "reference level {reference}");
    for (effect, name) in effects {
        let (mut e, _) = coded_editor(8);
        e.apply(Command::SetScriptAudio { audio: AudioSettings { effects: vec![effect.clone()], ..AudioSettings::default() } }).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join(format!("{name}.wav"));
        let mut s = settings(&e);
        s.video = None;
        s.audio = Some(AudioCodec::Wav);
        let cancel = AtomicBool::new(false);
        if let Err(err) = export(&e.project, e.comp().unwrap(), &s, &out, &mut |_| {}, &cancel) {
            panic!("{name}: export failed: {err:#}");
        }
        let inf = info(&out);
        assert!((inf.duration - 4.0).abs() < 0.002, "{name}: effects must not change the length, got {}", inf.duration);
        assert_eq!(inf.sample_rate, 48_000, "{name}");
        let a = decode_audio(&out);
        let level = tone_level(&a, unit_freq(5), 2.6, 2.95);
        let db = 20.0 * (level.max(1e-9) / reference).log10();
        match effect {
            AudioEffect::HighPass { .. } | AudioEffect::LowPass { .. } | AudioEffect::Eq { .. } | AudioEffect::LowShelf { .. } | AudioEffect::HighShelf { .. } => {
                assert!(db < -6.0, "{name}: the 500 Hz tone should be clearly attenuated, changed by {db:.1} dB")
            }
            AudioEffect::Limiter { .. } => assert!(db < -8.0, "{name}: a tone 11 dB over the ceiling should come down to it, changed by {db:.1} dB"),
            AudioEffect::Compressor { .. } | AudioEffect::Loudness { .. } => assert!(db < -3.0, "{name}: a loud tone should come out quieter, changed by {db:.1} dB"),
            AudioEffect::Gate { .. } => assert!(db < -3.0, "{name}: a tone under the threshold should be gated, changed by {db:.1} dB"),
            _ => assert!(level > 0.02, "{name}: the tone should still be there, level {level}"),
        }
        // Timing is intact: the right unit is heard at the right time.
        if !matches!(effect, AudioEffect::Bitcrusher { .. } | AudioEffect::Distortion { .. } | AudioEffect::Reverb { .. } | AudioEffect::Flanger { .. } | AudioEffect::LowPass { .. } | AudioEffect::Gate { .. }) {
            assert_eq!(unit_heard(&a, 1.1, 1.4), Some(2), "{name}");
        }
    }
}

#[test]
fn gain_mute_and_pan_apply_to_script_audio() {
    let run = |audio: AudioSettings, name: &str| {
        let (mut e, _) = coded_editor(4);
        e.apply(Command::SetScriptAudio { audio }).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join(name);
        let mut s = settings(&e);
        s.video = None;
        s.audio = Some(AudioCodec::Wav);
        export_to(&e, &s, &out);
        (decode_audio_channel(&out, 0), decode_audio_channel(&out, 1))
    };
    let f = unit_freq(1);
    let (l, r) = run(AudioSettings::default(), "plain.wav");
    let base = tone_level(&l, f, 0.6, 0.9);
    // A mono source lands in both channels at its own level (0.5), not 3 dB down.
    assert!((base - 0.5).abs() < 0.02, "left level {base}");
    assert!((tone_level(&r, f, 0.6, 0.9) - base).abs() < 0.01, "a mono source is centred");
    let (l, _) = run(AudioSettings { gain_db: -6.0, ..AudioSettings::default() }, "gain.wav");
    let db = 20.0 * (tone_level(&l, f, 0.6, 0.9) / base).log10();
    assert!((db + 6.0).abs() < 0.3, "-6 dB gain gave {db:.2} dB");
    let (l, r) = run(AudioSettings { muted: true, ..AudioSettings::default() }, "mute.wav");
    assert!(rms(&l, 0.0, 2.0) < 1e-4 && rms(&r, 0.0, 2.0) < 1e-4, "muted script audio must be silent");
    let (l, r) = run(AudioSettings { pan: -1.0, ..AudioSettings::default() }, "pan.wav");
    assert!(tone_level(&l, f, 0.6, 0.9) > base * 0.9, "hard left keeps the left channel");
    assert!(tone_level(&r, f, 0.6, 0.9) < base * 0.01, "hard left silences the right channel");
}

#[test]
fn mono_and_sample_rate_settings_are_honoured() {
    let (e, _) = coded_editor(4);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("mono.wav");
    let mut s = settings(&e);
    s.video = None;
    s.audio = Some(AudioCodec::Wav);
    s.channels = 1;
    s.sample_rate = 44_100;
    export_to(&e, &s, &out);
    let inf = info(&out);
    assert_eq!((inf.channels, inf.sample_rate), (1, 44_100));
    assert!((inf.duration - 2.0).abs() < 0.002);
    assert_eq!(unit_heard(&decode_audio(&out), 1.1, 1.4), Some(2));
}

#[test]
fn video_layer_speed_and_source_offset() {
    let (mut e, _) = coded_editor(12); // 6 s base
    let src = add_asset(&mut e, &coded_source(124));
    // Play the coded source from 10 s (unit 20) at double speed for 2 s of
    // composition time: each unit lasts 0.25 s, units 20..28.
    let mut clip = MediaClip::new(src);
    clip.src_in = 10.0;
    clip.speed = 2.0;
    let mut layer = Layer::new("Fast", 1.0, 2.0, LayerContent::Video(clip));
    layer.transform = Transform { x: 0.75, y: 0.5, w: 0.5, h: 1.0, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer }).unwrap();
    // Silence the script so only the layer is heard.
    e.apply(Command::SetScriptAudio { audio: AudioSettings { muted: true, ..AudioSettings::default() } }).unwrap();
    let (v, a, _d) = export_video(&e, &small(&e), "speed.mp4");
    assert_eq!(v.frames.len(), 180);
    for j in 0..8 {
        // Middle of the j-th quarter second of the layer.
        let t = 1.0 + j as f64 * 0.25 + 0.125;
        let f = (t * 30.0) as usize;
        assert_eq!(unit_of_color(v.at(f, 0.75, 0.5)), Some(20 + j), "layer picture at {t}s");
        assert_eq!(unit_of_color(v.at(f, 0.2, 0.5)), Some((t / UNIT) as usize), "script picture beside the layer at {t}s");
    }
    assert_eq!(unit_of_color(v.at(29, 0.75, 0.5)), Some(1), "before the layer starts the script picture shows through");
    assert_eq!(unit_of_color(v.at(90, 0.75, 0.5)), Some(6), "the layer is gone on the frame it ends");
    assert_eq!(unit_of_color(v.at(89, 0.75, 0.5)), Some(27), "and still there the frame before");
    // atempo keeps pitch: unit 20's tone is heard, for a quarter second each.
    for j in [0usize, 3, 7] {
        let t = 1.0 + j as f64 * 0.25;
        assert_eq!(unit_heard(&a, t + 0.06, t + 0.2), Some(20 + j), "layer sound at {t}s");
    }
    assert!(rms(&a, 0.1, 0.9) < 1e-3 && rms(&a, 3.2, 5.9) < 1e-3, "nothing but the layer is audible");
}

#[test]
fn looping_layer_repeats_its_source() {
    let (mut e, _) = coded_editor(12);
    let short = add_asset(&mut e, &coded_source(2)); // 1 s: units 0 and 1
    let mut clip = MediaClip::new(short);
    clip.loop_playback = true;
    let mut layer = Layer::new("Loop", 1.0, 3.5, LayerContent::Video(clip));
    layer.transform = Transform { x: 0.75, y: 0.5, w: 0.5, h: 1.0, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer }).unwrap();
    e.apply(Command::SetScriptAudio { audio: AudioSettings { muted: true, ..AudioSettings::default() } }).unwrap();
    let (v, a, _d) = export_video(&e, &small(&e), "loop.mp4");
    for j in 0..7 {
        let t = 1.0 + j as f64 * 0.5 + 0.25;
        assert_eq!(unit_of_color(v.at((t * 30.0) as usize, 0.75, 0.5)), Some(j % 2), "looped picture at {t}s");
        assert_eq!(unit_heard(&a, t - 0.12, t + 0.12), Some(j % 2), "looped sound at {t}s");
    }
    assert_eq!(unit_of_color(v.at((4.6 * 30.0) as usize, 0.75, 0.5)), Some(9), "loop ends with its layer; the script shows again");
    assert!(rms(&a, 4.6, 5.9) < 1e-3);
}

#[test]
fn layer_crop_keeps_only_the_requested_part() {
    let (mut e, _) = coded_editor(4);
    let q = add_asset(&mut e, &quadrants());
    let mut clip = MediaClip::new(q);
    clip.audio.muted = true;
    clip.crop = Crop { left: 0.5, top: 0.5, right: 0.0, bottom: 0.0 }; // bottom-right quadrant: white
    clip.fit = FitMode::Stretch;
    let mut layer = Layer::new("Crop", 0.0, 2.0, LayerContent::Video(clip));
    layer.transform = Transform { x: 0.5, y: 0.5, w: 0.5, h: 0.5, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer }).unwrap();
    let (v, _, _d) = export_video(&e, &small(&e), "crop.mp4");
    let white = v.count_in(30, 0.0, 0.0, 1.0, 1.0, |p| quad(p) == 'W');
    assert!((white as i64 - 160 * 90).abs() < 600, "the cropped quadrant should fill the 160x90 box, {white} px");
    assert_eq!(v.count_in(30, 0.0, 0.0, 1.0, 1.0, |p| matches!(quad(p), 'R' | 'G' | 'B')), 0, "the other quadrants are cropped away");
}

#[test]
fn rotation_and_opacity_apply_to_media_layers() {
    let (mut e, _) = editor_with_source(&coded_audio(4), None);
    e.apply(Command::SetCanvas { width: 320, height: 180, fill: false }).unwrap();
    e.apply(Command::SetBackground { color: Rgba::rgb(255, 0, 0) }).unwrap();
    let png = solid_png("blue_100x20.png", "0x0000FF", 100, 20);
    let id = add_asset(&mut e, &png);
    let mk = |x: f64, rotation_deg: f64, opacity: f64| {
        let mut l = Layer::new("img", 0.0, 2.0, LayerContent::Image(ImageClip { asset: id, crop: Crop::default(), fit: FitMode::Fit, effects: vec![], zoom_to: 1.0 }));
        // Box 100x20 px.
        l.transform = Transform { x, y: 0.5, w: 100.0 / 320.0, h: 20.0 / 180.0, rotation_deg, opacity };
        l
    };
    e.apply(Command::AddLayer { layer: mk(0.25, 90.0, 1.0) }).unwrap();
    e.apply(Command::AddLayer { layer: mk(0.75, 0.0, 0.5) }).unwrap();
    let (v, _, _d) = export_video(&e, &settings(&e), "rot.mp4");
    // Left: rotated a quarter turn -> 20 wide, 100 tall, centred at (80, 90).
    let (x0, y0, x1, y1) = v.bbox(30, |p| is_blue(p) && true).map(|b| b).unwrap();
    let left_box = {
        let mut b: Option<(usize, usize, usize, usize)> = None;
        for y in 0..v.h {
            for x in 0..160 {
                if is_blue(v.pixel(30, x, y)) {
                    b = Some(match b { None => (x, y, x, y), Some((a, c, d, e2)) => (a.min(x), c.min(y), d.max(x), e2.max(y)) });
                }
            }
        }
        b.expect("rotated image visible")
    };
    let _ = (x0, y0, x1, y1);
    let near = |a: usize, b: usize| (a as i64 - b as i64).abs() <= 3;
    assert!(near(left_box.0, 70) && near(left_box.2, 89) && near(left_box.1, 40) && near(left_box.3, 139), "rotated box {left_box:?}");
    // Right: half-transparent blue over red -> purple.
    assert_color(v.pixel(30, 240, 90), [128, 0, 128], 22, "50% blue over red");
    assert_eq!(quad(v.pixel(30, 240, 60)), 'R', "background outside the layer");
}

#[test]
fn ken_burns_zoom_grows_the_image_around_its_centre() {
    let (mut e, _) = coded_editor(8);
    let png = solid_png("green_100.png", "0x00FF00", 100, 100);
    let id = add_asset(&mut e, &png);
    let mut l = Layer::new("zoom", 0.0, 4.0, LayerContent::Image(ImageClip { asset: id, crop: Crop::default(), fit: FitMode::Fit, effects: vec![], zoom_to: 2.0 }));
    l.transform = Transform { x: 0.5, y: 0.5, w: 0.25, h: 0.25, rotation_deg: 0.0, opacity: 1.0 }; // 45x45 px image
    e.apply(Command::AddLayer { layer: l }).unwrap();
    let (v, _, _d) = export_video(&e, &small(&e), "zoom.mp4");
    let size = |f: usize| {
        let (x0, y0, x1, y1) = v.bbox(f, is_green).unwrap();
        ((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64, (x0 + x1) as f64 / 2.0, (y0 + y1) as f64 / 2.0)
    };
    let (w0, h0, cx0, cy0) = size(1);
    let (w1, _, _, _) = size(60);
    let (w2, h2, cx2, cy2) = size(118);
    assert!((w0 - 45.0).abs() <= 3.0 && (h0 - 45.0).abs() <= 3.0, "starts at its box size, {w0}x{h0}");
    assert!((w1 - 67.5).abs() <= 4.0, "halfway it is 1.5x, {w1}");
    assert!((w2 - 90.0).abs() <= 4.0 && (h2 - 90.0).abs() <= 4.0, "ends at 2x, {w2}x{h2}");
    for (cx, cy) in [(cx0, cy0), (cx2, cy2)] {
        assert!((cx - 160.0).abs() <= 2.0 && (cy - 90.0).abs() <= 2.0, "stays centred, centre ({cx}, {cy})");
    }
}

#[test]
fn slide_and_fade_animations_move_and_fade_layers() {
    let (mut e, _) = editor_with_source(&coded_audio(8), None);
    e.apply(Command::SetCanvas { width: 320, height: 180, fill: false }).unwrap();
    let shape = |color: Rgba, y: f64| {
        let mut l = Layer::new("Box", 1.0, 2.0, LayerContent::Shape(Shape { kind: ShapeKind::Rectangle, fill: color, stroke: None, corner_radius: 0.0 }));
        l.transform = Transform { x: 0.5, y, w: 0.25, h: 0.25, rotation_deg: 0.0, opacity: 1.0 };
        l
    };
    let mut slide = shape(Rgba::rgb(0, 0, 255), 0.25);
    slide.animation_in = Animation::SlideLeft; // enters from the right
    slide.animation_out = Animation::SlideLeft; // leaves to the left
    e.apply(Command::AddLayer { layer: slide.clone() }).unwrap();
    let mut fade = shape(Rgba::rgb(0, 255, 0), 0.75);
    fade.animation_in = Animation::Fade;
    fade.animation_out = Animation::Fade;
    e.apply(Command::AddLayer { layer: fade }).unwrap();
    let (v, _, _d) = export_video(&e, &settings(&e), "anim.mp4");
    let centre_x = |f: usize, pred: fn([u8; 3]) -> bool| v.bbox(f, pred).map(|(x0, _, x1, _)| (x0 + x1) as f64 / 2.0);
    // Settled in the middle of its life.
    let mid = centre_x(60, is_blue).expect("box visible mid-life");
    assert!((mid - 160.0).abs() <= 2.0, "settled at its transform position, x={mid}");
    // Shortly after the start it is still to the right, and moving left.
    let early = centre_x(32, is_blue).expect("box entering");
    let later = centre_x(36, is_blue).expect("box entering");
    assert!(early > later && later > mid + 5.0, "slides in from the right: {early} -> {later} -> {mid}");
    // Near the end it has moved left of centre.
    let leaving = centre_x(88, is_blue).expect("box leaving");
    assert!(leaving < mid - 20.0, "slides out to the left: {leaving}");
    // The fading box is partly transparent early and late, solid in the middle.
    let g = |f: usize| v.pixel(f, 160, 135)[1];
    assert!(g(60) > 235, "solid mid-life, green {}", g(60));
    assert!(g(33) > 20 && g(33) < 200, "fading in, green {}", g(33));
    assert!(g(87) > 20 && g(87) < 200, "fading out, green {}", g(87));
    // The preview compositor matches the export at the same instants.
    let cues = [];
    for f in [32usize, 36, 60, 88] {
        let frame = mango_media::raster::render_overlay(&e.project, e.comp().unwrap(), &cues, f as f64 / 30.0, 320, 180);
        let mut xs = (usize::MAX, 0usize);
        for y in 0..90u32 {
            for x in 0..320u32 {
                let p = frame.pixel(x, y);
                if p[3] > 128 && p[2] > 200 {
                    xs = (xs.0.min(x as usize), xs.1.max(x as usize));
                }
            }
        }
        let preview_cx = (xs.0 + xs.1) as f64 / 2.0;
        let export_cx = centre_x(f, is_blue).unwrap();
        // The export clips at the canvas edge, so compare only when fully on screen.
        if xs.0 > 0 && xs.1 < 319 {
            assert!((preview_cx - export_cx).abs() <= 3.0, "frame {f}: preview x={preview_cx}, export x={export_cx}");
        }
    }
}

#[test]
fn volume_automation_and_fades_shape_layer_audio() {
    let (mut e, _) = editor_with_source(&coded_audio(12), None); // 6 s
    e.apply(Command::SetScriptAudio { audio: AudioSettings { muted: true, ..AudioSettings::default() } }).unwrap();
    let music = tone_wav("tone_2000_quarter.wav", 2000, 8.0, 0.25);
    let id = add_asset(&mut e, &music);
    let mut clip = MediaClip::new(id);
    // 0 dB until 1 s, down to -20 dB by 2 s, hold.
    clip.volume_keys = vec![(1.0, 0.0), (2.0, -20.0)];
    let mut layer = Layer::new("Music", 1.0, 4.0, LayerContent::Audio(clip));
    layer.fade_out = 1.0;
    e.apply(Command::AddLayer { layer }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("auto.wav");
    let mut s = settings(&e);
    s.video = None;
    s.audio = Some(AudioCodec::Wav);
    export_to(&e, &s, &out);
    let a = decode_audio(&out);
    let lvl = |t0: f64, t1: f64| tone_level(&a, 2000.0, t0, t1);
    let full = lvl(1.2, 1.9); // layer time 0.2..0.9
    let low = lvl(3.2, 3.8); // layer time 2.2..2.8, after the ramp
    assert!(full > 0.01);
    let db = 20.0 * (low / full).log10();
    assert!((db + 20.0).abs() < 1.5, "automation should lower the level by 20 dB, got {db:.1}");
    let mid = lvl(2.45, 2.55); // layer time 1.5: halfway, -10 dB
    let mid_db = 20.0 * (mid / full).log10();
    assert!((mid_db + 10.0).abs() < 2.0, "halfway through the ramp is -10 dB, got {mid_db:.1}");
    // Fade out over the last second (4.0..5.0): quieter still, then silent.
    assert!(lvl(4.7, 4.95) < low * 0.4, "fading out");
    assert!(lvl(5.1, 5.9) < full * 0.01, "silent after the layer");
    assert!(lvl(0.1, 0.9) < full * 0.01, "silent before the layer");
}

#[test]
fn script_video_placement_and_per_scene_override() {
    let (mut e, _) = quad_editor(); // 3 s
    e.apply(Command::SetCanvas { width: 320, height: 176, fill: false }).unwrap();
    e.apply(Command::SetBackground { color: Rgba::rgb(0, 0, 0) }).unwrap();
    // Picture-in-picture: bottom-right quarter of the canvas.
    let pip = VideoPlacement { transform: Transform { x: 0.75, y: 0.75, w: 0.5, h: 0.5, rotation_deg: 0.0, opacity: 1.0 }, ..VideoPlacement::default() };
    e.apply(Command::SetScriptVideo { placement: pip }).unwrap();
    // Second scene (from 1.5 s) overrides: full canvas, hidden picture.
    e.apply(Command::SplitAt { time: 1.5 }).unwrap();
    let at = e.comp().unwrap().token_at_time(1.6).unwrap();
    e.apply(Command::SetSceneBreak { at, on: true }).unwrap();
    let full = VideoPlacement { hidden: true, ..VideoPlacement::default() };
    e.apply(Command::SetScene { scene: 1, settings: SceneSettings { script_video: Some(full), ..SceneSettings::default() } }).unwrap();
    let (v, a, _d) = export_video(&e, &settings(&e), "pip.mp4");
    assert_eq!(v.frames.len(), 90);
    // Scene 1: quadrants inside the bottom-right quarter, black elsewhere.
    let q = |f: usize, fx: f64, fy: f64| quad(v.at(f, fx, fy));
    assert_eq!([q(20, 0.625, 0.625), q(20, 0.875, 0.625), q(20, 0.625, 0.875), q(20, 0.875, 0.875)], ['R', 'G', 'B', 'W']);
    assert_eq!([q(20, 0.25, 0.25), q(20, 0.75, 0.25), q(20, 0.25, 0.75)], ['K', 'K', 'K']);
    // Scene 2: picture hidden, sound continues.
    assert_eq!([q(60, 0.25, 0.25), q(60, 0.875, 0.875), q(60, 0.625, 0.625)], ['K', 'K', 'K']);
    assert_eq!(q(44, 0.875, 0.875), 'W', "scene 1 runs up to the cut");
    assert_eq!(q(45, 0.875, 0.875), 'K', "scene 2 starts on the frame of the scene break");
    assert!(tone_level(&a, 1000.0, 1.6, 2.9) > 0.05);
}

#[test]
fn layers_can_outlast_the_script() {
    let (mut e, _) = coded_editor(4); // 2 s of script
    e.apply(Command::SetBackground { color: Rgba::rgb(0, 255, 0) }).unwrap();
    let mut shape = Layer::new("Box", 1.0, 3.0, LayerContent::Shape(Shape { kind: ShapeKind::Rectangle, fill: Rgba::rgb(0, 0, 255), stroke: None, corner_radius: 0.0 }));
    shape.transform = Transform { x: 0.25, y: 0.5, w: 0.25, h: 0.5, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer: shape }).unwrap();
    let music = tone_wav("tone_2000_quarter.wav", 2000, 8.0, 0.25);
    let id = add_asset(&mut e, &music);
    e.apply(Command::AddLayer { layer: Layer::new("Music", 0.0, 4.0, LayerContent::Audio(MediaClip::new(id))) }).unwrap();
    assert!((e.comp().unwrap().duration() - 4.0).abs() < 1e-9);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("tail.mp4");
    export_to(&e, &small(&e), &out);
    let inf = info(&out);
    assert_eq!(inf.frames, 120, "the export runs to the end of the last layer");
    assert!((inf.duration - 4.0).abs() <= FRAME);
    let v = decode_video(&out);
    let a = decode_audio(&out);
    assert!((a.len() as f64 / RATE as f64 - 4.0).abs() <= FRAME);
    assert_eq!(unit_of_color(v.at(30, 0.75, 0.5)), Some(2), "script picture while the script lasts");
    assert_eq!(unit_of_color(v.at(59, 0.75, 0.5)), Some(3));
    for f in [60, 90, 119] {
        assert!(is_green(v.at(f, 0.75, 0.5)), "background after the script ends, frame {f}: {:?}", v.at(f, 0.75, 0.5));
        assert!(is_blue(v.at(f, 0.25, 0.5)), "the layer is still showing on frame {f}");
    }
    assert!(tone_level(&a, 2000.0, 2.5, 3.9) > 0.01, "layer audio continues after the script");
    assert!(tone_level(&a, unit_freq(3), 2.5, 3.9) < 0.005);
}

#[test]
fn composition_with_only_layers_exports() {
    let mut e = Editor::new_project("Layers only");
    e.apply(Command::SetCanvas { width: 320, height: 180, fill: false }).unwrap();
    e.apply(Command::SetBackground { color: Rgba::rgb(0, 255, 0) }).unwrap();
    let mut title = Layer::new("Title", 0.0, 2.0, LayerContent::Text(TextStyle { text: "Hi".into(), size: 0.4, color: Rgba::rgb(0, 0, 255), shadow: false, ..TextStyle::default() }));
    title.transform = Transform::default();
    e.apply(Command::AddLayer { layer: title }).unwrap();
    let music = tone_wav("tone_2000_quarter.wav", 2000, 8.0, 0.25);
    let id = add_asset(&mut e, &music);
    e.apply(Command::AddLayer { layer: Layer::new("Music", 0.5, 1.0, LayerContent::Audio(MediaClip::new(id))) }).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("layers.mp4");
    export_to(&e, &settings(&e), &out);
    let inf = info(&out);
    assert_eq!(inf.frames, 60);
    assert_eq!((inf.width, inf.height), (320, 180));
    let v = decode_video(&out);
    assert!(is_green(v.at(30, 0.05, 0.05)));
    assert!(v.count_in(30, 0.2, 0.2, 0.8, 0.8, is_blue) > 300, "title is drawn");
    let a = decode_audio(&out);
    assert!((a.len() as f64 / RATE as f64 - 2.0).abs() <= FRAME);
    assert!(tone_level(&a, 2000.0, 0.6, 1.4) > 0.01);
    assert!(tone_level(&a, 2000.0, 1.6, 1.95) < 0.001 && tone_level(&a, 2000.0, 0.05, 0.45) < 0.001);
}

#[test]
fn scene_shorter_than_a_frame_does_not_break_the_export() {
    let (mut e, _) = coded_editor(6); // 3 s
    // A 10 ms sliver as its own scene: 1.000 .. 1.010.
    e.apply(Command::SplitAt { time: 1.0 }).unwrap();
    e.apply(Command::SplitAt { time: 1.01 }).unwrap();
    let a = e.comp().unwrap().token_at_time(1.005).unwrap();
    e.apply(Command::SetSceneBreak { at: a, on: true }).unwrap();
    e.apply(Command::SetSceneBreak { at: a + 1, on: true }).unwrap();
    assert_eq!(e.comp().unwrap().scene_spans().len(), 3);
    let chunks = plan_chunks(e.comp().unwrap(), &settings(&e));
    assert!(chunks.iter().any(|c| c.frames == 0), "test needs a zero-frame chunk: {:?}", chunks.iter().map(|c| c.frames).collect::<Vec<_>>());
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("sliver.mp4");
    export_to(&e, &settings(&e), &out);
    let inf = info(&out);
    assert_eq!(inf.frames, 90);
    let v = decode_video(&out);
    let au = decode_audio(&out);
    assert!((au.len() as f64 / RATE as f64 - 3.0).abs() <= FRAME);
    for k in 0..6 {
        assert_eq!(unit_of_color(v.mean(k * 15 + 7)), Some(k));
        assert_eq!(unit_heard(&au, k as f64 * UNIT + 0.1, k as f64 * UNIT + 0.4), Some(k));
    }
}

#[test]
fn rotated_phone_video_exports_upright() {
    // The stored frames are 160x88 with a 90 degree rotation flag, as phones record portrait video.
    let rotated = fixture("quadrants_rot90.mp4", |out| {
        ff(&["-display_rotation", "90", "-i", quadrants().to_str().unwrap(), "-c", "copy", "-f", "mp4", out.to_str().unwrap()]);
    });
    let (e, id) = editor_with_source(&rotated, None);
    let asset = e.project.asset(id).unwrap();
    assert_eq!((asset.width, asset.height), (88, 160), "probe reports display size");
    assert_eq!((e.comp().unwrap().width, e.comp().unwrap().height), (88, 160), "the composition adopts the upright size");
    let (v, _, _d) = export_video(&e, &settings(&e), "upright.mp4");
    assert_eq!((v.w, v.h), (88, 160));
    // The preview frame grabber agrees with the export about orientation.
    let frame = mango_media::frames::frame_at(&rotated, 1.0, 88, 160).unwrap();
    assert_eq!((frame.width, frame.height), (88, 160));
    let corners_export = [quad(v.at(30, 0.25, 0.25)), quad(v.at(30, 0.75, 0.25)), quad(v.at(30, 0.25, 0.75)), quad(v.at(30, 0.75, 0.75))];
    let fp = |fx: f64, fy: f64| {
        let p = frame.pixel((fx * 88.0) as u32, (fy * 160.0) as u32);
        quad([p[0], p[1], p[2]])
    };
    let corners_frame = [fp(0.25, 0.25), fp(0.75, 0.25), fp(0.25, 0.75), fp(0.75, 0.75)];
    assert_eq!(corners_export, corners_frame);
    // All four quadrants are present, each filling a quarter, in a rotated arrangement.
    let mut sorted = corners_export;
    sorted.sort();
    assert_eq!(sorted, ['B', 'G', 'R', 'W']);
    assert_ne!(corners_export, ['R', 'G', 'B', 'W'], "the picture must actually be rotated");
}

#[test]
fn spoken_audio_from_say_exports_and_keeps_its_length() {
    let speech = fixture("speech.aiff", |out| {
        let status = std::process::Command::new("/usr/bin/say").args(["-o", out.to_str().unwrap(), "Mango Loco edits video like a document."]).status().expect("run say");
        assert!(status.success(), "say failed");
    });
    let (e, id) = editor_with_source(&speech, None);
    let asset = e.project.asset(id).unwrap().clone();
    assert_eq!(asset.kind, MediaKind::Audio);
    assert!(asset.duration > 1.0 && asset.duration < 10.0, "speech duration {}", asset.duration);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("speech.m4a");
    let mut s = settings(&e);
    s.video = None;
    export_to(&e, &s, &out);
    let inf = info(&out);
    assert!((inf.duration - asset.duration).abs() < 0.03, "{} vs {}", inf.duration, asset.duration);
    let a = decode_audio(&out);
    let original = decode_audio(&speech);
    assert!(rms(&a, 0.0, asset.duration) > 0.01, "speech is audible");
    // Same loudness contour as the source, moment by moment.
    let mut compared = 0;
    let mut t = 0.0;
    while t + 0.1 < asset.duration {
        let (x, y) = (rms(&original, t, t + 0.1), rms(&a, t, t + 0.1));
        if x > 0.02 {
            assert!((20.0 * (y / x).log10()).abs() < 3.0, "level at {t:.1}s differs: source {x:.4}, export {y:.4}");
            compared += 1;
        }
        t += 0.1;
    }
    assert!(compared >= 5, "the speech should have at least half a second of voiced audio, had {compared} windows");
}

#[test]
fn hardware_encoders_work_when_available() {
    let (e, _) = coded_editor(4);
    for (codec, name) in [(VideoCodec::H264, "h264"), (VideoCodec::Hevc, "hevc")] {
        // VideoToolbox is not usable in every environment (CI, sandboxes): ask ffmpeg directly first.
        let probe_out = tempfile::tempdir().unwrap();
        let probe_file = probe_out.path().join("probe.mp4");
        let available = std::process::Command::new(ffmpeg_bin())
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=red:s=320x180:r=30:d=0.5", "-c:v", &format!("{name}_videotoolbox"), "-b:v", "1000k", "-pix_fmt", "yuv420p"])
            .arg(&probe_file)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !available {
            eprintln!("skipping {name}_videotoolbox: the hardware encoder is not usable here");
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join(format!("hw_{name}.mp4"));
        let mut s = small(&e);
        s.video = Some(codec);
        s.hardware = true;
        export_to(&e, &s, &out);
        let inf = info(&out);
        assert_eq!(inf.vcodec.as_deref(), Some(name));
        assert_eq!(inf.frames, 60);
        let v = decode_video(&out);
        for k in 0..4 {
            assert!(close(v.at(k * 15 + 7, 0.5, 0.5), unit_color(k), 20), "{name}: unit {k} colour {:?}", v.at(k * 15 + 7, 0.5, 0.5));
        }
    }
}

#[test]
fn peak_normalisation_brings_the_mix_to_its_ceiling() {
    for (gain_db, target_db) in [(-20.0, -6.0), (0.0, -12.0), (-30.0, -1.0)] {
        let (mut e, _) = coded_editor(12);
        e.apply(Command::SetScriptAudio { audio: AudioSettings { gain_db, ..AudioSettings::default() } }).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("peak.wav");
        let mut s = settings(&e);
        s.video = None;
        s.audio = Some(AudioCodec::Wav);
        s.normalize = Normalize::Peak(target_db);
        export_to(&e, &s, &out);
        assert!((info(&out).duration - 6.0).abs() < 0.002);
        // Peak over both channels.
        let peak = (0..2).map(|c| decode_audio_channel(&out, c).iter().fold(0f32, |m, v| m.max(v.abs()))).fold(0f32, f32::max);
        let peak_db = 20.0 * (peak as f64).log10();
        assert!((peak_db - target_db).abs() < 0.5, "source at {gain_db} dB normalised to {target_db} dBFS peaks at {peak_db:.2} dBFS");
        // Still the same audio.
        assert_eq!(unit_heard(&decode_audio(&out), 1.1, 1.4), Some(2));
        assert_eq!(listing(dir.path()), ["peak.wav"]);
    }
}

#[test]
fn many_text_layers_export_together() {
    let (mut e, _) = coded_editor(6);
    for i in 0..12 {
        let mut l = Layer::new(format!("T{i}"), 0.0, 3.0, LayerContent::Text(TextStyle { text: "X".into(), size: 0.15, color: Rgba::rgb(0, 0, 255), shadow: false, ..TextStyle::default() }));
        l.transform = Transform { x: (i % 4) as f64 * 0.25 + 0.125, y: (i / 4) as f64 * 0.33 + 0.17, w: 0.25, h: 0.3, rotation_deg: 0.0, opacity: 1.0 };
        e.apply(Command::AddLayer { layer: l }).unwrap();
    }
    let (v, _, _d) = export_video(&e, &small(&e), "many.mp4");
    for i in 0..12 {
        let (x0, y0) = ((i % 4) as f64 * 0.25, (i / 4) as f64 * 0.33);
        let n = v.count_in(45, x0, y0, x0 + 0.25, y0 + 0.34, is_blue);
        assert!(n > 40, "text layer {i} should be drawn in its cell, found {n} px");
    }
}
