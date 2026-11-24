//! The reference mixer against projects edited through `mango_core::Editor`.

mod common;
use common::*;
use mango_core::*;
use mango_playback::mix::{self, AudioSegment, MICROFADE, Timeline};
use mango_playback::pcm::{Pcm, PcmCache};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

struct Fixture {
    _dir: tempfile::TempDir,
    cache: PcmCache,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let cache = PcmCache::with_dir(dir.path());
        Self { _dir: dir, cache }
    }
    fn load(&self, path: &Path) -> Arc<Pcm> {
        self.cache.ensure(path, &mut |_| {}, &AtomicBool::new(false)).unwrap().expect("has audio")
    }
    fn timeline(&self, e: &Editor) -> Timeline {
        mix::timeline(&e.project, e.comp().unwrap(), &self.cache)
    }
    fn render(&self, e: &Editor) -> Vec<[f32; 2]> {
        render_all(&self.timeline(e))
    }
}

fn render_all(tl: &Timeline) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; tl.frames() as usize];
    mix::render(tl, 0, &mut out);
    out
}

fn source(pcm: &Pcm) -> Vec<[f32; 2]> {
    let mut v = vec![[0.0f32; 2]; pcm.frames() as usize];
    pcm.read(0, &mut v);
    v
}

/// The 8 s stepped tone as a script with one word per second.
fn tone_script(fx: &Fixture) -> (Editor, AssetId, Arc<Pcm>) {
    let wav = tone_wav(8);
    let pcm = fx.load(&wav);
    let (e, id) = editor_with_script(asset_per_second(&wav, MediaKind::Audio, 8));
    (e, id, pcm)
}

/// An empty composition plus the asset, for layer tests.
fn layer_project(fx: &Fixture, path: &Path, duration: f64) -> (Editor, AssetId, Arc<Pcm>) {
    let pcm = fx.load(path);
    let asset = asset_with_words(path, MediaKind::Audio, duration, Vec::new());
    let id = asset.id;
    let mut e = Editor::new_project("Layers");
    e.apply(Command::AddAsset { asset }).unwrap();
    (e, id, pcm)
}

fn add_layer(e: &mut Editor, start: f64, duration: f64, clip: MediaClip, tweak: impl FnOnce(&mut Layer)) -> LayerId {
    let mut layer = Layer::new("layer", start, duration, LayerContent::Audio(clip));
    tweak(&mut layer);
    let id = layer.id;
    e.apply(Command::AddLayer { layer }).unwrap();
    id
}

fn assert_close(a: f32, b: f32, tol: f32, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b}");
}

/// Dominant frequency of every whole second of `out`, measured away from the edges.
fn freqs_per_second(out: &[[f32; 2]]) -> Vec<f64> {
    (0..out.len() / SR).map(|s| dominant_freq(span(out, s as f64 + 0.1, s as f64 + 0.9), 0)).collect()
}

#[test]
fn an_unedited_script_plays_the_source() {
    let fx = Fixture::new();
    let (e, _, pcm) = tone_script(&fx);
    let tl = fx.timeline(&e);
    assert_eq!(tl.duration, 8.0);
    assert_eq!(tl.segments.len(), 1);
    let out = render_all(&tl);
    let src = source(&pcm);
    assert_eq!(out.len(), src.len());
    // Identical except for the microfades at the very start and end.
    let fade = (MICROFADE * SR as f64) as usize;
    assert_eq!(&out[fade..out.len() - fade], &src[fade..src.len() - fade]);
    assert_eq!(freqs_per_second(&out), (0..8).map(step_freq).collect::<Vec<_>>());
}

#[test]
fn deleting_a_word_plays_what_follows_it() {
    let fx = Fixture::new();
    let (mut e, _, _) = tone_script(&fx);
    let i = word_index(&e, "w2");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let tl = fx.timeline(&e);
    assert_eq!(tl.duration, 7.0);
    let out = render_all(&tl);
    assert_eq!(out.len(), 7 * SR);
    // Output second 2 now holds source second 3, and so on.
    assert_eq!(freqs_per_second(&out), [0, 1, 3, 4, 5, 6, 7].map(step_freq));

    // Undo brings it back.
    assert!(e.undo());
    assert_eq!(freqs_per_second(&fx.render(&e)), (0..8).map(step_freq).collect::<Vec<_>>());
}

