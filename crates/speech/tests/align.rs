use mango_core::{Transcript, TranscriptWord};
use mango_speech::align::{AlignedWord, align_text, speaker_names};

const SPOKEN: &str = "Welcome back to the show. Today we talk about editing video with text, and why it saves time.";

/// A reference transcript with 0.3 s words and 0.1 s gaps.
fn reference(text: &str) -> Transcript {
    let words = text
        .split_whitespace()
        .enumerate()
        .map(|(i, w)| TranscriptWord { text: w.to_string(), start: 1.0 + i as f64 * 0.4, end: 1.3 + i as f64 * 0.4, speaker: None, confidence: 0.9 })
        .collect();
    Transcript { language: "en".into(), words }
}

fn assert_monotonic(words: &[AlignedWord], duration: f64) {
    let mut cursor = 0.0;
    for w in words {
        assert!(w.start >= cursor - 1e-9 && w.end >= w.start && w.end <= duration + 1e-9, "not monotonic inside 0..{duration}: {words:#?}");
        cursor = w.end;
    }
}

#[test]
fn exact_text_matches_every_word_with_reference_times() {
    let r = reference(SPOKEN);
    let out = align_text(&r, SPOKEN, 60.0);
    assert_eq!(out.len(), r.words.len());
    for (a, w) in out.iter().zip(&r.words) {
        assert!(a.matched, "{a:?}");
        assert_eq!((a.text.as_str(), a.start, a.end), (w.text.as_str(), w.start, w.end));
        assert_eq!(a.speaker, None);
    }
}

#[test]
fn case_punctuation_and_line_breaks_do_not_matter_and_user_spelling_is_kept() {
    let r = reference(SPOKEN);
    let user = "WELCOME back to the show!\n\ntoday, we talk about editing video with text -- and why it saves time";
    let out = align_text(&r, user, 60.0);
    assert_eq!(out.len(), r.words.len(), "the free-standing dash must attach to a word: {out:#?}");
    assert!(out.iter().all(|w| w.matched));
    assert_eq!(out[0].text, "WELCOME");
    assert_eq!(out[4].text, "show!");
    assert!(out.iter().any(|w| w.text == "text--"), "{out:#?}");
    for (a, w) in out.iter().zip(&r.words) {
        assert_eq!((a.start, a.end), (w.start, w.end));
    }
}

#[test]
fn changed_inserted_and_deleted_words_keep_every_user_word() {
    let r = reference(SPOKEN);
    // changed: show -> programme, saves -> spares; inserted: "really" "truly" after "why it";
    // deleted: "Today"; inserted at the very start: "Hello"; at the very end: "indeed".
    let user = "Hello welcome back to the programme. We talk about editing video with text, and why it really truly spares time indeed";
    let out = align_text(&r, user, 60.0);
    let user_words: Vec<&str> = user.split_whitespace().collect();
    assert_eq!(out.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), user_words, "every user word, in order");
    assert_monotonic(&out, 60.0);

    let get = |text: &str| out.iter().find(|w| w.text == text).unwrap_or_else(|| panic!("{text} missing"));
    let rw = |text: &str| r.words.iter().find(|w| w.text == text).unwrap();
    let unmatched: Vec<&str> = out.iter().filter(|w| !w.matched).map(|w| w.text.as_str()).collect();
    assert_eq!(unmatched, ["Hello", "programme.", "really", "truly", "spares", "indeed"]);
    // Matched words keep the recogniser's times.
    for text in ["welcome", "back", "We", "text,", "time"] {
        let plain = |t: &str| t.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        let reference_word = r.words.iter().find(|w| plain(&w.text) == plain(text)).unwrap();
        assert_eq!((get(text).start, get(text).end), (reference_word.start, reference_word.end), "{text}");
    }
    // A changed word takes over the stretch of what was heard in its place ("show. Today").
    assert_eq!(get("programme.").start, rw("show.").start);
    assert_eq!(get("programme.").end, rw("Today").end);
    // Inserted + changed words share the time between their matched neighbours ("it" .. "time").
    let (it, time) = (get("it"), get("time"));
    for text in ["really", "truly", "spares"] {
        let w = get(text);
        assert!(w.start >= it.end && w.end <= time.start, "{text} must lie between its neighbours: {w:?}");
    }
    assert!(get("really").end <= get("truly").start && get("truly").end <= get("spares").start);
    assert!(get("spares").end > get("really").start, "the three words must share real time, not collapse");
    // Before the first and after the last matched word: next to the neighbour, at a
    // plausible length, not stretched over the remaining 50 seconds.
    assert_eq!(get("Hello").end, get("welcome").start);
    assert!(get("Hello").start >= 0.0 && get("Hello").end - get("Hello").start > 0.1);
    assert_eq!(get("indeed").start, get("time").end);
    let indeed = get("indeed").end - get("indeed").start;
    assert!(indeed > 0.1 && indeed < 2.0, "inserted trailing word lasts {indeed} s");
}

