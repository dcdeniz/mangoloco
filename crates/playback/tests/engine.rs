//! Transport semantics on the device-less engine, the engine's output against
//! the reference mixer, and the real output device.

mod common;
use common::*;
use mango_core::*;
use mango_playback::audio::{AudioEngine, Engine, NullEngine, XFADE_FRAMES};
use mango_playback::mix::{self, AudioSegment, Timeline};
use mango_playback::pcm::{Pcm, PcmCache};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

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
        self.cache.ensure(path, &mut |_| {}, &AtomicBool::new(false)).unwrap().unwrap()
    }
    /// The 8 s stepped tone as an unedited script.
    fn tone(&self) -> Timeline {
        let wav = tone_wav(8);
        self.load(&wav);
        let (e, _) = editor_with_script(asset_per_second(&wav, MediaKind::Audio, 8));
        mix::timeline(&e.project, e.comp().unwrap(), &self.cache)
    }
    /// The same script with word `w<n>` deleted.
    fn tone_without(&self, n: usize) -> Timeline {
        let wav = tone_wav(8);
        self.load(&wav);
        let (mut e, _) = editor_with_script(asset_per_second(&wav, MediaKind::Audio, 8));
        let i = word_index(&e, &format!("w{n}"));
        e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
        mix::timeline(&e.project, e.comp().unwrap(), &self.cache)
    }
    /// A steady 450 Hz tone of `secs` seconds as one segment.
    fn steady(&self, secs: f64) -> Timeline {
        let pcm = self.load(&sine_wav("steady10.wav", 450, 10.0, 48_000, 2));
        Timeline {
            segments: vec![AudioSegment { pcm, comp_start: 0.0, duration: secs, src_start: 0.0, speed: 1.0, gain: 4.0, pan: 0.0, fade_in: 0.0, fade_out: 0.0, volume_keys: Vec::new(), looped: false }],
            duration: secs,
        }
    }
}

fn reference(tl: &Timeline, start_frame: usize, frames: usize) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; frames];
    mix::render(tl, start_frame as u64, &mut out);
    out
}

fn pull(engine: &NullEngine, frames: usize) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; frames];
    engine.render(&mut out);
    out
}

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

fn max_step(buf: &[[f32; 2]]) -> f32 {
    buf.windows(2).map(|w| (w[1][0] - w[0][0]).abs().max((w[1][1] - w[0][1]).abs())).fold(0.0, f32::max)
}

#[test]
fn transport_semantics() {
    let fx = Fixture::new();
    let e = NullEngine::new();

    // Nothing loaded.
    assert_eq!((e.position(), e.is_playing(), e.finished(), e.duration()), (0.0, false, false, 0.0));
    e.advance(1.0);
    assert_eq!(e.position(), 0.0);

    e.set_timeline(fx.tone());
    assert_eq!(e.duration(), 8.0);

    // Paused: time does not move.
    e.advance(1.0);
    assert_eq!((e.position(), e.is_playing()), (0.0, false));

    // Playing: the position follows the clock exactly.
    e.play();
    assert!(e.is_playing());
    assert_eq!(e.position(), 0.0, "nothing has been sent yet");
    e.advance(1.0);
    approx(e.position(), 1.0);
    for _ in 0..60 {
        e.advance(1.0 / 60.0);
    }
    approx(e.position(), 2.0);

    // Pause holds the position.
    e.pause();
    assert!(!e.is_playing());
    e.advance(0.5);
    approx(e.position(), 2.0);
    assert!(!e.finished());

    // Seek is visible at once, paused or playing, and playback continues from there.
    e.seek(5.25);
    approx(e.position(), 5.25);
    e.advance(0.25);
    approx(e.position(), 5.25);
    e.play();
    e.advance(0.5);
    approx(e.position(), 5.75);
    e.seek(1.0);
    approx(e.position(), 1.0);
    assert!(e.is_playing());
    e.advance(0.5);
    approx(e.position(), 1.5);

    // Seeks are clamped to the timeline.
    e.pause();
    e.seek(-3.0);
    approx(e.position(), 0.0);
    e.seek(f64::NAN);
    approx(e.position(), 0.0);
    e.seek(1e12);
    approx(e.position(), 8.0);
    assert!(!e.finished(), "seeking to the end is not the same as playing into it");

    // Running into the end stops and reports finished.
    e.seek(7.5);
    e.play();
    e.advance(0.25);
    assert!(e.is_playing() && !e.finished());
    e.advance(0.5);
    assert!(!e.is_playing());
    assert!(e.finished());
    approx(e.position(), 8.0);
    e.advance(2.0);
    approx(e.position(), 8.0);

    // Play at the end starts over.
    e.play();
    assert!(e.is_playing() && !e.finished());
    approx(e.position(), 0.0);
    e.advance(0.5);
    approx(e.position(), 0.5);

    // Seek clears finished too.
    e.seek(7.9);
    e.advance(1.0);
    assert!(e.finished());
    e.seek(3.0);
    assert!(!e.finished() && !e.is_playing());
    approx(e.position(), 3.0);
}