#[test]
fn ignored_text_is_skipped_like_deleted_text() {
    let fx = Fixture::new();
    let (mut e, _, _) = tone_script(&fx);
    let i = word_index(&e, "w5");
    e.apply(Command::IgnoreRange { start: i, end: i + 2, ignored: true }).unwrap();
    assert_eq!(freqs_per_second(&fx.render(&e)), [0, 1, 2, 3, 4, 7].map(step_freq));
}

#[test]
fn a_gap_clip_is_digital_silence() {
    let fx = Fixture::new();
    let (mut e, _, _) = tone_script(&fx);
    let i = word_index(&e, "w1");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 1 }).unwrap();
    let out = fx.render(&e);
    assert_eq!(out.len(), 8 * SR);
    assert!(span(&out, 1.0, 2.0).iter().all(|s| *s == [0.0, 0.0]), "the gap is exactly zero");
    assert_eq!(dominant_freq(span(&out, 0.1, 0.9), 0), step_freq(0));
    assert_eq!(dominant_freq(span(&out, 2.1, 2.9), 0), step_freq(2));

    // An inserted gap pushes everything after it later.
    let (mut e, _, _) = tone_script(&fx);
    let i = word_index(&e, "w3");
    e.apply(Command::InsertGap { at: i, secs: 1.5 }).unwrap();
    let out = fx.render(&e);
    assert_eq!(out.len(), (9.5 * SR as f64) as usize);
    assert!(span(&out, 3.0, 4.5).iter().all(|s| *s == [0.0, 0.0]));
    assert_eq!(dominant_freq(span(&out, 4.6, 5.4), 0), step_freq(3));
}

#[test]
fn moved_text_plays_in_the_new_order() {
    let fx = Fixture::new();
    let (mut e, id, _) = tone_script(&fx);
    let i = word_index(&e, "w5");
    e.apply(Command::MoveRange { start: i, end: i + 2, dest: 0 }).unwrap();
    let words: Vec<String> = e.comp().unwrap().script.iter().filter(|t| t.is_word()).map(|t| t.text.clone()).collect();
    assert_eq!(words, ["w5", "w6", "w0", "w1", "w2", "w3", "w4", "w7"]);
    let out = fx.render(&e);
    assert_eq!(freqs_per_second(&out), [5, 6, 0, 1, 2, 3, 4, 7].map(step_freq));
    // The model's own time mapping agrees with what is heard.
    for sec in 0..8 {
        let (asset, src) = e.comp().unwrap().source_at(sec as f64 + 0.5).unwrap();
        assert_eq!(asset, id);
        assert_eq!(dominant_freq(span(&out, sec as f64 + 0.1, sec as f64 + 0.9), 0), step_freq(src.floor() as usize));
    }
}

