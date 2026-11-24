//! Detection passes over a script: filler words, repeated words, retakes.
//! These only *find* things; the caller decides whether to delete or ignore.

use crate::model::*;
use crate::script::normalize_word;
use std::ops::Range;

/// Single-word fillers that are almost never meaningful.
pub const FILLERS_STRICT: &[&str] = &["um", "uh", "umm", "uhh", "uhm", "er", "erm", "ah", "eh", "hmm", "mm", "mhm", "uh-huh"];
/// Words that are fillers only in some positions.
pub const FILLERS_LOOSE: &[&str] = &["like", "basically", "actually", "literally", "so", "right", "okay", "well"];
/// Multi-word fillers.
pub const FILLER_PHRASES: &[&[&str]] = &[&["you", "know"], &["i", "mean"], &["sort", "of"], &["kind", "of"], &["you", "see"]];

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FillerKind {
    /// "um", "uh" …
    Filler,
    /// "you know", "I mean" …
    Phrase,
    /// "like", "basically" … (only reported when asked for).
    Loose,
    /// Immediate repetition: "the the".
    Repeat,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FillerHit {
    pub kind: FillerKind,
    pub tokens: Range<usize>,
    pub text: String,
    /// Composition time of the hit.
    pub time: Secs,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FillerOptions {
    pub phrases: bool,
    pub loose: bool,
    pub repeats: bool,
    /// Extra words to treat as fillers.
    #[serde(default)]
    pub custom: Vec<String>,
}

impl Default for FillerOptions {
    fn default() -> Self {
        Self { phrases: true, loose: false, repeats: true, custom: Vec::new() }
    }
}

/// Indices of playing word tokens with their normalised text.
fn playing_words(comp: &Composition) -> Vec<(usize, String)> {
    comp.script
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TokenKind::Word && !t.ignored && !t.hidden_text)
        .map(|(i, t)| (i, normalize_word(&t.text)))
        .collect()
}

pub fn find_fillers(comp: &Composition, opts: &FillerOptions) -> Vec<FillerHit> {
    let words = playing_words(comp);
    let starts = comp.token_starts();
    let custom: Vec<String> = opts.custom.iter().map(|w| normalize_word(w)).filter(|w| !w.is_empty()).collect();
    let mut hits: Vec<FillerHit> = Vec::new();
    let mut k = 0;
    while k < words.len() {
        let (idx, w) = (&words[k].0, &words[k].1);
        let push = |kind: FillerKind, span: usize, hits: &mut Vec<FillerHit>| {
            let last = words[k + span - 1].0;
            let text = (k..k + span).map(|j| comp.script[words[j].0].text.clone()).collect::<Vec<_>>().join(" ");
            hits.push(FillerHit { kind, tokens: *idx..last + 1, text, time: starts[*idx] });
        };
        if FILLERS_STRICT.contains(&w.as_str()) || custom.contains(w) {
            push(FillerKind::Filler, 1, &mut hits);
            k += 1;
            continue;
        }
        if opts.phrases {
            if let Some(p) = FILLER_PHRASES.iter().find(|p| k + p.len() <= words.len() && p.iter().enumerate().all(|(j, pw)| words[k + j].1 == *pw)) {
                // "kind of" / "sort of" followed by a noun phrase is real speech
                // ("a kind of bird"); only flag when not preceded by an article.
                let preceded_by_article = k > 0 && matches!(words[k - 1].1.as_str(), "a" | "the" | "what" | "that" | "this" | "some" | "any" | "which");
                if !(matches!(p[0], "kind" | "sort") && preceded_by_article) {
                    push(FillerKind::Phrase, p.len(), &mut hits);
                    k += p.len();
                    continue;
                }
            }
        }
        if opts.repeats && k + 1 < words.len() && !w.is_empty() && *w == words[k + 1].1 && !is_legit_double(w) {
            // Report the first of the pair as the one to drop.
            push(FillerKind::Repeat, 1, &mut hits);
            k += 1;
            continue;
        }
        if opts.loose && FILLERS_LOOSE.contains(&w.as_str()) {
            // Only at a clause edge: sentence start, or set off by commas.
            let tok = &comp.script[*idx];
            let prev_ended = k == 0 || {
                let p = &comp.script[words[k - 1].0].text;
                p.ends_with([',', '.', '?', '!', ';', ':'])
            };
            if prev_ended || tok.text.ends_with(',') {
                push(FillerKind::Loose, 1, &mut hits);
            }
        }
        k += 1;
    }
    hits
}

fn is_legit_double(w: &str) -> bool {
    matches!(w, "that" | "had" | "very" | "really" | "so" | "no" | "bye" | "ha" | "yeah" | "is" | "do" | "many" | "far" | "long")
}

/// A stretch that was said and then said again (a false start or retake).
#[derive(Clone, Debug, PartialEq)]
pub struct RetakeHit {
    /// The earlier take, which would normally be removed.
    pub earlier: Range<usize>,
    /// The later take that replaces it.
    pub later: Range<usize>,
    pub text: String,
    pub time: Secs,
}

/// Finds retakes: a run of at least `min_words` words that is repeated within
/// the following `window` words. Greedy, non-overlapping, longest match first.
pub fn find_retakes(comp: &Composition, min_words: usize, window: usize) -> Vec<RetakeHit> {
    let words = playing_words(comp);
    let starts = comp.token_starts();
    let min_words = min_words.max(2);
    let n = words.len();
    let mut hits = Vec::new();
    let mut i = 0;
    while i + min_words < n {
        let mut best: Option<(usize, usize)> = None; // (start of later take, length)
        let limit = (i + window + min_words).min(n);
        let mut j = i + 1;
        while j + min_words <= limit {
            if words[j].1 == words[i].1 && !words[i].1.is_empty() {
                let mut len = 0;
                // The earlier take must end before the later one starts.
                while j + len < n && i + len < j && words[i + len].1 == words[j + len].1 {
                    len += 1;
                }
                if len >= min_words && best.is_none_or(|(_, bl)| len > bl) {
                    best = Some((j, len));
                }
            }
            j += 1;
        }
        if let Some((j, len)) = best {
            // Everything from the first take up to the start of the second is
            // the abandoned attempt.
            let earlier = words[i].0..words[j].0;
            let later = words[j].0..words[j + len - 1].0 + 1;
            let text = comp.text_of(earlier.clone());
            hits.push(RetakeHit { time: starts[earlier.start], earlier, later, text });
            i = j;
        } else {
            i += 1;
        }
    }
    hits
}

/// Runs of silence (gap tokens) at least `min` seconds long, as token ranges
/// with their total duration.
pub fn find_gaps(comp: &Composition, min: Secs) -> Vec<(Range<usize>, Secs)> {
    let mut out = Vec::new();
    let mut i = 0;
    let s = &comp.script;
    while i < s.len() {
        if s[i].ignored || s[i].kind == TokenKind::Word {
            i += 1;
            continue;
        }
        let start = i;
        let mut total = 0.0;
        while i < s.len() && !s[i].ignored && s[i].kind != TokenKind::Word {
            total += s[i].duration();
            i += 1;
        }
        if total >= min {
            out.push((start..i, total));
        }
    }
    out
}

/// Candidate clip: a stretch of whole sentences between `min` and `max`
/// seconds long. Used as raw material for "create clips" before ranking.
pub fn sentence_windows(comp: &Composition, min: Secs, max: Secs) -> Vec<Range<usize>> {
    // Sentence boundaries: index after each sentence-ending word.
    let mut bounds = vec![0usize];
    for (i, t) in comp.script.iter().enumerate() {
        if t.shows_text() && crate::script::ends_sentence(&t.text) {
            bounds.push(i + 1);
        }
    }
    if bounds.last() != Some(&comp.script.len()) {
        bounds.push(comp.script.len());
    }
    let starts = comp.token_starts();
    let mut out = Vec::new();
    for a in 0..bounds.len() {
        for b in a + 1..bounds.len() {
            let d = starts[bounds[b]] - starts[bounds[a]];
            if d > max {
                break;
            }
            if d >= min {
                out.push(bounds[a]..bounds[b]);
            }
        }
    }
    out
}