#[test]
fn rate_changes_speed_and_pitch() {
    let fx = Fixture::new();
    let e = NullEngine::new();
    e.set_timeline(fx.tone());
    assert_eq!(e.rate(), 1.0);

    e.set_rate(2.0);
    e.play();
    e.advance(1.0);
    approx(e.position(), 2.0);
    // Source second 2 (500 Hz) goes by in half a second, an octave up.
    let out = pull(&e, SR / 2);
    approx(e.position(), 3.0);
    assert_eq!(dominant_freq(&out[2400..SR / 2 - 2400], 0), 1000.0);

    e.set_rate(0.5);
    e.seek(4.0);
    let out = pull(&e, SR);
    approx(e.position(), 4.5);
    assert_eq!(dominant_freq(&out[4800..SR - 4800], 0), step_freq(4) / 2.0);
    assert!(max_step(&out) < 0.05, "half speed output is smooth");

    // Limits.
    e.set_rate(100.0);
    assert_eq!(e.rate(), 3.0);
    e.set_rate(0.01);
    assert_eq!(e.rate(), 0.5);
    e.set_rate(f32::NAN);
    assert_eq!(e.rate(), 1.0);

    // At a fractional rate the end is still reached exactly.
    e.set_rate(1.5);
    e.seek(7.0);
    e.advance(0.5);
    approx(e.position(), 7.75);
    e.advance(0.5);
    assert!(e.finished());
    approx(e.position(), 8.0);
}

#[test]
fn engine_output_equals_the_reference_mixer() {
    let fx = Fixture::new();
    let tl = fx.tone_without(2);
    let e = NullEngine::new();
    e.set_timeline(tl.clone());
    e.seek(1.0);
    e.play();

    // Pull three seconds (across the cut at 2 s) in uneven device-like blocks.
    let mut out = Vec::new();
    for n in [512usize, 480, 1, 4096, 333, 2048].iter().cycle() {
        if out.len() >= 3 * SR {
            break;
        }
        out.extend(pull(&e, *n));
    }
    let want = reference(&tl, SR, out.len());
    approx(e.position(), 1.0 + out.len() as f64 / SR as f64);

    // After the 5 ms fade-in at the start of playback the engine is the mixer, bit for bit.
    assert!(out[XFADE_FRAMES..] == want[XFADE_FRAMES..], "engine output differs from mix::render");
    // The fade-in itself is the same audio under a linear ramp from silence.
    for i in 0..XFADE_FRAMES {
        let w = i as f32 / XFADE_FRAMES as f32;
        assert!((out[i][0] - want[i][0] * w).abs() < 1e-6 && (out[i][1] - want[i][1] * w).abs() < 1e-6, "fade-in frame {i}");
    }
    assert_eq!(out[0], [0.0, 0.0]);
    // It is what the edit says: second 2 of the output is source second 3.
    assert_eq!(dominant_freq(&out[SR + 4800..2 * SR - 4800], 0), step_freq(3));
}