#[test]
fn script_gain_mute_and_pan() {
    let fx = Fixture::new();
    let (mut e, _, _) = tone_script(&fx);
    let reference = fx.render(&e);
    let mid = |v: &[[f32; 2]]| -> Vec<[f32; 2]> { span(v, 1.0, 7.0).to_vec() };

    // -6 dB is a factor of 0.501.
    e.apply(Command::SetScriptAudio { audio: AudioSettings { gain_db: -6.0, ..Default::default() } }).unwrap();
    let quiet = fx.render(&e);
    let ratio = rms(&mid(&quiet), 0) / rms(&mid(&reference), 0);
    assert!((ratio - 10f64.powf(-6.0 / 20.0)).abs() < 1e-4, "gain ratio {ratio}");
    assert_close(peak(&mid(&quiet), 1), peak(&mid(&reference), 1) * 0.5012, 1e-3, "peak");

    // +6 dB doubles (the mixer itself does not clip).
    e.apply(Command::SetScriptAudio { audio: AudioSettings { gain_db: 6.0206, ..Default::default() } }).unwrap();
    let loud = fx.render(&e);
    assert_close(peak(&mid(&loud), 0), 2.0 * peak(&mid(&reference), 0), 1e-3, "boost");

    // Hard left: the right channel is exactly zero, the left untouched.
    e.apply(Command::SetScriptAudio { audio: AudioSettings { pan: -1.0, ..Default::default() } }).unwrap();
    let left = fx.render(&e);
    assert!(left.iter().all(|s| s[1] == 0.0));
    assert!(left.iter().zip(&reference).all(|(a, b)| a[0] == b[0]));

    // Hard right, and half right.
    e.apply(Command::SetScriptAudio { audio: AudioSettings { pan: 1.0, ..Default::default() } }).unwrap();
    let right = fx.render(&e);
    assert!(right.iter().all(|s| s[0] == 0.0));
    assert!(right.iter().zip(&reference).all(|(a, b)| a[1] == b[1]));
    e.apply(Command::SetScriptAudio { audio: AudioSettings { pan: 0.5, ..Default::default() } }).unwrap();
    let half = fx.render(&e);
    assert!((rms(&mid(&half), 0) / rms(&mid(&reference), 0) - 0.5).abs() < 1e-4);
    assert!(half.iter().zip(&reference).all(|(a, b)| a[1] == b[1]));

    // Mute: silence, but the composition keeps its length.
    e.apply(Command::SetScriptAudio { audio: AudioSettings { muted: true, ..Default::default() } }).unwrap();
    let tl = fx.timeline(&e);
    assert_eq!(tl.duration, 8.0);
    assert!(render_all(&tl).iter().all(|s| *s == [0.0, 0.0]));
}

#[test]
fn a_cut_does_not_click() {
    let fx = Fixture::new();
    let wav = tone_wav(8);
    let pcm = fx.load(&wav);
    // Cut points chosen where the waveform is at opposite extremes, so a
    // plain splice would jump by the full amplitude.
    let (cut_from, cut_to) = (2.0005, 3.00125);
    let word = |text: &str, start: f64, end: f64| TranscriptWord { text: text.into(), start, end, speaker: None, confidence: 1.0 };
    let asset = asset_with_words(&wav, MediaKind::Audio, 8.0, vec![word("keep", 0.0, cut_from), word("cut", cut_from, cut_to), word("rest", cut_to, 8.0)]);
    let (mut e, _) = editor_with_script(asset);
    let i = word_index(&e, "cut");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    assert_eq!(e.comp().unwrap().script_clips().len(), 2);

    let src = source(&pcm);
    let before = src[(cut_from * SR as f64).round() as usize - 1][0];
    let after = src[(cut_to * SR as f64).round() as usize][0];
    assert!((before - after).abs() > 0.9, "the splice would jump from {before} to {after}");

    let out = fx.render(&e);
    let cut = (cut_from * SR as f64).round() as usize;
    // The steepest the 600 Hz tone ever gets, plus the slope of a 4 ms fade of a 0.5 signal.
    let natural = 0.5 * 2.0 * std::f32::consts::PI * 600.0 / SR as f32;
    let allowed = natural + 0.5 / (MICROFADE as f32 * SR as f32) + 1e-4;
    let mut worst = 0.0f32;
    for i in cut - 600..cut + 600 {
        worst = worst.max((out[i + 1][0] - out[i][0]).abs()).max((out[i + 1][1] - out[i][1]).abs());
    }
    assert!(worst <= allowed, "largest step near the cut is {worst}, allowed {allowed}");
    assert!(out[cut - 1][0].abs() < 0.01 && out[cut][0].abs() < 0.01, "both sides meet at silence: {} {}", out[cut - 1][0], out[cut][0]);
    // The fades are short: 5 ms either side the audio is untouched and belongs to the right source.
    let away = (0.005 * SR as f64) as usize;
    assert_eq!(out[cut - away], src[cut - away]);
    let after_src = (cut_to * SR as f64).round() as usize;
    assert_eq!(out[cut + away], src[after_src + away]);
    // And tight: nothing is inserted or lost.
    assert_eq!(out.len(), ((8.0 - (cut_to - cut_from)) * SR as f64).round() as usize);
}

