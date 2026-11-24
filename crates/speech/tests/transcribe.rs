//! Transcription against real speech generated with the system synthesiser.
mod common;

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::*;
use mango_core::Transcript;
use mango_speech::transcribe::{self, Options};

struct Fixture {
    _dir: tempfile::TempDir,
    clip: Clip,
    wav: PathBuf,
    dir: PathBuf,
}

/// The six-phrase clip with known phrase times, built once per test run.
fn fixture() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let clip = standard_clip(dir.path());
        let wav = dir.path().join("passage.wav");
        write_wav(&wav, &clip.samples, SR as u32);
        Fixture { dir: dir.path().to_path_buf(), _dir: dir, clip, wav }
    })
}

fn base() -> Options {
    Options::new(model("ggml-base.en.bin"))
}

fn run(media: &std::path::Path, opts: &Options) -> anyhow::Result<Transcript> {
    transcribe::transcribe(media, opts, &mut |_| {}, &AtomicBool::new(false))
}

fn joined(t: &Transcript) -> String {
    t.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
}

fn assert_well_formed(t: &Transcript, duration: f64) {
    let mut cursor = 0.0;
    for w in &t.words {
        assert!(!w.text.is_empty() && w.text == w.text.trim(), "word text has stray whitespace: {:?}", w.text);
        assert!(w.text.chars().any(char::is_alphanumeric), "punctuation must be attached to a word, got {:?}", w.text);
        assert!(w.start.is_finite() && w.end.is_finite());
        assert!(w.start >= cursor - 1e-9, "words overlap or go backwards at {:?} (previous ended {cursor})", w);
        assert!(w.end >= w.start, "negative duration: {w:?}");
        assert!(w.end <= duration + 1e-6, "word ends after the audio ({duration}): {w:?}");
        assert!((0.0..=1.0).contains(&w.confidence), "confidence out of range: {w:?}");
        cursor = w.end;
    }
}

/// Index of the first reference word of each phrase.
fn phrase_first_words() -> Vec<usize> {
    PHRASES
        .iter()
        .scan(0, |acc, p| {
            let i = *acc;
            *acc += norm_words(p).len();
            Some(i)
        })
        .collect()
}

#[test]
fn recognises_the_passage_with_accurate_phrase_timing() {
    let f = fixture();
    let t = run(&f.wav, &base()).unwrap();
    let duration = f.clip.duration();
    assert_eq!(t.language, "en");
    assert_well_formed(&t, duration);

    let reference = norm_words(&PHRASES.join(" "));
    assert!(reference.len() >= 40);
    let hypothesis: Vec<String> = t.words.iter().map(|w| norm_words(&w.text).join("")).collect();
    let error_rate = wer(&reference, &hypothesis);
    println!("WER {error_rate:.3} over {} words: {}", reference.len(), joined(&t));
    assert!(error_rate <= 0.10, "word error rate {error_rate:.3} is above 10%: {}", joined(&t));

    // Punctuation is attached to words, never free-standing.
    assert!(t.words.iter().filter(|w| w.text.ends_with('.')).count() >= 5, "sentence ends should carry their full stop: {}", joined(&t));
    assert!(t.words.iter().all(|w| w.confidence > 0.0), "confidence should come from token probabilities");
    assert!(t.words.iter().any(|w| w.confidence < 1.0), "confidence should not be a constant 1.0");

    let map = align_indices(&reference, &hypothesis);
    let firsts = phrase_first_words();
    let mut start_errors = Vec::new();
    let mut end_errors = Vec::new();
    for (k, &(phrase_start, phrase_end)) in f.clip.phrases.iter().enumerate() {
        let first = map[firsts[k]].unwrap_or_else(|| panic!("first word of phrase {k} was not recognised: {}", joined(&t)));
        let last_ref = if k + 1 < firsts.len() { firsts[k + 1] - 1 } else { reference.len() - 1 };
        let last = map[last_ref].unwrap_or_else(|| panic!("last word of phrase {k} was not recognised: {}", joined(&t)));
        let start_error = t.words[first].start - phrase_start;
        assert!(start_error.abs() <= 0.20, "phrase {k} starts at {phrase_start:.2} but its first word starts at {:.2}", t.words[first].start);
        if let Some(next) = f.clip.phrases.get(k + 1) {
            assert!(t.words[last].end <= next.0, "last word of phrase {k} ends at {:.2}, after the next phrase starts ({:.2})", t.words[last].end, next.0);
        }
        assert!(t.words[last].end > phrase_start);
        // Every word of the phrase lies inside it (give or take the tolerance).
        for w in &t.words[first..=last] {
            assert!(w.start >= phrase_start - 0.20 && w.end <= phrase_end + 0.20, "word {w:?} strays outside its phrase {phrase_start:.2}..{phrase_end:.2}");
        }
        start_errors.push(start_error);
        end_errors.push(t.words[last].end - phrase_end);
    }
    let (s, e) = (stats(&start_errors), stats(&end_errors));
    println!("phrase start error: mean {:.3} s, max {:.3} s (n={}); phrase end error: mean {:.3} s, max {:.3} s", s.mean, s.max, s.n, e.mean, e.max);
    assert!(e.max <= 0.20, "phrase ends are off by up to {:.3} s", e.max);
}