#[test]
fn set_timeline_while_playing_keeps_the_position_and_does_not_click() {
    let fx = Fixture::new();
    let before = fx.tone();
    let after = fx.tone_without(1);
    let e = NullEngine::new();
    e.set_timeline(before.clone());
    e.play();
    e.advance(1.25);
    approx(e.position(), 1.25);

    // The edit lands while playing.
    e.set_timeline(after.clone());
    approx(e.position(), 1.25);
    assert!(e.is_playing());
    assert_eq!(e.duration(), 7.0);
    let start = (1.25 * SR as f64) as usize;
    let out = pull(&e, SR / 2);
    approx(e.position(), 1.75);

    // The first 5 ms cross from the old content to the new, then it is exactly the new timeline.
    let old = reference(&before, start, out.len());
    let new = reference(&after, start, out.len());
    for i in 0..XFADE_FRAMES {
        let w = i as f32 / XFADE_FRAMES as f32;
        let want = new[i][0] * w + old[i][0] * (1.0 - w);
        assert!((out[i][0] - want).abs() < 1e-6, "crossfade frame {i}: {} vs {want}", out[i][0]);
    }
    assert!(out[XFADE_FRAMES..] == new[XFADE_FRAMES..]);
    assert_eq!(out[0], old[0], "the swap starts from what was playing");
    assert_eq!(dominant_freq(&out[2400..], 0), step_freq(2), "now playing the edited script");
    // No click: no step larger than the tones themselves can make (500 Hz at 0.5, plus the fade).
    let natural = 0.5 * 2.0 * std::f32::consts::PI * 500.0 / SR as f32;
    assert!(max_step(&out[..2 * XFADE_FRAMES]) <= natural + 1.0 / XFADE_FRAMES as f32, "step {}", max_step(&out[..2 * XFADE_FRAMES]));

    // Swapping while paused changes nothing audible and keeps the position.
    e.pause();
    pull(&e, 1024);
    e.set_timeline(before);
    approx(e.position(), 1.75);
    assert!(pull(&e, 1024).iter().all(|s| *s == [0.0, 0.0]));

    // A timeline that ends before the playhead: playback finishes at its end.
    e.seek(6.5);
    e.play();
    e.advance(0.1);
    let short = fx.steady(2.0);
    e.set_timeline(short);
    approx(e.position(), 2.0);
    e.advance(0.1);
    assert!(e.finished() && !e.is_playing());
    approx(e.position(), 2.0);
}

#[test]
fn pause_seek_and_play_are_faded() {
    let fx = Fixture::new();
    let tl = fx.steady(10.0);
    let e = NullEngine::new();
    e.set_timeline(tl.clone());
    let amp = peak(&reference(&tl, 0, SR), 0);
    let natural = amp * 2.0 * std::f32::consts::PI * 450.0 / SR as f32;
    let allowed = natural + 1.5 * amp / XFADE_FRAMES as f32;

    e.play();
    let a = pull(&e, 1000);
    e.pause();
    let b = pull(&e, 1000);
    e.play();
    let c = pull(&e, 1000);
    e.seek(4.0001);
    let d = pull(&e, 1000);
    // Seeks land on the nearest frame.
    approx(e.position(), ((4.0001f64 * SR as f64).round() + 1000.0) / SR as f64);

    // Pause: a short tail that fades out, then silence; the position does not move.
    assert!(b[0] != [0.0, 0.0] && b[10][0].abs() > 0.0, "the tail continues the sound");
    assert!(b[XFADE_FRAMES..].iter().all(|s| *s == [0.0, 0.0]));
    // Play: fades in from silence at the paused position.
    assert_eq!(c[0], [0.0, 0.0]);
    assert!(c[XFADE_FRAMES..] == reference(&tl, 1000, 1000)[XFADE_FRAMES..]);
    // Seek while playing: crosses over to the new place.
    assert!(d[XFADE_FRAMES..] == reference(&tl, (4.0001f64 * SR as f64).round() as usize, 1000)[XFADE_FRAMES..]);

    let all: Vec<[f32; 2]> = [a, b, c, d].concat();
    assert!(max_step(&all) <= allowed, "largest step {} exceeds {allowed}", max_step(&all));
}

