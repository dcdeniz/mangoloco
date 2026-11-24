mod common;

use std::process::Command;
use std::sync::atomic::AtomicBool;

use common::*;
use mango_speech::{transcribe, tts};

fn stream_info(path: &std::path::Path) -> String {
    run(Command::new(ffprobe())
        .args(["-v", "error", "-show_entries", "stream=codec_name,sample_rate,channels", "-of", "default=noprint_wrappers=1"])
        .arg(path))
}

#[test]
fn system_voices_are_listed() {
    let voices = tts::voices().unwrap();
    assert!(!voices.is_empty());
    for v in &voices {
        assert!(!v.id.is_empty() && v.id == v.id.trim() && !v.name.is_empty(), "{v:?}");
        assert!(v.language.len() >= 2 && !v.language.contains('_') && !v.language.contains(' '), "{v:?}");
    }
    let mut ids: Vec<&str> = voices.iter().map(|v| v.id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), voices.len(), "voice ids must be unique");
    assert!(voices.iter().any(|v| v.language.starts_with("en")), "expected at least one English voice");
}

#[test]
fn voice_listing_parser_handles_names_with_spaces_and_brackets() {
    let listing = "Albert              en_US    # Hello! My name is Albert.\n\
Bad News            en_US    # Hello! My name is Bad News.\n\
Eddy (English (US)) en_US    # Hello! My name is Eddy.\n\
Tingting            zh_CN    # 你好！我叫婷婷。\n\
Majed               ar_001   # مرحبًا! اسمي ماجد.\n\
\n\
garbage line without locale\n";
    let voices = tts::parse_voices(listing);
    let got: Vec<(&str, &str)> = voices.iter().map(|v| (v.id.as_str(), v.language.as_str())).collect();
    assert_eq!(got, [("Albert", "en-US"), ("Bad News", "en-US"), ("Eddy (English (US))", "en-US"), ("Tingting", "zh-CN"), ("Majed", "ar-001")]);
}

#[test]
fn synthesis_writes_a_playable_48k_wav_with_the_reported_duration() {
    let dir = tempfile::tempdir().unwrap();
    let short = dir.path().join("short.wav");
    let long = dir.path().join("nested/folder/long.wav");
    let d_short = tts::synthesize("Hello there.", Some("Samantha"), None, &short).unwrap();
    let d_long = tts::synthesize(
        "Hello there. This sentence is considerably longer than the first one, so it must take more time to say.",
        Some("Samantha"),
        None,
        &long,
    )
    .unwrap();
    for (path, reported) in [(&short, d_short), (&long, d_long)] {
        let probed = probe_duration(path);
        assert!((probed - reported).abs() < 0.01, "reported {reported} but ffprobe says {probed}");
        let info = stream_info(path);
        assert!(info.contains("codec_name=pcm_s16le") && info.contains("sample_rate=48000") && info.contains("channels=1"), "{info}");
        let pcm = decode_pcm(path);
        assert!(pcm.iter().any(|s| s.abs() > 0.05), "the file should contain audible speech");
    }
    assert!(d_short > 0.3 && d_long > d_short * 2.0, "short {d_short}, long {d_long}");
    // No temporary files left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).filter(|n| n.to_string_lossy().contains(".tmp")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn default_voice_works() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("default.wav");
    let d = tts::synthesize("Testing the default voice.", None, None, &out).unwrap();
    assert!((probe_duration(&out) - d).abs() < 0.01 && d > 0.5);
}

#[test]
fn chosen_voice_and_rate_are_honoured() {
    let dir = tempfile::tempdir().unwrap();
    let text = "The same sentence spoken by two different voices.";
    let (a, b, a2) = (dir.path().join("a.wav"), dir.path().join("b.wav"), dir.path().join("a2.wav"));
    tts::synthesize(text, Some("Samantha"), None, &a).unwrap();
    tts::synthesize(text, Some("Daniel"), None, &b).unwrap();
    tts::synthesize(text, Some("Samantha"), None, &a2).unwrap();
    let (pa, pb, pa2) = (decode_pcm(&a), decode_pcm(&b), decode_pcm(&a2));
    let difference = |x: &[f32], y: &[f32]| {
        let n = x.len().min(y.len());
        let sample_diff: f32 = x[..n].iter().zip(&y[..n]).map(|(p, q)| (p - q).abs()).sum::<f32>() / n as f32;
        sample_diff + (x.len() as f32 - y.len() as f32).abs() / SR as f32
    };
    let same_voice = difference(&pa, &pa2);
    let other_voice = difference(&pa, &pb);
    assert!(other_voice > 0.01 && other_voice > same_voice * 5.0 + 0.005, "different voices should give different audio (same voice twice: {same_voice}, other voice: {other_voice})");

    let (slow, fast) = (dir.path().join("slow.wav"), dir.path().join("fast.wav"));
    let d_slow = tts::synthesize(text, Some("Samantha"), Some(110), &slow).unwrap();
    let d_fast = tts::synthesize(text, Some("Samantha"), Some(300), &fast).unwrap();
    assert!(d_slow > d_fast * 1.5, "110 wpm took {d_slow} s, 300 wpm took {d_fast} s");
}

#[test]
fn bad_requests_are_errors_and_leave_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.wav");
    for text in ["", "   \n\t "] {
        let err = tts::synthesize(text, None, None, &out).unwrap_err();
        assert!(err.to_string().contains("no text"), "{err}");
    }
    let err = tts::synthesize("Hello", Some("No Such Voice 123"), None, &out).unwrap_err();
    assert!(err.to_string().contains("No Such Voice 123") && err.to_string().contains("not installed"), "{err}");
    let err = tts::synthesize("Hello", None, Some(5), &out).unwrap_err();
    assert!(err.to_string().contains("rate"), "{err}");
    assert!(!out.exists());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing may be left behind");
}

#[test]
fn text_that_looks_like_options_is_spoken_not_interpreted() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("dash.wav");
    let d = tts::synthesize("-v Daniel --rate 900 is just text here", Some("Samantha"), None, &out).unwrap();
    assert!(d > 1.0, "the whole sentence should be spoken, got {d} s");
}

#[test]
fn existing_file_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("again.wav");
    std::fs::write(&out, b"old junk").unwrap();
    let d = tts::synthesize("Fresh audio.", Some("Samantha"), None, &out).unwrap();
    assert!((probe_duration(&out) - d).abs() < 0.01);
}

#[test]
fn round_trip_speech_to_text() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("roundtrip.wav");
    let sentence = "The meeting starts at nine tomorrow morning, so please bring your notes and a cup of coffee.";
    let duration = tts::synthesize(sentence, Some("Samantha"), None, &out).unwrap();
    let opts = transcribe::Options::new(model("ggml-base.en.bin"));
    let t = transcribe::transcribe(&out, &opts, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let heard = t.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    let error_rate = wer(&norm_words(sentence), &norm_words(&heard));
    assert!(error_rate <= 0.07, "said {sentence:?}, heard {heard:?} (WER {error_rate:.3})");
    assert!(t.words.last().unwrap().end <= duration + 1e-6);
    assert!(t.words[0].start < 0.5 && t.words.last().unwrap().end > duration - 1.0);
}