#[test]
fn a_scene_break_without_a_cut_is_seamless() {
    let fx = Fixture::new();
    let (mut e, _, pcm) = tone_script(&fx);
    let i = word_index(&e, "w3");
    e.apply(Command::SetSceneBreak { at: i, on: true }).unwrap();
    assert_eq!(e.comp().unwrap().script_clips().len(), 2, "the scene break splits the script track");
    let out = fx.render(&e);
    let src = source(&pcm);
    assert_eq!(&out[2 * SR..4 * SR], &src[2 * SR..4 * SR], "audio runs straight through the scene break");
}

#[test]
fn layer_audio_position_and_source_offset() {
    let fx = Fixture::new();
    let wav = tone_wav(8);
    let (mut e, id, pcm) = layer_project(&fx, &wav, 8.0);
    add_layer(&mut e, 1.5, 3.0, MediaClip { src_in: 2.0, ..MediaClip::new(id) }, |_| {});
    let tl = fx.timeline(&e);
    assert_eq!(tl.duration, 4.5);
    let out = render_all(&tl);
    let src = source(&pcm);
    assert!(span(&out, 0.0, 1.5).iter().all(|s| *s == [0.0, 0.0]), "silent before the layer starts");
    // Sample exact: composition 1.5 s is source 2.0 s.
    let (c0, s0) = ((1.5 * SR as f64) as usize, 2 * SR);
    assert_eq!(&out[c0..c0 + 3 * SR], &src[s0..s0 + 3 * SR]);
    assert_eq!(dominant_freq(span(&out, 1.6, 2.4), 0), step_freq(2));
    assert_eq!(dominant_freq(span(&out, 2.6, 3.4), 0), step_freq(3));

    // On top of a script, layers add to it.
    let fx2 = Fixture::new();
    let (mut e2, id2, _) = tone_script(&fx2);
    let script_only = fx2.render(&e2);
    add_layer(&mut e2, 1.5, 3.0, MediaClip { src_in: 2.0, ..MediaClip::new(id2) }, |_| {});
    let both = fx2.render(&e2);
    let k = (2.75 * SR as f64) as usize;
    for i in k..k + 1000 {
        assert_close(both[i][0], script_only[i][0] + out[i][0], 1e-6, "sum of script and layer");
    }
}

#[test]
fn layer_gain_mute_pan_and_hidden() {
    let fx = Fixture::new();
    let wav = tone_wav(8);
    let (mut e, id, pcm) = layer_project(&fx, &wav, 8.0);
    let src = source(&pcm);
    let audio = |gain_db: f64, pan: f64, muted: bool| AudioSettings { gain_db, pan, muted, effects: Vec::new() };

    let quiet = add_layer(&mut e, 0.0, 4.0, MediaClip { audio: audio(-12.0, 0.0, false), ..MediaClip::new(id) }, |_| {});
    let out = fx.render(&e);
    let g = 10f32.powf(-12.0 / 20.0);
    for i in (1000..4 * SR).step_by(997) {
        assert_close(out[i][0], src[i][0] * g, 1e-6, "layer gain");
    }
    e.apply(Command::RemoveLayer { id: quiet }).unwrap();

    let right = add_layer(&mut e, 0.0, 4.0, MediaClip { audio: audio(0.0, 1.0, false), ..MediaClip::new(id) }, |_| {});
    let out = fx.render(&e);
    assert!(out.iter().all(|s| s[0] == 0.0) && out[SR..2 * SR] == src[SR..2 * SR].iter().map(|s| [0.0, s[1]]).collect::<Vec<_>>());
    e.apply(Command::RemoveLayer { id: right }).unwrap();

    add_layer(&mut e, 0.0, 4.0, MediaClip { audio: audio(0.0, 0.0, true), ..MediaClip::new(id) }, |_| {});
    add_layer(&mut e, 0.0, 4.0, MediaClip::new(id), |l| l.hidden = true);
    let tl = fx.timeline(&e);
    assert_eq!(tl.duration, 4.0);
    assert!(tl.segments.is_empty(), "muted and hidden layers are not mixed at all");
    assert!(render_all(&tl).iter().all(|s| *s == [0.0, 0.0]));
}

