//! Measurements behind the choice of timestamp strategy. Run with
//! `cargo test -p mango-speech --test eval -- --ignored --nocapture`.
mod common;
use common::*;
use mango_speech::transcribe::{self, Timing};

#[test]
#[ignore = "measurement, prints a comparison of timestamp strategies"]
fn compare_timestamp_strategies() {
    let dir = tempfile::tempdir().unwrap();
    let clip = standard_clip(dir.path());
    let reference: Vec<String> = norm_words(&PHRASES.join(" "));
    let firsts: Vec<usize> = PHRASES.iter().scan(0, |acc, p| { let i = *acc; *acc += norm_words(p).len(); Some(i) }).collect();
    let (iso, iso_truth) = isolated_words_clip(dir.path(), &ISOLATED.split(' ').collect::<Vec<_>>());
    let iso_ref = norm_words(ISOLATED);
    for m in ["ggml-tiny.en.bin", "ggml-base.en.bin"] {
        let opts = transcribe::Options::new(model(m));
        for timing in [Timing::TokenHeuristic, Timing::DtwRaw, Timing::DtwSnapped] {
            let t = transcribe::transcribe_samples_with(&clip.samples, &opts, timing).unwrap();
            let hyp: Vec<String> = t.words.iter().map(|w| norm_words(&w.text).join("")).collect();
            let map = align_indices(&reference, &hyp);
            let mut start_err = Vec::new();
            let mut end_err = Vec::new();
            let mut overruns = 0;
            for (k, &(ps, pe)) in clip.phrases.iter().enumerate() {
                if let Some(i) = map[firsts[k]] { start_err.push(t.words[i].start - ps); }
                let last_ref = if k + 1 < firsts.len() { firsts[k + 1] - 1 } else { reference.len() - 1 };
                if let Some(i) = map[last_ref] {
                    end_err.push(t.words[i].end - pe);
                    if k + 1 < clip.phrases.len() && t.words[i].end > clip.phrases[k + 1].0 { overruns += 1; }
                }
            }
            let (s, e) = (stats(&start_err), stats(&end_err));
            let ti = transcribe::transcribe_samples_with(&iso, &opts, timing).unwrap();
            let hyp: Vec<String> = ti.words.iter().map(|w| norm_words(&w.text).join("")).collect();
            let map = align_indices(&iso_ref, &hyp);
            let mut b = Vec::new();
            for (r, h) in map.iter().enumerate() { if let Some(h) = h { b.push(ti.words[*h].start - iso_truth[r].0); } }
            let bs = stats(&b);
            println!("{m:18} {timing:?}: WER {:.3} | phrase start err mean {:.3} max {:.3} (n={}) {:?} | phrase end err mean {:.3} max {:.3} overruns {overruns} | isolated-word start err mean {:.3} max {:.3} (n={})",
                wer(&reference, &norm_words(&t.words.iter().map(|w| w.text.clone()).collect::<Vec<_>>().join(" "))),
                s.mean, s.max, s.n, start_err.iter().map(|v| (v * 100.0).round() / 100.0).collect::<Vec<_>>(), e.mean, e.max, bs.mean, bs.max, bs.n);
        }
    }
}

#[test]
#[ignore = "measurement"]
fn long_file_diff() {
    let dir = tempfile::tempdir().unwrap();
    let clip = standard_clip(dir.path());
    let mut samples = Vec::new();
    let mut reference = Vec::new();
    for _ in 0..5 { samples.extend_from_slice(&clip.samples); reference.extend(norm_words(&PHRASES.join(" "))); }
    let opts = transcribe::Options::new(model("ggml-base.en.bin"));
    for timing in [Timing::DtwRaw, Timing::DtwSnapped] {
        let t = transcribe::transcribe_samples_with(&samples, &opts, timing).unwrap();
        let hyp: Vec<String> = t.words.iter().map(|w| norm_words(&w.text).join("")).collect();
        let map = align_indices(&reference, &hyp);
        let missing: Vec<(usize, &String)> = map.iter().enumerate().filter(|(_, h)| h.is_none()).map(|(i, _)| (i, &reference[i])).collect();
        println!("{timing:?}: {} words, WER {:.3}, missing {missing:?}", hyp.len(), wer(&reference, &hyp));
        for w in t.words.iter().filter(|w| w.start > 24.0 && w.start < 32.0) { println!("   {:.2}-{:.2} {}", w.start, w.end, w.text); }
    }
}