#[test]
fn inner_word_boundaries_are_close_to_the_truth() {
    // Words synthesised one by one, 40 ms apart: boundaries are known exactly and the pauses
    // are far too short for pause detection, so this measures the alignment itself.
    let dir = tempfile::tempdir().unwrap();
    let words: Vec<&str> = ISOLATED.split(' ').collect();
    let (samples, truth) = isolated_words_clip(dir.path(), &words);
    let t = transcribe::transcribe_samples(&samples, &base(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_well_formed(&t, samples.len() as f64 / SR as f64);
    let reference = norm_words(ISOLATED);
    let hypothesis: Vec<String> = t.words.iter().map(|w| norm_words(&w.text).join("")).collect();
    let map = align_indices(&reference, &hypothesis);
    let errors: Vec<f64> = map.iter().enumerate().filter_map(|(r, h)| h.map(|h| t.words[h].start - truth[r].0)).collect();
    assert!(errors.len() >= reference.len() - 2, "too few words recognised: {}", joined(&t));
    let s = stats(&errors);
    println!("inner word start error: mean {:.3} s, max {:.3} s (n={})", s.mean, s.max, s.n);
    assert!(s.mean <= 0.12, "mean word start error {:.3} s", s.mean);
    assert!(s.max <= 0.30, "max word start error {:.3} s", s.max);
}

#[test]
fn silence_gives_no_words() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("silence.wav");
    write_wav(&wav, &vec![0.0; SR * 12], SR as u32);
    let mut last = -1.0;
    let t = transcribe::transcribe(&wav, &base(), &mut |p| last = p, &AtomicBool::new(false)).unwrap();
    assert!(t.words.is_empty(), "silence produced words: {}", joined(&t));
    assert_eq!(last, 1.0);
}

#[test]
fn faint_hiss_gives_no_words() {
    // Room tone without speech is where whisper likes to invent "Thank you."
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("hiss.wav");
    let mut x = 7u32;
    let hiss: Vec<f32> = (0..SR * 12)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.004
        })
        .collect();
    write_wav(&wav, &hiss, SR as u32);
    let t = run(&wav, &base()).unwrap();
    assert!(t.words.is_empty(), "hiss produced words: {}", joined(&t));
}