#[test]
fn layer_fades() {
    let fx = Fixture::new();
    let wav = tone_wav(8);
    let (mut e, id, pcm) = layer_project(&fx, &wav, 8.0);
    add_layer(&mut e, 1.0, 3.0, MediaClip::new(id), |l| {
        l.fade_in = 1.0;
        l.fade_out = 0.5;
    });
    let out = fx.render(&e);
    let src = source(&pcm);
    let start = SR;
    for k in (0..3 * SR).step_by(101) {
        let t = k as f64 / SR as f64;
        let env = if t < 1.0 {
            t
        } else if t >= 2.5 {
            (3.0 - t) / 0.5
        } else {
            1.0
        };
        assert_close(out[start + k][0], src[k][0] * env as f32, 2e-6, &format!("envelope at {t}"));
    }
    assert_eq!(out[start], [0.0, 0.0], "a fade in starts from silence");
    assert!(out[start + 3 * SR - 1][0].abs() < 1e-4, "a fade out ends in silence");
    // Loudness really rises: the first tenth is much quieter than the middle.
    assert!(rms(span(&out, 1.0, 1.1), 0) < 0.1 * rms(span(&out, 2.2, 2.4), 0));

    // Fades longer than the layer are limited to half of it each, as in export.
    let fx = Fixture::new();
    let (mut e, id, pcm) = layer_project(&fx, &wav, 8.0);
    add_layer(&mut e, 0.0, 2.0, MediaClip::new(id), |l| {
        l.fade_in = 5.0;
        l.fade_out = 5.0;
    });
    let out = fx.render(&e);
    let src = source(&pcm);
    assert_close(out[SR / 2][0], src[SR / 2][0] * 0.5, 2e-6, "half way up");
    assert_close(out[SR - 1][0], src[SR - 1][0] * ((SR - 1) as f32 / SR as f32), 2e-6, "top of the fade in");
    assert_close(out[SR + SR / 2][0], src[SR + SR / 2][0] * 0.5, 2e-6, "half way down");
}

#[test]
fn layer_volume_keys_interpolate_in_db() {
    let fx = Fixture::new();
    let wav = tone_wav(8);
    let (mut e, id, pcm) = layer_project(&fx, &wav, 8.0);
    // Deliberately out of order: the model does not promise sorted keys.
    let keys = vec![(2.0, -20.0), (1.0, 0.0), (3.0, -20.0), (3.5, -6.0)];
    add_layer(&mut e, 0.5, 5.0, MediaClip { volume_keys: keys, ..MediaClip::new(id) }, |_| {});
    let out = fx.render(&e);
    let src = source(&pcm);
    let db_at = |t: f64| -> f64 {
        if t < 1.0 {
            0.0
        } else if t < 2.0 {
            -20.0 * (t - 1.0)
        } else if t < 3.0 {
            -20.0
        } else if t < 3.5 {
            -20.0 + 14.0 * (t - 3.0) / 0.5
        } else {
            -6.0
        }
    };
    let start = SR / 2;
    for k in (0..5 * SR).step_by(89) {
        let t = k as f64 / SR as f64;
        let g = 10f64.powf(db_at(t) / 20.0) as f32;
        assert_close(out[start + k][0], src[k][0] * g, 3e-6, &format!("volume at layer time {t}"));
    }
    // Spot checks in plain numbers: -10 dB half way down the ramp, a tenth at -20 dB.
    let k = (1.5 * SR as f64) as usize + 7;
    assert_close(out[start + k][0] / src[k][0], 0.3162, 1e-3, "-10 dB");
    let k = (2.5 * SR as f64) as usize + 7;
    assert_close(out[start + k][0] / src[k][0], 0.1, 1e-3, "-20 dB");
}