#[test]
fn volume_and_levels() {
    let fx = Fixture::new();
    let tl = fx.steady(10.0);
    let amp = peak(&reference(&tl, 0, SR), 0);
    assert!(amp > 0.1);
    let e = NullEngine::new();
    e.set_timeline(tl.clone());
    assert_eq!(e.levels(), (0.0, 0.0));

    e.play();
    let full = pull(&e, 4800);
    assert!((peak(&full[1000..], 0) - amp).abs() < 1e-4);
    let (l, r) = e.levels();
    assert!((l - amp).abs() < 1e-3 && (r - amp).abs() < 1e-3, "meter shows the peak: {l} {r}");

    // Volume scales the output (smoothly), not the meter.
    e.set_volume(0.5);
    let ramp = pull(&e, 4800);
    // An instant change could jump by up to half the amplitude; ramped, the waveform's own slope is the limit.
    let natural = amp * 2.0 * std::f32::consts::PI * 450.0 / SR as f32;
    assert!(max_step(&ramp) <= natural + 1e-3, "volume changes are ramped: step {} vs {natural}", max_step(&ramp));
    let half = pull(&e, 4800);
    assert!((peak(&half, 0) - amp * 0.5).abs() < 1e-4);
    assert!((e.levels().0 - amp).abs() < 1e-3);

    e.set_volume(0.0);
    pull(&e, 4800);
    assert!(pull(&e, 4800).iter().all(|s| *s == [0.0, 0.0]));
    assert!(e.levels().0 > 0.1, "the meter still moves with the volume at zero");

    // Loud material is limited to full scale at the output.
    e.set_volume(2.0);
    let mut loud = tl.clone();
    loud.segments[0].gain = 20.0;
    e.set_timeline(loud);
    pull(&e, 4800);
    let clipped = pull(&e, 4800);
    assert!(peak(&clipped, 0) == 1.0 && clipped.iter().all(|s| s[0].abs() <= 1.0 && s[1].abs() <= 1.0));

    // Out-of-range volumes are clamped rather than trusted.
    e.set_volume(f32::NAN);
    pull(&e, 4800);
    assert!(pull(&e, 4800).iter().all(|s| *s == [0.0, 0.0]));

    // After pausing, the meter falls back to zero within a couple of seconds.
    e.pause();
    e.advance(0.3);
    let falling = e.levels().0;
    assert!(falling > 0.0 && falling < 0.6 * 20.0 * amp, "decaying: {falling}");
    e.advance(3.0);
    assert_eq!(e.levels(), (0.0, 0.0));

    // A hard-left source only moves the left meter.
    let mut left = tl.clone();
    left.segments[0].pan = -1.0;
    e.set_volume(1.0);
    e.set_timeline(left);
    e.play();
    pull(&e, 4800);
    let (l, r) = e.levels();
    assert!(l > 0.1 && r == 0.0, "{l} {r}");
}

#[test]
fn engines_share_one_interface() {
    let fx = Fixture::new();
    let engines: Vec<Box<dyn Engine>> = match AudioEngine::new() {
        Ok(a) => vec![Box::new(NullEngine::new()), Box::new(a)],
        Err(_) => vec![Box::new(NullEngine::new())],
    };
    for e in &engines {
        e.set_volume(0.0);
        e.set_timeline(fx.tone());
        e.seek(2.5);
        assert_eq!(e.position(), 2.5);
        assert_eq!(e.duration(), 8.0);
        assert!(!e.is_playing() && !e.finished());
        e.set_rate(1.5);
        assert_eq!(e.rate(), 1.5);
    }
}