#[test]
fn speech_followed_by_long_silence_gets_no_invented_words() {
    let f = fixture();
    let (start, end) = f.clip.phrases[0];
    let mut samples = f.clip.samples[..(end * SR as f64) as usize].to_vec();
    samples.extend(vec![0.0f32; SR * 20]);
    let t = transcribe::transcribe_samples(&samples, &base(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let hypothesis = norm_words(&joined(&t));
    assert!(wer(&norm_words(PHRASES[0]), &hypothesis) <= 0.10, "got: {}", joined(&t));
    let last = t.words.last().unwrap();
    assert!(last.end <= end + 0.2 && t.words[0].start >= start - 0.2, "words leak into the silence: {:?}", t.words);
}

#[test]
fn video_file_with_audio_is_transcribed() {
    let f = fixture();
    let mp4 = f.dir.join("clip.mp4");
    common::run(Command::new(ffmpeg())
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25", "-i"])
        .arg(&f.wav)
        .args(["-t", "9", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k", "-shortest"])
        .arg(&mp4));
    let t = run(&mp4, &base()).unwrap();
    assert_well_formed(&t, 9.2);
    let reference = norm_words(&PHRASES[..2].join(" "));
    let hypothesis = norm_words(&joined(&t));
    let error_rate = wer(&reference, &hypothesis);
    assert!(error_rate <= 0.15, "WER {error_rate:.3} on the video's audio: {}", joined(&t));
    // Source time is preserved through the container.
    assert!((t.words[0].start - f.clip.phrases[0].0).abs() <= 0.20, "first word at {:.2}", t.words[0].start);
}

#[test]
fn file_without_audio_stream_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let mp4 = dir.path().join("mute.mp4");
    common::run(Command::new(ffmpeg())
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=160x120:rate=10", "-t", "1", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-an"])
        .arg(&mp4));
    let err = run(&mp4, &base()).unwrap_err();
    assert!(!mango_speech::is_cancelled(&err));
    assert!(err.to_string().contains("no audio stream"), "unclear error: {err}");
}

#[test]
fn missing_file_bad_model_and_non_media_are_clear_errors() {
    let f = fixture();
    let err = run(std::path::Path::new("/definitely/not/here.mov"), &base()).unwrap_err();
    assert!(err.to_string().contains("not found") && err.to_string().contains("here.mov"), "unclear error: {err}");

    let err = run(&f.wav, &Options::new("/no/such/model.bin")).unwrap_err();
    assert!(err.to_string().contains("model") && err.to_string().contains("/no/such/model.bin"), "unclear error: {err}");

    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("ggml-fake.bin");
    std::fs::write(&fake, b"this is not a model at all").unwrap();
    let err = run(&f.wav, &Options::new(&fake)).unwrap_err();
    assert!(err.to_string().contains("not a whisper"), "unclear error: {err}");

    // Right magic number, garbage after it: must fail cleanly, not crash the process.
    let garbage = dir.path().join("ggml-garbage.bin");
    let mut bytes = 0x6767_6d6cu32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[7u8; 300]);
    std::fs::write(&garbage, bytes).unwrap();
    let err = run(&f.wav, &Options::new(&garbage)).unwrap_err();
    assert!(err.to_string().contains("not a whisper"), "unclear error: {err}");

    // A real model cut short (an interrupted download that was renamed by hand).
    let truncated = dir.path().join("ggml-truncated.bin");
    let real = std::fs::read(model("ggml-tiny.en.bin")).unwrap();
    std::fs::write(&truncated, &real[..real.len() / 3]).unwrap();
    let err = run(&f.wav, &Options::new(&truncated)).unwrap_err();
    assert!(err.to_string().contains("incomplete or damaged"), "unclear error: {err}");

    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "just some text").unwrap();
    let err = run(&text, &base()).unwrap_err();
    assert!(err.to_string().contains("notes.txt"), "unclear error: {err}");

    let mut opts = base();
    opts.language = Some("de".into());
    let err = run(&f.wav, &opts).unwrap_err();
    assert!(err.to_string().contains("only understands English"), "unclear error: {err}");
}

/// The passage repeated until it is `minutes` long.
fn long_wav(name: &str, minutes: f64) -> PathBuf {
    let f = fixture();
    let mut samples = Vec::new();
    while (samples.len() as f64) < minutes * 60.0 * SR as f64 {
        samples.extend_from_slice(&f.clip.samples);
    }
    let wav = f.dir.join(name);
    write_wav(&wav, &samples, SR as u32);
    wav
}

#[test]
fn progress_is_monotonic_and_ends_at_one() {
    let wav = long_wav("two-minutes.wav", 2.0);
    let mut seen: Vec<f32> = Vec::new();
    let t = transcribe::transcribe(&wav, &base(), &mut |p| seen.push(p), &AtomicBool::new(false)).unwrap();
    assert!(t.words.len() > 200, "expected the whole file to be transcribed, got {} words", t.words.len());
    assert!(seen.iter().all(|p| (0.0..=1.0).contains(p)), "{seen:?}");
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "progress went backwards: {seen:?}");
    assert_eq!(*seen.last().unwrap(), 1.0, "{seen:?}");
    assert_eq!(seen[0], 0.0, "{seen:?}");
    let mut distinct = seen.clone();
    distinct.dedup();
    assert!(distinct.len() >= 5, "progress should advance in steps on a long file: {seen:?}");
    // Words keep source time across whisper's 30-second windows.
    assert_well_formed(&t, probe_duration(&wav));
    assert!(t.words.last().unwrap().end > 100.0);
}

