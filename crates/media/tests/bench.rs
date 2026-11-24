//! A timing run for a realistic export. Not a correctness test: run with
//! `cargo test -p mango-media --test bench -- --ignored --nocapture`.

mod common;
use common::*;
use mango_core::*;
use mango_media::export::{Stage, export};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

#[test]
#[ignore = "benchmark: prints timings for a 60 s 1080p export, asserts only basic correctness"]
fn bench_60s_1080p_export() {
    let src = lavfi(
        "bench_1080p_60s.mp4",
        &[
            "-f", "lavfi", "-i", "testsrc2=s=1920x1080:r=30:d=60", "-f", "lavfi", "-i", "sine=f=440:d=60:r=48000", "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k", "-shortest",
            "-f", "mp4",
        ],
    );
    let music = tone_wav("tone_2000_60s.wav", 2000, 60.0, 0.5);
    let (mut e, _) = editor_with_source(&src, Some(unit_transcript(120)));
    // 20 cuts spread over the minute.
    for k in (3..120).step_by(6).rev() {
        let i = word_index(&e, &format!("w{k}"));
        e.apply(Command::DeleteRange { start: i, end: i + 1 }).unwrap();
    }
    let total = e.comp().unwrap().duration();
    let mut title = Layer::new("Title", 1.0, 6.0, LayerContent::Text(TextStyle { text: "Benchmark title".into(), ..TextStyle::default() }));
    title.transform = Transform { x: 0.5, y: 0.2, w: 0.8, h: 0.2, rotation_deg: 0.0, opacity: 1.0 };
    title.animation_in = Animation::Fade;
    e.apply(Command::AddLayer { layer: title }).unwrap();
    let mut bar = Layer::new("Bar", 0.0, total, LayerContent::Progress { color: Rgba::rgb(255, 138, 0), track_color: Rgba::rgba(255, 255, 255, 80) });
    bar.transform = Transform { x: 0.5, y: 0.97, w: 0.96, h: 0.012, rotation_deg: 0.0, opacity: 1.0 };
    e.apply(Command::AddLayer { layer: bar }).unwrap();
    let id = add_asset(&mut e, &music);
    let mut clip = MediaClip::new(id);
    clip.duck = Some(Ducking::default());
    clip.audio.gain_db = -12.0;
    e.apply(Command::AddLayer { layer: Layer::new("Music", 0.0, total, LayerContent::Audio(clip)) }).unwrap();
    e.apply(Command::SetCaptions { captions: CaptionSettings { enabled: true, highlight: CaptionHighlight::ActiveWord, ..CaptionSettings::default() } }).unwrap();

    let dir = tempfile::tempdir().unwrap();
    for (label, hardware) in [("libx264 (software)", false), ("h264_videotoolbox (hardware)", true)] {
        let out = dir.path().join(format!("bench_{hardware}.mp4"));
        let mut s = settings(&e);
        s.hardware = hardware;
        assert_eq!((s.width, s.height), (1920, 1080));
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let mut marks: Vec<(Stage, f64)> = Vec::new();
        let result = export(&e.project, e.comp().unwrap(), &s, &out, &mut |p| {
            if marks.last().map(|m| std::mem::discriminant(&m.0) != std::mem::discriminant(&p.stage)).unwrap_or(true) {
                marks.push((p.stage, started.elapsed().as_secs_f64()));
            }
        }, &cancel);
        let took = started.elapsed().as_secs_f64();
        match result {
            Ok(report) => {
                let inf = info(&out);
                assert_eq!(inf.frames, (total * 30.0).round() as u64);
                assert!((inf.duration - total).abs() < 0.05);
                let stage_at = |want: fn(&Stage) -> bool| marks.iter().find(|m| want(&m.0)).map(|m| m.1).unwrap_or(0.0);
                let chunks_at = stage_at(|s| matches!(s, Stage::Chunk { .. }));
                let final_at = stage_at(|s| matches!(s, Stage::Final));
                println!(
                    "{label}: {total:.1} s of 1080p30 in {took:.1} s ({:.2}x realtime): prepare {chunks_at:.1} s, chunks {:.1} s, final {:.1} s, {:.1} MB",
                    total / took,
                    final_at - chunks_at,
                    took - final_at,
                    report.bytes as f64 / 1e6
                );
            }
            Err(err) if hardware => println!("{label}: not available here ({err:#})"),
            Err(err) => panic!("{err:#}"),
        }
    }
}