#[test]
fn speaker_lines_name_the_following_words() {
    let r = reference("Hi Ben how are you I am fine thanks and you");
    let user = "Ana: Hi Ben, how are you?\nBen: I am fine, thanks.\nAnd you?\nDr. Mary-Jane O'Neil (host):\nGreat. Note: this stays text. It is 10:30 at http://example.com";
    let out = align_text(&r, user, 30.0);
    let by = |name: &str| out.iter().filter(|w| w.speaker.as_deref() == Some(name)).map(|w| w.text.as_str()).collect::<Vec<_>>();
    assert_eq!(by("Ana"), ["Hi", "Ben,", "how", "are", "you?"]);
    assert_eq!(by("Ben"), ["I", "am", "fine,", "thanks.", "And", "you?"]);
    assert_eq!(by("Dr. Mary-Jane O'Neil (host)"), ["Great.", "Note:", "this", "stays", "text.", "It", "is", "10:30", "at", "http://example.com"]);
    assert_eq!(speaker_names(&out), ["Ana", "Ben", "Dr. Mary-Jane O'Neil (host)"]);
    // Speaker labels are not words; times and URLs are not speakers.
    assert!(out.iter().all(|w| !["Ana:", "Ben:"].contains(&w.text.as_str())));
    assert_eq!(out.iter().filter(|w| w.matched).count(), 11);
    assert_monotonic(&out, 30.0);
}

#[test]
fn text_without_speaker_lines_has_no_speakers() {
    let out = align_text(&reference(SPOKEN), SPOKEN, 60.0);
    assert!(speaker_names(&out).is_empty());
}

#[test]
fn empty_text_gives_nothing() {
    let r = reference(SPOKEN);
    assert!(align_text(&r, "", 60.0).is_empty());
    assert!(align_text(&r, "  \n\t\n", 60.0).is_empty());
    assert!(align_text(&r, "Ana:\nBen:   ", 60.0).is_empty());
    assert!(align_text(&r, "... -- !", 60.0).is_empty());
}

#[test]
fn empty_reference_spreads_the_words_over_the_duration() {
    let out = align_text(&Transcript::default(), "one two three four", 8.0);
    assert_eq!(out.len(), 4);
    assert!(out.iter().all(|w| !w.matched));
    assert_monotonic(&out, 8.0);
    assert_eq!(out[0].start, 0.0);
    assert!((out[3].end - 8.0).abs() < 1e-9);
    assert!(out.iter().all(|w| w.end > w.start));
}

#[test]
fn completely_different_text_still_returns_all_words_in_order() {
    let r = reference(SPOKEN);
    let out = align_text(&r, "Lorem ipsum dolor sit amet", 60.0);
    assert_eq!(out.len(), 5);
    assert!(out.iter().all(|w| !w.matched));
    assert_monotonic(&out, 60.0);
}

#[test]
fn times_are_clamped_to_the_duration_and_bad_input_does_not_panic() {
    let r = reference(SPOKEN); // runs to about 8 s
    let out = align_text(&r, SPOKEN, 4.0);
    assert_monotonic(&out, 4.0);
    assert_eq!(out.len(), r.words.len());
    for d in [0.0, -5.0, f64::NAN, f64::INFINITY] {
        let out = align_text(&r, SPOKEN, d);
        assert_eq!(out.len(), r.words.len());
        assert!(out.iter().all(|w| w.start.is_finite() && w.end.is_finite() && w.end >= w.start));
    }
}

#[test]
fn repeated_phrases_align_in_order() {
    let r = reference("no no no yes no no");
    let out = align_text(&r, "no no yes no", 10.0);
    assert!(out.iter().all(|w| w.matched));
    assert_monotonic(&out, 10.0);
    let yes = out.iter().find(|w| w.text == "yes").unwrap();
    assert_eq!(yes.start, r.words[3].start);
}

#[test]
fn long_transcripts_align_quickly_and_correctly() {
    // An hour of speech: 9000 words; the user's version drops and changes words throughout.
    let vocabulary = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet", "kilo"];
    let spoken: Vec<String> = (0..9000).map(|i| format!("{}{}", vocabulary[(i * 7 + i / 11) % vocabulary.len()], i % 13)).collect();
    let r = reference(&spoken.join(" "));
    let user: Vec<String> = spoken
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 97 != 5)
        .map(|(i, w)| if i % 53 == 7 { "changed".to_string() } else { w.clone() })
        .collect();
    let started = std::time::Instant::now();
    let out = align_text(&r, &user.join(" "), 4000.0);
    assert!(started.elapsed().as_secs_f64() < 5.0, "alignment took {:?}", started.elapsed());
    assert_eq!(out.len(), user.len());
    assert_monotonic(&out, 4000.0);
    let matched = out.iter().filter(|w| w.matched).count();
    let expected = user.iter().filter(|w| *w != "changed").count();
    assert_eq!(matched, expected, "every unchanged word should find its partner");
    assert_eq!(out.last().unwrap().end, r.words.last().unwrap().end);
}