#[test]
fn cancel_stops_a_long_transcription_quickly() {
    let wav = long_wav("ten-minutes.wav", 10.0);
    // Reference: how long the whole job takes (so "quickly" means something).
    let cancel = AtomicBool::new(false);
    let cancelled_at: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
    let started = Instant::now();
    let result = transcribe::transcribe(
        &wav,
        &base(),
        &mut |p| {
            if p >= 0.15 && !cancel.swap(true, Ordering::SeqCst) {
                *cancelled_at.lock().unwrap() = Some(Instant::now());
            }
        },
        &cancel,
    );
    let returned = Instant::now();
    let err = result.expect_err("a cancelled transcription must not return a transcript");
    assert!(mango_speech::is_cancelled(&err), "expected the Cancelled error, got: {err}");
    assert!(err.downcast_ref::<mango_speech::Cancelled>().is_some());
    let at = cancelled_at.lock().unwrap().expect("progress reached 15%");
    let latency = returned.duration_since(at);
    println!("cancel latency {:?}, total {:?}", latency, started.elapsed());
    assert!(latency < Duration::from_millis(1500), "took {latency:?} to stop after cancel");

    // Already cancelled before starting: returns at once.
    let started = Instant::now();
    let err = transcribe::transcribe(&wav, &base(), &mut |_| {}, &AtomicBool::new(true)).unwrap_err();
    assert!(mango_speech::is_cancelled(&err));
    assert!(started.elapsed() < Duration::from_millis(1500));
}

#[test]
fn glossary_spellings_are_preferred() {
    // Invented names whisper cannot know how to spell. Counted over two voices.
    let dir = tempfile::tempdir().unwrap();
    let cases = [
        ("Please send the invoice to Kaelthas Brightwynn before Monday.", ["Kaelthas", "Brightwynn"]),
        ("We spoke with Doctor Zephyrix about the Xylotek sensor.", ["Zephyrix", "Xylotek"]),
    ];
    let (mut with, mut without, mut total) = (0, 0, 0);
    for voice in ["Samantha", "Daniel"] {
        for (sentence, terms) in &cases {
            let mut pcm = vec![0.0f32; SR / 2];
            pcm.extend(say_pcm(dir.path(), sentence, Some(voice)));
            pcm.extend(vec![0.0f32; SR / 2]);
            let mut opts = base();
            let plain = transcribe::transcribe_samples(&pcm, &opts, &mut |_| {}, &AtomicBool::new(false)).unwrap();
            opts.glossary = terms.iter().map(|t| t.to_string()).collect();
            let hinted = transcribe::transcribe_samples(&pcm, &opts, &mut |_| {}, &AtomicBool::new(false)).unwrap();
            println!("{voice}: plain  = {}\n{voice}: hinted = {}", joined(&plain), joined(&hinted));
            for term in terms {
                total += 1;
                without += usize::from(joined(&plain).contains(term));
                with += usize::from(joined(&hinted).contains(term));
            }
            // The prompt itself must never leak into the transcript.
            assert!(!joined(&hinted).contains("Glossary"), "prompt leaked: {}", joined(&hinted));
        }
    }
    println!("glossary spelling used: {with}/{total} with the glossary, {without}/{total} without");
    assert!(with > without, "glossary did not help: {with}/{total} with vs {without}/{total} without");
}

#[test]
fn two_transcriptions_can_run_at_the_same_time() {
    let f = fixture();
    let results: Vec<Transcript> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..2).map(|_| s.spawn(|| run(&f.wav, &base()).unwrap())).collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(joined(&results[0]), joined(&results[1]));
    assert!(results[0].words.len() > 50);
}