#[test]
fn many_timeline_swaps_from_another_thread() {
    // The control side may be hammered while the render side runs; nothing may tear or crash.
    let fx = Fixture::new();
    let (a, b) = (fx.tone(), fx.tone_without(3));
    let e = Arc::new(NullEngine::new());
    e.set_timeline(a.clone());
    e.play();
    let stop = Arc::new(AtomicBool::new(false));
    let control = {
        let (e, stop) = (e.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut i = 0u64;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                e.set_timeline(if i.is_multiple_of(2) { b.clone() } else { a.clone() });
                if i.is_multiple_of(7) {
                    e.seek((i % 60) as f64 / 10.0);
                }
                if i.is_multiple_of(13) {
                    e.play();
                }
                let p = e.position();
                assert!((0.0..=8.0).contains(&p), "position {p}");
                i += 1;
            }
            i
        })
    };
    let mut buf = vec![[0.0f32; 2]; 256];
    let until = Instant::now() + Duration::from_millis(400);
    let mut blocks = 0;
    while Instant::now() < until {
        e.render(&mut buf);
        assert!(buf.iter().all(|s| s[0].abs() <= 1.0 && s[1].abs() <= 1.0));
        blocks += 1;
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let swaps = control.join().unwrap();
    assert!(swaps > 100 && blocks > 100, "{swaps} swaps, {blocks} blocks");
}

#[test]
fn render_cost_per_callback() {
    // Not a pass/fail benchmark of the machine; a guard against the renderer becoming
    // slower than realtime by orders of magnitude, and the number for the report.
    let fx = Fixture::new();
    let wav = tone_wav(8);
    fx.load(&wav);
    let (mut e, id) = editor_with_script(asset_per_second(&wav, MediaKind::Audio, 8));
    let i = word_index(&e, "w2");
    e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    for (start, speed) in [(0.0, 1.0), (0.5, 1.25)] {
        let clip = MediaClip { speed, loop_playback: true, volume_keys: vec![(0.0, -12.0), (5.0, -6.0)], ..MediaClip::new(id) };
        let mut layer = Layer::new("bed", start, 6.5, LayerContent::Audio(clip));
        layer.fade_in = 1.0;
        e.apply(Command::AddLayer { layer }).unwrap();
    }
    let tl = mix::timeline(&e.project, e.comp().unwrap(), &fx.cache);
    let engine = NullEngine::new();
    engine.set_timeline(tl);
    engine.set_volume(0.8);
    let mut buf = vec![[0.0f32; 2]; 512];
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    let mut blocks = 0u32;
    for rate in [1.0f32, 1.5] {
        engine.set_rate(rate);
        engine.seek(0.0);
        engine.play();
        while engine.is_playing() {
            let t = Instant::now();
            engine.render(&mut buf);
            let d = t.elapsed();
            worst = worst.max(d);
            total += d;
            blocks += 1;
        }
    }
    let block_secs = 512.0 / SR as f64;
    let mean = total.as_secs_f64() / blocks as f64;
    println!(
        "MEASURE audio callback (512 frames = {:.2} ms of audio; script with a cut + 2 looping layers with speed, keys, fade): mean {:.1} us = {:.2}% of realtime, worst {:.1} us",
        block_secs * 1e3,
        mean * 1e6,
        100.0 * mean / block_secs,
        worst.as_secs_f64() * 1e6
    );
    assert!(mean < block_secs / 4.0, "rendering a block takes {mean} s on average");
}