#[test]
fn layer_loop() {
    let fx = Fixture::new();
    let blip = sine_wav("blip1s.wav", 450, 1.0, 48_000, 2);
    let (mut e, id, pcm) = layer_project(&fx, &blip, 1.0);
    let src = source(&pcm);
    assert_eq!(src.len(), SR);

    // Without looping the layer falls silent when the file ends.
    let once = add_layer(&mut e, 0.5, 3.5, MediaClip::new(id), |_| {});
    let out = fx.render(&e);
    assert_eq!(out.len(), 4 * SR);
    assert_eq!(&out[SR / 2..SR / 2 + SR], &src[..]);
    assert!(out[SR / 2 + SR..].iter().all(|s| *s == [0.0, 0.0]));
    e.apply(Command::RemoveLayer { id: once }).unwrap();

    // Looping repeats it for the whole layer.
    add_layer(&mut e, 0.5, 3.5, MediaClip { loop_playback: true, ..MediaClip::new(id) }, |_| {});
    let out = fx.render(&e);
    for k in 0..(3.5 * SR as f64) as usize {
        assert_eq!(out[SR / 2 + k], src[k % SR], "loop frame {k}");
    }
    assert!(rms(span(&out, 3.0, 4.0), 0) > 0.05);

    // A loop that starts inside the file wraps back to that point, as export does.
    let fx = Fixture::new();
    let (mut e, id, pcm) = layer_project(&fx, &blip, 1.0);
    let src = source(&pcm);
    add_layer(&mut e, 0.0, 2.0, MediaClip { loop_playback: true, src_in: 0.75, ..MediaClip::new(id) }, |_| {});
    let out = fx.render(&e);
    let (from, len) = (SR * 3 / 4, SR / 4);
    for k in 0..2 * SR {
        assert_eq!(out[k], src[from + k % len], "offset loop frame {k}");
    }
}

#[test]
fn layer_speed_doubles_pitch_and_halves_length() {
    let fx = Fixture::new();
    let wav = tone_wav(8);
    let (mut e, id, _) = layer_project(&fx, &wav, 8.0);
    add_layer(&mut e, 0.0, 5.0, MediaClip { speed: 2.0, ..MediaClip::new(id) }, |_| {});
    let out = fx.render(&e);
    assert_eq!(out.len(), 5 * SR);
    // Each source second lasts half a second and sounds an octave up.
    for sec in 0..8 {
        let from = sec as f64 * 0.5;
        assert_eq!(dominant_freq(span(&out, from + 0.05, from + 0.45), 0), 2.0 * step_freq(sec), "source second {sec}");
    }
    // The 8 s source is used up after 4 s.
    assert!(peak(span(&out, 3.9, 3.99), 0) > 0.4);
    assert!(span(&out, 4.0, 5.0).iter().all(|s| *s == [0.0, 0.0]));

    // Half speed: an octave down and twice as long.
    let fx = Fixture::new();
    let (mut e, id, pcm) = layer_project(&fx, &wav, 8.0);
    add_layer(&mut e, 0.0, 6.0, MediaClip { speed: 0.5, src_in: 1.0, ..MediaClip::new(id) }, |_| {});
    let out = fx.render(&e);
    assert_eq!(dominant_freq(span(&out, 0.2, 1.8), 0), step_freq(1) / 2.0);
    assert_eq!(dominant_freq(span(&out, 2.2, 3.8), 0), step_freq(2) / 2.0);
    // Every second output sample is a source sample; the ones between are the mean of their neighbours.
    let src = source(&pcm);
    for k in (0..40_000).step_by(2) {
        assert_eq!(out[k], src[SR + k / 2]);
        assert_close(out[k + 1][0], (src[SR + k / 2][0] + src[SR + k / 2 + 1][0]) / 2.0, 1e-6, "interpolated sample");
    }
}