#[test]
fn tiny_model_also_works() {
    let f = fixture();
    let t = run(&f.wav, &Options::new(model("ggml-tiny.en.bin"))).unwrap();
    assert_well_formed(&t, f.clip.duration());
    let error_rate = wer(&norm_words(&PHRASES.join(" ")), &norm_words(&joined(&t)));
    assert!(error_rate <= 0.20, "tiny.en WER {error_rate:.3}: {}", joined(&t));
}

#[test]
#[ignore = "measurement: prints transcription speed for tiny.en and base.en"]
fn measure_speed() {
    let wav = long_wav("speed-five-minutes.wav", 5.0);
    let audio = probe_duration(&wav);
    for name in ["ggml-tiny.en.bin", "ggml-base.en.bin"] {
        for pass in 0..2 {
            let started = Instant::now();
            let t = run(&wav, &Options::new(model(name))).unwrap();
            let secs = started.elapsed().as_secs_f64();
            println!("{name} pass {pass}: {audio:.0} s of audio in {secs:.2} s = {:.0}x realtime, {} words, gpu={}", audio / secs, t.words.len(), transcribe::gpu_enabled());
        }
    }
}

// ---------------------------------------------------------------------------
// Regression: words at the start of an audible stretch collapsed to 20 ms slivers and their
// audio was attributed to the next word (short 0.2 s pauses between sentences).
// ---------------------------------------------------------------------------

const SHORT_PAUSES: &str = "Hello there. Um, this is a test of the mango editor. The weather is lovely today. Goodbye everyone.";

struct ShortPauses {
    samples: Vec<f32>,
    transcript: Transcript,
}

fn short_pauses() -> &'static ShortPauses {
    static F: OnceLock<ShortPauses> = OnceLock::new();
    F.get_or_init(|| {
        // As the app does it: synthesised speech muxed to mp4 / AAC 48 kHz.
        let dir = tempfile::tempdir().unwrap();
        let aiff = dir.path().join("speech.aiff");
        common::run(Command::new("/usr/bin/say").args(["-v", "Samantha", "-o"]).arg(&aiff).arg(SHORT_PAUSES));
        let mp4 = dir.path().join("speech.mp4");
        common::run(Command::new(ffmpeg()).args(["-v", "error", "-y", "-i"]).arg(&aiff).args(["-c:a", "aac", "-b:a", "160k", "-ar", "48000"]).arg(&mp4));
        let transcript = run(&mp4, &base()).unwrap();
        let samples = transcribe::decode_audio(&mp4, &AtomicBool::new(false)).unwrap();
        ShortPauses { samples, transcript }
    })
}

fn plain(text: &str) -> String {
    norm_words(text).join("")
}

#[test]
fn no_word_is_a_sliver_and_every_word_sits_on_audible_sound() {
    let f = short_pauses();
    let t = &f.transcript;
    let duration = f.samples.len() as f64 / SR as f64;
    assert_well_formed(t, duration);
    for w in &t.words {
        println!("{:>10} {:.2}-{:.2} ({:.0} ms)", w.text, w.start, w.end, (w.end - w.start) * 1000.0);
    }
    assert_eq!(wer(&norm_words(SHORT_PAUSES), &norm_words(&joined(t))), 0.0, "{}", joined(t));
    let stretches = transcribe::audible_stretches(&f.samples).expect("speech with pauses has stretches");
    assert!(stretches.len() >= 4, "the sentence has pauses: {stretches:?}");
    for w in &t.words {
        let length = w.end - w.start;
        let letters = w.text.chars().filter(|c| c.is_alphanumeric()).count() as f64;
        let plausible_minimum = (0.035 * letters).clamp(0.06, 0.20);
        assert!(length >= plausible_minimum - 1e-9, "{w:?} is shorter than a spoken word of its length can be");
        // Only one- and two-letter words ("a", "is", "of") are genuinely shorter than 120 ms.
        if letters >= 3.0 {
            assert!(length >= 0.12, "{w:?} is a sliver");
        }
        assert!(length <= 0.30 + 0.09 * letters + 1e-9, "{w:?} is implausibly long: it probably swallowed a neighbour");
        // The whole word lies inside one audible stretch: it never covers a pause, and it never sits in one.
        assert!(
            stretches.iter().any(|&(a, b)| w.start >= a - 0.03 && w.end <= b + 0.03),
            "{w:?} is not inside an audible stretch {stretches:?}"
        );
    }
    // Words on either side of a pause do not touch across it.
    for pair in t.words.windows(2) {
        if pair[0].text.ends_with(['.', ',']) {
            assert!(pair[1].start - pair[0].end >= 0.1, "the pause between {:?} and {:?} was given to a word", pair[0].text, pair[1].text);
        }
    }
}