pub const REPRO: &str = "Hello there. Um, this is a test of the mango editor. The weather is lovely today. Goodbye everyone.";

#[test]
#[ignore = "measurement"]
fn repro_sentence() {
    let dir = tempfile::tempdir().unwrap();
    let aiff = dir.path().join("speech.aiff");
    run(std::process::Command::new("/usr/bin/say").args(["-v", "Samantha", "-o"]).arg(&aiff).arg(REPRO));
    let samples = decode_pcm(&aiff);
    println!("stretches {:?}", transcribe::audible_stretches(&samples));
    let opts = transcribe::Options::new(model("ggml-base.en.bin"));
    let raw = transcribe::transcribe_samples_with(&samples, &opts, Timing::DtwRaw).unwrap();
    let snap = transcribe::transcribe_samples_with(&samples, &opts, Timing::DtwSnapped).unwrap();
    for (r, s) in raw.words.iter().zip(&snap.words) {
        println!("{:>10} raw {:.2} | {:.2}-{:.2} ({:.0} ms)", s.text, r.start, s.start, s.end, (s.end - s.start) * 1000.0);
    }
}

#[test]
#[ignore = "measurement"]
fn anchors_on_isolated_words() {
    let dir = tempfile::tempdir().unwrap();
    let (iso, truth) = isolated_words_clip(dir.path(), &ISOLATED.split(' ').collect::<Vec<_>>());
    println!("truth starts {:?}", truth.iter().map(|t| (t.0 * 100.0).round() / 100.0).collect::<Vec<_>>());
    for m in ["ggml-base.en.bin", "ggml-tiny.en.bin"] {
        println!("== {m}");
        transcribe::transcribe_samples_with(&iso, &transcribe::Options::new(model(m)), Timing::DtwSnapped).unwrap();
    }
}

/// How often cutting the audio at a word's boundaries removes that word and leaves its
/// neighbours intact, judged by transcribing the remainder.
#[test]
#[ignore = "measurement"]
fn cut_property_pass_rate() {
    use std::sync::atomic::AtomicBool;
    let dir = tempfile::tempdir().unwrap();
    let sentences = [
        ("Samantha", REPRO),
        ("Daniel", "Welcome back. Today we are going to look at three simple ideas. First, keep every recording short. Second, always check the sound."),
        ("Samantha", "I think the answer is probably no. But, um, we could try again on Friday morning. Does that work for everybody?"),
    ];
    let opts = transcribe::Options::new(model("ggml-base.en.bin"));
    let (mut pass, mut total, mut pass_first, mut total_first) = (0, 0, 0, 0);
    for (voice, text) in sentences {
        let samples = say_pcm(dir.path(), text, Some(voice));
        let t = transcribe::transcribe_samples(&samples, &opts, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        let reference: Vec<String> = t.words.iter().map(|w| norm_words(&w.text).join("")).collect();
        let count = |words: &[String], x: &str| words.iter().filter(|w| *w == x).count();
        for (i, w) in t.words.iter().enumerate() {
            let (from, to) = ((w.start * SR as f64).round() as usize, ((w.end * SR as f64).round() as usize).min(samples.len()));
            let mut cut = samples[..from].to_vec();
            cut.extend_from_slice(&samples[to..]);
            let heard = transcribe::transcribe_samples(&cut, &opts, &mut |_| {}, &AtomicBool::new(false)).unwrap();
            let hw: Vec<String> = heard.words.iter().map(|w| norm_words(&w.text).join("")).collect();
            let mut ok = count(&hw, &reference[i]) + 1 == count(&reference, &reference[i]);
            for nb in [i.checked_sub(1), (i + 1 < reference.len()).then_some(i + 1)].into_iter().flatten() {
                ok &= count(&hw, &reference[nb]) == count(&reference, &reference[nb]);
            }
            let first = i == 0 || w.start - t.words[i - 1].end > 0.1;
            total += 1; pass += usize::from(ok);
            if first { total_first += 1; pass_first += usize::from(ok); }
            println!("{} {:>10} {:.2}-{:.2} {}", if ok { "ok  " } else { "FAIL" }, w.text, w.start, w.end, if ok { String::new() } else { heard.words.iter().map(|w| w.text.clone()).collect::<Vec<_>>().join(" ") });
        }
    }
    println!("CUT PASS {pass}/{total} overall, {pass_first}/{total_first} for the first word after a pause");
}