/// A timeline that uses every feature at once.
fn busy_timeline(fx: &Fixture) -> Timeline {
    let (mut e, id, _) = tone_script(fx);
    let i = word_index(&e, "w2");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    let i = word_index(&e, "w5");
    e.apply(Command::ReplaceWithGap { start: i, end: i + 1 }).unwrap();
    e.apply(Command::SetScriptAudio { audio: AudioSettings { gain_db: -3.0, pan: 0.25, ..Default::default() } }).unwrap();
    let blip = sine_wav("blip1s.wav", 450, 1.0, 48_000, 2);
    fx.load(&blip);
    let blip_asset = asset_with_words(&blip, MediaKind::Audio, 1.0, Vec::new());
    let blip_id = blip_asset.id;
    e.apply(Command::AddAsset { asset: blip_asset }).unwrap();
    add_layer(&mut e, 0.3333, 6.1, MediaClip { loop_playback: true, speed: 1.37, audio: AudioSettings { gain_db: -9.0, pan: -0.6, ..Default::default() }, ..MediaClip::new(blip_id) }, |l| {
        l.fade_in = 0.7;
        l.fade_out = 1.1;
    });
    add_layer(&mut e, 2.2, 4.0, MediaClip { src_in: 0.5, speed: 0.8, volume_keys: vec![(0.0, -30.0), (1.0, 0.0), (2.5, -12.0)], ..MediaClip::new(id) }, |_| {});
    add_layer(&mut e, 6.5, 2.75, MediaClip { src_in: 6.0, ..MediaClip::new(id) }, |l| l.fade_out = 0.25);
    let tl = fx.timeline(&e);
    assert_eq!(tl.segments.len(), 3 + 3, "three script clips and three layers");
    tl
}

#[test]
fn block_size_does_not_change_the_result() {
    let fx = Fixture::new();
    let tl = busy_timeline(&fx);
    let whole = render_all(&tl);
    assert!(rms(&whole, 0) > 0.05);

    let bits = |v: &[[f32; 2]]| -> Vec<[u32; 2]> { v.iter().map(|s| [s[0].to_bits(), s[1].to_bits()]).collect() };
    let whole_bits = bits(&whole);
    // Pseudo-random block sizes from 1 to 4096 frames, plus fixed odd sizes.
    for (seed, fixed) in [(1u64, None), (7, None), (0, Some(1usize)), (0, Some(63)), (0, Some(480)), (0, Some(4099))] {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut out = vec![[7.0f32; 2]; whole.len()];
        let mut pos = 0;
        // Size 1 over the whole thing is slow; a stretch across the busiest part is enough.
        let (from, to) = if fixed == Some(1) { (2 * SR, 3 * SR) } else { (0, whole.len()) };
        pos += from;
        while pos < to {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let n = fixed.unwrap_or(1 + (state % 4096) as usize).min(to - pos);
            mix::render(&tl, pos as u64, &mut out[pos..pos + n]);
            pos += n;
        }
        assert!(bits(&out[from..to]) == whole_bits[from..to], "block pattern seed {seed} / fixed {fixed:?} differs from a single block");
    }

    // Rendering beyond the end, or a range that starts past it, is silence.
    let mut tail = vec![[1.0f32; 2]; 1000];
    mix::render(&tl, tl.frames() - 400, &mut tail);
    assert_eq!(&tail[..400], &whole[whole.len() - 400..]);
    assert!(tail[400..].iter().all(|s| *s == [0.0, 0.0]));
    mix::render(&tl, tl.frames() + 10_000_000, &mut tail);
    assert!(tail.iter().all(|s| *s == [0.0, 0.0]));
    mix::render(&tl, u64::MAX - 5000, &mut tail);
    assert!(tail.iter().all(|s| *s == [0.0, 0.0]));
    mix::render(&tl, 0, &mut []);
}

#[test]
fn sources_that_are_not_decoded_yet_are_skipped() {
    let fx = Fixture::new();
    let wav = tone_wav(8);
    let (mut e, id) = editor_with_script(asset_per_second(&wav, MediaKind::Audio, 8));
    add_layer(&mut e, 9.0, 2.0, MediaClip::new(id), |_| {});
    // Nothing decoded: silent, but the right length.
    let tl = fx.timeline(&e);
    assert!(tl.segments.is_empty());
    assert_eq!(tl.duration, 11.0);
    assert!(render_all(&tl).iter().all(|s| *s == [0.0, 0.0]));
    // Once the decode has finished, the same call picks it up.
    fx.load(&wav);
    let tl = fx.timeline(&e);
    assert_eq!(tl.segments.len(), 2);
    assert_eq!(dominant_freq(span(&render_all(&tl), 9.1, 9.9), 0), step_freq(0));
}