/// Removes `words[i]` from the audio at its boundaries, transcribes what is left, and says
/// whether exactly that word is gone: it must no longer be heard, and the words on either
/// side must still be heard.
fn cut_removes_exactly(f: &ShortPauses, i: usize) -> Result<(), String> {
    let t = &f.transcript;
    let reference: Vec<String> = t.words.iter().map(|w| plain(&w.text)).collect();
    let w = &t.words[i];
    let (from, to) = ((w.start * SR as f64).round() as usize, (w.end * SR as f64).round() as usize);
    let mut cut = f.samples[..from].to_vec();
    cut.extend_from_slice(&f.samples[to.min(f.samples.len())..]);
    let heard = transcribe::transcribe_samples(&cut, &base(), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let heard_words: Vec<String> = heard.words.iter().map(|w| plain(&w.text)).collect();
    let count = |words: &[String], x: &str| words.iter().filter(|w| *w == x).count();
    let target = &reference[i];
    if count(&heard_words, target) + 1 != count(&reference, target) {
        return Err(format!("{:?} is still audible after cutting {:.2}-{:.2}: {}", w.text, w.start, w.end, joined(&heard)));
    }
    for neighbour in [i.checked_sub(1), (i + 1 < reference.len()).then_some(i + 1)].into_iter().flatten() {
        let n = &reference[neighbour];
        if count(&heard_words, n) != count(&reference, n) {
            return Err(format!("cutting {:?} ({:.2}-{:.2}) damaged its neighbour {n:?}: {}", w.text, w.start, w.end, joined(&heard)));
        }
    }
    Ok(())
}

#[test]
fn cutting_a_word_out_of_the_audio_removes_exactly_that_word() {
    // The property the editor depends on: delete a word in the text, cut the media at the
    // word's boundaries, and that word (and only that word) is gone from the sound.
    let f = short_pauses();
    let reference: Vec<String> = f.transcript.words.iter().map(|w| plain(&w.text)).collect();
    let index = |word: &str, after: Option<&str>| {
        let from = after.map(|a| reference.iter().position(|w| w == a).expect("anchor word recognised") + 1).unwrap_or(0);
        from + reference[from..].iter().position(|w| w == word).unwrap_or_else(|| panic!("{word} was not recognised: {}", joined(&f.transcript)))
    };
    // Every first word after a pause ("Hello", "Um", "this", "The" [weather], "Goodbye"), the
    // last word, and two words in mid-phrase. These are words whose removal leaves a
    // sentence that still makes sense; see `most_words_can_be_cut_cleanly` for the rest.
    let targets = [
        index("hello", None),
        index("um", None),
        index("this", None),
        index("mango", None),
        index("the", Some("editor")),
        index("lovely", None),
        index("goodbye", None),
        index("everyone", None),
    ];
    let failures: Vec<String> = targets.iter().filter_map(|&i| cut_removes_exactly(f, i).err()).collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn most_words_can_be_cut_cleanly() {
    // The same check for every word of the sentence. It cannot be 100%: when the removed
    // word leaves an ungrammatical remainder ("this is a ... of the mango editor"), the
    // recogniser used as the judge rewrites the neighbours ("this isn't of ..."), and inner
    // boundaries in continuous speech are only accurate to a few hundredths of a second.
    let f = short_pauses();
    let results: Vec<Result<(), String>> = (0..f.transcript.words.len()).map(|i| cut_removes_exactly(f, i)).collect();
    let failures: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    let passed = results.len() - failures.len();
    println!("cut cleanly: {passed}/{} words", results.len());
    for failure in &failures {
        println!("  {failure}");
    }
    assert!(passed * 3 >= results.len() * 2, "only {passed}/{} words can be cut cleanly:\n{}", results.len(), failures.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"));
}