#[test]
fn audio_engine_on_this_machine() {
    let fx = Fixture::new();
    let engine = match AudioEngine::new() {
        Ok(e) => e,
        Err(e) => {
            // No usable output in this session: the error must say so plainly.
            let msg = format!("{e:#}");
            println!("MEASURE audio device: none ({msg})");
            assert!(msg.contains("audio output device"), "unclear error: {msg}");
            return;
        }
    };
    println!("MEASURE audio device: {} Hz, {} channels", engine.device_sample_rate(), engine.device_channels());
    // Nothing audible: the volume is zero; the meter is measured before the volume control.
    engine.set_volume(0.0);
    engine.set_timeline(fx.steady(10.0));
    assert_eq!(engine.position(), 0.0);
    assert!(!engine.is_playing());
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(engine.position(), 0.0, "paused: the clock stands still");
    assert_eq!(engine.levels(), (0.0, 0.0));

    engine.play();
    assert!(engine.is_playing());
    // Wait for the device to actually start, then time one second.
    let deadline = Instant::now() + Duration::from_secs(3);
    while engine.position() == 0.0 {
        assert!(Instant::now() < deadline, "the device never started: {:?}", engine.take_error());
        std::thread::sleep(Duration::from_millis(2));
    }
    let (t0, p0) = (Instant::now(), engine.position());
    let mut seen_level = (0.0f32, 0.0f32);
    let mut last = p0;
    while t0.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(10));
        let p = engine.position();
        assert!(p >= last, "the clock never runs backwards");
        last = p;
        let (l, r) = engine.levels();
        seen_level = (seen_level.0.max(l), seen_level.1.max(r));
    }
    let (wall, advanced) = (t0.elapsed().as_secs_f64(), engine.position() - p0);
    println!("MEASURE audio clock: {advanced:.4} s of audio in {wall:.4} s of wall time");
    let latency = engine.output_latency();
    println!("MEASURE audio output latency reported by the device: {:.1} ms", latency * 1e3);
    assert!(latency > 0.0 && latency < 2.0, "implausible output latency {latency}");
    assert!((advanced - wall).abs() < 0.06, "position advanced {advanced} s in {wall} s");
    assert!(seen_level.0 > 0.05 && seen_level.1 > 0.05, "levels while a tone plays: {seen_level:?}");

    // Pause: the clock stops within a buffer or two.
    engine.pause();
    std::thread::sleep(Duration::from_millis(100));
    let held = engine.position();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(engine.position(), held);
    assert!(!engine.is_playing());

    // Seek while paused, then let the device go idle (it is stopped after 1.5 s) and start again.
    engine.seek(7.0);
    assert_eq!(engine.position(), 7.0);
    std::thread::sleep(Duration::from_millis(2200));
    assert_eq!(engine.levels(), (0.0, 0.0), "meter is at rest when idle");
    assert_eq!(engine.position(), 7.0);
    let t = Instant::now();
    engine.play();
    let play_call = t.elapsed();
    while engine.position() == 7.0 {
        assert!(t.elapsed() < Duration::from_secs(3), "the device did not restart: {:?}", engine.take_error());
        std::thread::sleep(Duration::from_millis(1));
    }
    println!("MEASURE audio restart after idle: play() returned in {play_call:?}, first audio after {:?}", t.elapsed());
    std::thread::sleep(Duration::from_millis(300));
    assert!(engine.position() > 7.2 && engine.position() < 7.6, "{}", engine.position());

    // Run into the end at triple speed.
    engine.set_rate(3.0);
    let deadline = Instant::now() + Duration::from_secs(4);
    while !engine.finished() {
        assert!(Instant::now() < deadline, "never finished; position {}", engine.position());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!engine.is_playing());
    assert_eq!(engine.position(), 10.0);
    assert_eq!(engine.take_error(), None);
    println!("MEASURE audio xruns during the test: {}", engine.xruns());

    // Dropping while playing shuts everything down without hanging.
    engine.play();
    std::thread::sleep(Duration::from_millis(50));
    let t = Instant::now();
    drop(engine);
    assert!(t.elapsed() < Duration::from_secs(2), "drop took {:?}", t.elapsed());
}