#[test]
fn odd_values_do_not_panic() {
    let fx = Fixture::new();
    let wav = tone_wav(8);
    let pcm = fx.load(&wav);
    let seg = |f: &dyn Fn(&mut AudioSegment)| {
        let mut s = AudioSegment { pcm: pcm.clone(), comp_start: 1.0, duration: 2.0, src_start: 0.0, speed: 1.0, gain: 1.0, pan: 0.0, fade_in: 0.0, fade_out: 0.0, volume_keys: Vec::new(), looped: false };
        f(&mut s);
        s
    };
    let segments = vec![
        seg(&|s| s.speed = f64::NAN),
        seg(&|s| s.speed = 0.0),
        seg(&|s| s.speed = -3.0),
        seg(&|s| s.speed = 1e12),
        seg(&|s| s.src_start = -5.0),
        seg(&|s| s.src_start = 1e9),
        seg(&|s| s.src_start = f64::INFINITY),
        seg(&|s| s.duration = -1.0),
        seg(&|s| s.duration = f64::NAN),
        seg(&|s| s.comp_start = f64::NAN),
        seg(&|s| s.comp_start = -4.0),
        seg(&|s| (s.fade_in, s.fade_out) = (f64::NAN, 1e9)),
        seg(&|s| (s.fade_in, s.fade_out) = (-1.0, -1.0)),
        seg(&|s| s.gain = f32::NAN),
        seg(&|s| s.pan = f32::INFINITY),
        seg(&|s| s.volume_keys = vec![(f64::NAN, 0.0), (1.0, f64::NEG_INFINITY), (1.0, 3.0)]),
        seg(&|s| {
            s.looped = true;
            s.src_start = 8.0;
        }),
        seg(&|s| {
            s.looped = true;
            s.speed = 63.0;
            s.src_start = 7.99999;
        }),
    ];
    let tl = Timeline { segments, duration: 4.0 };
    let out = render_all(&tl);
    assert_eq!(out.len(), 4 * SR);
    assert!(out.iter().all(|s| s[0].is_finite() && s[1].is_finite()));
    mix::prefetch(&tl, 0, u64::MAX);

    // The same through the model: the editor may be handed nonsense by a damaged project file.
    let (mut e, id, _) = layer_project(&fx, &wav, 8.0);
    add_layer(&mut e, -2.0, 3.0, MediaClip { speed: 0.0, src_in: -1.0, ..MediaClip::new(id) }, |l| {
        l.fade_in = -1.0;
        l.fade_out = f64::NAN;
    });
    add_layer(&mut e, 1.0, 1.0, MediaClip { speed: f64::NAN, src_in: f64::NAN, volume_keys: vec![(f64::NAN, f64::NAN), (0.5, f64::NAN)], ..MediaClip::new(id) }, |_| {});
    // Layers the editor would refuse, as a hand-edited project file could contain them.
    let comp = e.comp_mut().unwrap();
    comp.layers.push(Layer::new("ghost", 0.0, 1.0, LayerContent::Audio(MediaClip::new(AssetId::new()))));
    comp.layers.push(Layer::new("nan", f64::NAN, f64::NAN, LayerContent::Audio(MediaClip::new(id))));
    comp.layers.push(Layer::new("backwards", 3.0, -1.0, LayerContent::Video(MediaClip::new(id))));
    let tl = fx.timeline(&e);
    let out = render_all(&tl);
    assert!(out.iter().all(|s| s[0].is_finite() && s[1].is_finite()));
    assert!(Timeline::default().frames() == 0);
    assert!(Timeline { segments: Vec::new(), duration: f64::NAN }.frames() == 0);
}
