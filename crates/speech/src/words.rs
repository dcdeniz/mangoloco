//! Turning whisper's sub-word tokens into words with times.

use crate::transcribe::RawSegment;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub confidence: f32,
}

fn has_alnum(s: &str) -> bool {
    s.chars().any(char::is_alphanumeric)
}

/// Scripts written without spaces between words: every token is its own unit.
fn is_spaceless_script(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // hiragana, katakana
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF // CJK ideographs
        | 0x0E00..=0x0E7F // Thai
        | 0x20000..=0x2FA1F)
}

fn is_opening_punct(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| matches!(c, '¿' | '¡' | '"' | '“' | '‘' | '«' | '(' | '[' | '\'' | '„' | '「' | '『'))
}

fn is_music(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '♪' | '♫' | '♬' | '♩'))
}

/// A word as the decoder produced it, before its boundaries are refined against the audio.
#[derive(Clone, Debug)]
pub(crate) struct Draft {
    pub text: String,
    pub segment: usize,
    /// Cross-attention (DTW) time of the word's first token, when the model provides it.
    pub dtw: Option<f64>,
    /// whisper's own timestamp prediction for the start of this word and its probability.
    pub ts: Option<(f64, f32)>,
    /// whisper.cpp's heuristic token times.
    pub t0: f64,
    pub t1: f64,
    pub confidence: f32,
}

struct Building {
    bytes: Vec<u8>,
    dtw: Option<f64>,
    ts: Option<(f64, f32)>,
    t0: f64,
    t1: f64,
    logp_sum: f64,
    n: usize,
}

/// Merges sub-word tokens into words, attaches punctuation to the neighbouring word and
/// removes non-speech annotations such as `[BLANK_AUDIO]`, `(music)`, `*laughs*` and `♪`.
pub(crate) fn draft_words(segments: &[RawSegment]) -> Vec<Draft> {
    let mut out: Vec<Draft> = Vec::new();
    let mut prefix = String::new();
    for (seg_index, seg) in segments.iter().enumerate() {
        // 1. tokens -> raw words
        let mut raw: Vec<Building> = Vec::new();
        for tok in &seg.tokens {
            if tok.bytes.is_empty() {
                continue;
            }
            let starts_word = match raw.last() {
                None => true,
                Some(cur) => {
                    tok.bytes[0] == b' ' || {
                        // Spaceless scripts: split where both sides are complete characters of such a script.
                        let cur_ok = std::str::from_utf8(&cur.bytes).ok().and_then(|s| s.chars().last()).is_some_and(is_spaceless_script);
                        let tok_ok = std::str::from_utf8(&tok.bytes).ok().and_then(|s| s.chars().next()).is_some_and(is_spaceless_script);
                        cur_ok && tok_ok
                    }
                }
            };
            if starts_word {
                raw.push(Building { bytes: Vec::new(), dtw: tok.t_dtw, ts: tok.ts, t0: tok.t0, t1: tok.t1, logp_sum: 0.0, n: 0 });
            }
            let cur = raw.last_mut().expect("pushed above");
            cur.bytes.extend_from_slice(&tok.bytes);
            let piece = String::from_utf8_lossy(&tok.bytes);
            if has_alnum(&piece) || cur.n == 0 {
                // Trailing punctuation tokens must not stretch the word over the pause that follows.
                cur.t1 = tok.t1.max(cur.t0);
                cur.logp_sum += (tok.p.clamp(1e-6, 1.0) as f64).ln();
                cur.n += 1;
            }
        }

        // 2. raw words -> words, dropping annotations
        let mut skip_until: Option<char> = None;
        for b in raw {
            let text = String::from_utf8_lossy(&b.bytes).replace('\u{FFFD}', "");
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if let Some(close) = skip_until {
                if text.contains(close) {
                    skip_until = None;
                }
                continue;
            }
            let first = text.chars().next().unwrap_or(' ');
            let annotation_close = match first {
                '[' => Some(']'),
                '(' => Some(')'),
                '*' => Some('*'),
                _ => None,
            };
            if let Some(close) = annotation_close {
                let rest = &text[first.len_utf8()..];
                if !rest.contains(close) {
                    skip_until = Some(close);
                }
                continue;
            }
            if is_music(text) {
                continue;
            }
            if !has_alnum(text) {
                if is_opening_punct(text) {
                    prefix.push_str(text);
                } else if let Some(prev) = out.last_mut() {
                    prev.text.push_str(text);
                }
                continue;
            }
            let confidence = if b.n > 0 { (b.logp_sum / b.n as f64).exp() as f32 } else { 0.0 };
            let mut full = std::mem::take(&mut prefix);
            full.push_str(text);
            out.push(Draft {
                text: full,
                segment: seg_index,
                dtw: b.dtw,
                ts: b.ts,
                t0: b.t0,
                t1: b.t1,
                confidence: confidence.clamp(0.0, 1.0),
            });
        }
        prefix.clear();
    }
    out
}

// ---------------------------------------------------------------------------
// Speech activity from signal energy
// ---------------------------------------------------------------------------

const HOP: usize = 160; // 10 ms at 16 kHz
const WIN: usize = 320;
const HOP_SECS: f64 = HOP as f64 / crate::transcribe::SAMPLE_RATE as f64;
/// Pauses shorter than this are part of speech (stop closures, short hesitations).
const MIN_SILENCE: f64 = 0.18;
const MIN_SPEECH: f64 = 0.03;

/// Where the audio is audibly active.
pub(crate) struct Activity {
    /// Per-frame level in dB (after pre-emphasis), 10 ms apart.
    db: Vec<f32>,
    /// `None`: the signal has no usable contrast between quiet and loud (constant noise or
    /// music), so nothing can be said. `Some(regions)`: sorted, non-overlapping active
    /// stretches in seconds; empty means the audio is silent.
    pub regions: Option<Vec<(f64, f64)>>,
}

pub(crate) fn analyse(samples: &[f32]) -> Activity {
    let duration = samples.len() as f64 / crate::transcribe::SAMPLE_RATE as f64;
    let n_frames = samples.len() / HOP;
    let mut db = Vec::with_capacity(n_frames);
    let mut raw_db: Vec<f32> = Vec::with_capacity(n_frames);
    let mut raw_peak = 0.0f32;
    for f in 0..n_frames {
        let from = f * HOP;
        let to = (from + WIN).min(samples.len());
        let mut energy = 0.0f64;
        let mut raw_energy = 0.0f64;
        for i in from..to {
            raw_energy += (samples[i] as f64) * (samples[i] as f64);
            raw_peak = raw_peak.max(samples[i].abs());
            // Pre-emphasis: keeps weak consonants visible, removes hum and DC.
            let prev = if i > 0 { samples[i - 1] } else { 0.0 };
            let v = (samples[i] - 0.95 * prev) as f64;
            energy += v * v;
        }
        let mean = energy / (to - from).max(1) as f64;
        db.push((10.0 * (mean + 1e-12).log10()) as f32);
        raw_db.push((10.0 * (raw_energy / (to - from).max(1) as f64 + 1e-12).log10()) as f32);
    }
    // Inaudible throughout (below -60 dBFS peak): silent.
    if db.is_empty() || raw_peak < 0.001 {
        return Activity { db, regions: Some(Vec::new()) };
    }
    let mut sorted = db.clone();
    sorted.sort_by(f32::total_cmp);
    let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p).round() as usize];
    let floor = pct(0.10);
    let peak = pct(0.97).max(pct(1.0) - 25.0);
    if peak - floor < 10.0 {
        // No contrast between loud and quiet. A steady quiet signal is room tone or hiss with
        // nobody speaking; a steady loud one (music, heavy noise) is left for whisper to judge.
        raw_db.sort_by(f32::total_cmp);
        let level = raw_db[((raw_db.len() - 1) as f64 * 0.97).round() as usize];
        let regions = if level < -40.0 { Some(Vec::new()) } else { None };
        return Activity { db, regions };
    }
    let threshold = (floor + 12.0).clamp(peak - 45.0, peak - 18.0);

    let mut runs: Vec<(usize, usize)> = Vec::new(); // frame ranges, end exclusive
    let mut start: Option<usize> = None;
    for (i, &level) in db.iter().enumerate() {
        match (level > threshold, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                runs.push((s, i));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push((s, db.len()));
    }
    // Bridge short pauses, then drop clicks.
    let min_gap = (MIN_SILENCE / HOP_SECS).round() as usize;
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for r in runs {
        match merged.last_mut() {
            Some(last) if r.0 - last.1 < min_gap => last.1 = r.1,
            _ => merged.push(r),
        }
    }
    let min_len = (MIN_SPEECH / HOP_SECS).round() as usize;
    let regions = merged
        .into_iter()
        .filter(|r| r.1 - r.0 >= min_len)
        .map(|(a, b)| {
            // A frame spans 20 ms; open slightly early and close slightly late so that a
            // cut at the boundary never clips the onset or the tail.
            let start = (a as f64 * HOP_SECS - 0.01).max(0.0);
            let end = ((b - 1) as f64 * HOP_SECS + WIN as f64 / crate::transcribe::SAMPLE_RATE as f64 + 0.01).min(duration);
            (start, end)
        })
        .collect();
    Activity { db, regions: Some(regions) }
}

impl Activity {
    /// The quietest instant within `radius` seconds of `t`: where a cut is least audible
    /// and where a word boundary is most likely to be.
    fn quietest_near(&self, t: f64, radius: f64, lo: f64, hi: f64) -> f64 {
        if self.db.is_empty() || hi <= lo {
            return t.clamp(lo.min(hi), hi.max(lo));
        }
        let frame = |x: f64| ((x / HOP_SECS).floor().max(0.0) as usize).min(self.db.len() - 1);
        let from = frame((t - radius).max(lo));
        let to = frame((t + radius).min(hi));
        let centre = frame(t);
        let mut best = centre.clamp(from, to.max(from));
        let mut best_score = f32::INFINITY;
        for i in from..=to.max(from) {
            // Mild preference for staying close to the estimate.
            let score = self.db[i] + 20.0 * ((i as f64 - centre as f64).abs() * HOP_SECS / radius.max(1e-6)) as f32;
            if score < best_score {
                best_score = score;
                best = i;
            }
        }
        ((best as f64 + 1.0) * HOP_SECS).clamp(lo, hi)
    }
}

// ---------------------------------------------------------------------------
// Word timing
// ---------------------------------------------------------------------------

/// How timestamps are derived; the non-default choices exist to measure the alternatives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    /// whisper.cpp's heuristic token timestamps as they come.
    TokenHeuristic,
    /// Cross-attention DTW token times as they come (word start = its first token's time,
    /// word end = the next word's start).
    DtwRaw,
    /// Every word is placed inside the audible stretch it was spoken in: the first word of a
    /// stretch starts where the sound starts, the last ends where it ends, and the boundaries
    /// in between are a least-squares fit to the DTW times, whisper's own timestamp
    /// predictions and word lengths, with a minimum duration per word.
    /// This is what [`crate::transcribe::transcribe`] uses.
    DtwSnapped,
}

/// Typical distance between a word's onset and its DTW time. Only used when the signal has
/// no usable pauses at all (music bed, heavy noise) and to tell the model families apart.
pub(crate) const DEFAULT_LAG: f64 = 0.20;
/// The tiny models attend to the end of a word rather than its middle (measured: about 0.5 s).
pub(crate) const TINY_LAG: f64 = 0.45;
/// Where inside a word the DTW time falls, as a fraction of the word (measured on words
/// with known boundaries: 0.5 for base.en, 1.0 for tiny.en).
pub(crate) const DEFAULT_FRACTION: f64 = 0.5;
pub(crate) const TINY_FRACTION: f64 = 1.0;

/// Longest plausible duration of a spoken word, used so that a word never stretches across
/// a pause that the signal analysis could not see.
fn max_word_duration(text: &str) -> f64 {
    0.30 + 0.09 * text.chars().filter(|c| c.is_alphanumeric()).count() as f64
}

/// Index of the region containing `t`, otherwise the nearest one.
fn nearest_region(regions: &[(f64, f64)], t: f64) -> (usize, f64) {
    let mut best = (0usize, f64::INFINITY);
    for (k, &(a, b)) in regions.iter().enumerate() {
        let d = if t < a { a - t } else if t > b { t - b } else { 0.0 };
        if d < best.1 {
            best = (k, d);
        }
    }
    best
}

pub(crate) fn time_words(drafts: Vec<Draft>, activity: &Activity, timing: Timing, duration: f64, default_lag: f64) -> Vec<Word> {
    if drafts.is_empty() {
        return Vec::new();
    }
    let all_dtw = drafts.iter().all(|d| d.dtw.is_some());
    if timing == Timing::TokenHeuristic || !all_dtw {
        let words = drafts
            .into_iter()
            .map(|d| Word { text: d.text, start: d.t0, end: d.t1, confidence: d.confidence })
            .collect();
        return make_monotonic(words, duration);
    }

    // DTW times, forced non-decreasing. A raw DTW time lies *inside* its word (around the
    // middle for most models, near the end for the tiny ones), never before its onset.
    let mut raw: Vec<f64> = Vec::with_capacity(drafts.len());
    let mut cursor = 0.0f64;
    for d in &drafts {
        cursor = cursor.max(d.dtw.unwrap_or(cursor));
        raw.push(cursor);
    }
    if timing == Timing::DtwRaw {
        let n = raw.len();
        let words = drafts
            .into_iter()
            .enumerate()
            .map(|(i, d)| {
                let limit = raw[i] + max_word_duration(&d.text);
                let end = if i + 1 < n { raw[i + 1].min(limit) } else { limit };
                Word { text: d.text, start: raw[i], end, confidence: d.confidence }
            })
            .collect();
        return make_monotonic(words, duration);
    }

    let words = match activity.regions.as_deref() {
        Some(regions) if !regions.is_empty() => {
            let fraction = if default_lag >= TINY_LAG { TINY_FRACTION } else { DEFAULT_FRACTION };
            snap_to_stretches(drafts, raw, regions, activity, fraction)
        }
        // No usable pauses in the signal (music bed, heavy noise): lag-corrected DTW only.
        _ => {
            let n = drafts.len();
            let mins: Vec<f64> = drafts.iter().map(|d| min_word_duration(&d.text)).collect();
            let desired: Vec<f64> = raw.iter().map(|t| (t - default_lag).max(0.0)).collect();
            let last_end = (desired[n - 1] + max_word_duration(&drafts[n - 1].text)).min(duration.max(desired[0]));
            let starts = fit_boundaries(desired[0], last_end, &desired, &mins);
            drafts
                .into_iter()
                .enumerate()
                .map(|(i, d)| {
                    let limit = starts[i] + max_word_duration(&d.text);
                    let end = if i + 1 < n { starts[i + 1].min(limit) } else { last_end.min(limit).max(starts[i]) };
                    Word { text: d.text, start: starts[i], end, confidence: d.confidence }
                })
                .collect()
        }
    };
    make_monotonic(words, duration)
}

/// Shortest plausible duration of a spoken word: 35 ms per letter, at least 60 ms, and no
/// more than 200 ms is insisted on. No word is ever given less than this unless the audible
/// stretch it belongs to is itself too short for its words.
pub(crate) fn min_word_duration(text: &str) -> f64 {
    (0.035 * text.chars().filter(|c| c.is_alphanumeric()).count() as f64).clamp(0.06, 0.20)
}

/// Places `desired.len()` consecutive words in `from..to`: word `i` starts as close to
/// `desired[i]` as possible while every word keeps at least `mins[i]`. The first word starts
/// at `from`. When the span is too short for all the minimums, it is divided in proportion
/// to them, so no word is left as a sliver.
fn fit_boundaries(from: f64, to: f64, desired: &[f64], mins: &[f64]) -> Vec<f64> {
    let n = desired.len();
    let span = (to - from).max(0.0);
    let need: f64 = mins.iter().sum();
    let scale = if need > span && need > 0.0 { span / need } else { 1.0 };
    let mut starts = vec![from; n];
    for i in 1..n {
        starts[i] = desired[i].max(starts[i - 1] + mins[i - 1] * scale);
    }
    let mut limit = from + span;
    for i in (1..n).rev() {
        limit -= mins[i] * scale;
        starts[i] = starts[i].min(limit).max(from);
        limit = starts[i];
    }
    starts
}

/// whisper's timestamp prediction for a word start is used when it gave it at least this
/// probability; below that the predictions are mostly round numbers and noise.
const ANCHOR_MIN_PROBABILITY: f32 = 0.05;
/// Weight of the words-take-time-in-proportion-to-their-length prior against one DTW time.
const LENGTH_PRIOR: f64 = 0.3;

/// Consecutive words `first..end` filling the time `from..to`.
struct Group {
    first: usize,
    end: usize,
    from: f64,
    to: f64,
}

/// How much speaking time a word takes relative to others: its letters, with a floor so
/// that "a" and "I" still get their share.
fn speech_weight(text: &str) -> f64 {
    (text.chars().filter(|c| c.is_alphanumeric()).count() as f64).max(1.5) + 1.0
}

/// Least-squares word starts for one group. The unknowns are the starts of words 2..n; the
/// group's first word starts at `from` and its last ends at `to`. Three kinds of evidence:
/// - DTW: `raw[i]` lies at `fraction` of the way through word i;
/// - anchors: whisper's own timestamp for a word start;
/// - a weak prior that words take time in proportion to their length, which keeps the
///   solution from zig-zagging on noisy DTW times.
/// Returns the starts of all words in the group and the DTW misfit (for choosing `fraction`).
fn solve_group(g: &Group, raw: &[f64], anchors: &[Option<f64>], weights: &[f64], fraction: f64, prior: f64) -> (Vec<f64>, f64) {
    let n = g.end - g.first;
    let mut starts = vec![g.from; n];
    let position = |starts: &[f64], k: usize| if k < n { starts[k] } else { g.to };
    if n > 1 {
        let m = n - 1; // unknown k (0-based) is the start of word k + 1
        let (mut diag, mut upper, mut rhs) = (vec![0.0f64; m], vec![0.0f64; m], vec![0.0f64; m]);
        // Adds weight * (alpha * s[k] + beta * s[k + 1] - gamma)^2, where s[0] and s[n] are fixed.
        let mut add = |k: usize, alpha: f64, beta: f64, gamma: f64, weight: f64| {
            let mut gamma = gamma;
            let left = if k == 0 {
                gamma -= alpha * g.from;
                None
            } else {
                Some(k - 1)
            };
            let right = if k + 1 == n {
                gamma -= beta * g.to;
                None
            } else {
                Some(k)
            };
            if let Some(l) = left {
                diag[l] += weight * alpha * alpha;
                rhs[l] += weight * alpha * gamma;
            }
            if let Some(r) = right {
                diag[r] += weight * beta * beta;
                rhs[r] += weight * beta * gamma;
            }
            if let (Some(l), Some(_)) = (left, right) {
                upper[l] += weight * alpha * beta;
            }
        };
        let total_weight: f64 = weights[g.first..g.end].iter().sum();
        let rate = (g.to - g.from) / total_weight;
        for k in 0..n {
            let w = g.first + k;
            add(k, 1.0 - fraction, fraction, raw[w], 1.0);
            add(k, -1.0, 1.0, rate * weights[w], prior);
            if k > 0 {
                if let Some(t) = anchors[w].filter(|t| *t > g.from && *t < g.to) {
                    add(k - 1, 0.0, 1.0, t, 1.0);
                }
            }
        }
        // Thomas algorithm for the symmetric tridiagonal system.
        let mut c = vec![0.0f64; m];
        let mut d = vec![0.0f64; m];
        for k in 0..m {
            let lower = if k > 0 { upper[k - 1] } else { 0.0 };
            let denom = diag[k] - if k > 0 { lower * c[k - 1] } else { 0.0 };
            let denom = if denom.abs() < 1e-12 { 1e-12 } else { denom };
            c[k] = upper[k] / denom;
            d[k] = (rhs[k] - if k > 0 { lower * d[k - 1] } else { 0.0 }) / denom;
        }
        for k in (0..m).rev() {
            starts[k + 1] = d[k] - if k + 1 < m { c[k] * starts[k + 2] } else { 0.0 };
        }
    }
    let misfit = (0..n)
        .map(|k| {
            let r = (1.0 - fraction) * position(&starts, k) + fraction * position(&starts, k + 1) - raw[g.first + k];
            r * r
        })
        .sum();
    (starts, misfit)
}

/// The timing step proper: every word is placed inside the audible stretch it was spoken in.
fn snap_to_stretches(drafts: Vec<Draft>, raw: Vec<f64>, regions: &[(f64, f64)], activity: &Activity, default_fraction: f64) -> Vec<Word> {
    // 1. Which stretch each word belongs to. The raw DTW time is inside the word, so the
    //    stretch that contains it (or the nearest one) is the word's stretch. The
    //    lag-corrected time must not be used here: it can fall into the pause before the
    //    word, and the word would then be attached to the end of the previous stretch.
    let mut region_of = Vec::with_capacity(raw.len());
    let mut distance = Vec::with_capacity(raw.len());
    let mut last = 0usize;
    for &t in &raw {
        let (k, d) = nearest_region(regions, t);
        last = last.max(k);
        region_of.push(last);
        distance.push(d);
    }

    // 2. A whole segment "heard" where the signal is silent is a hallucination.
    let mut keep = vec![true; drafts.len()];
    let mut i = 0;
    while i < drafts.len() {
        let mut j = i;
        while j < drafts.len() && drafts[j].segment == drafts[i].segment {
            j += 1;
        }
        if (i..j).all(|w| distance[w] > 0.4) {
            keep[i..j].iter_mut().for_each(|k| *k = false);
        }
        i = j;
    }
    let mut kept = keep.iter();
    let drafts: Vec<Draft> = drafts.into_iter().filter(|_| *kept.next().unwrap_or(&true)).collect();
    let pick = |v: Vec<f64>| -> Vec<f64> { v.into_iter().zip(&keep).filter(|(_, k)| **k).map(|(x, _)| x).collect() };
    let raw = pick(raw);
    let mut region_of: Vec<usize> = region_of.into_iter().zip(&keep).filter(|(_, k)| **k).map(|(x, _)| x).collect();
    let n = drafts.len();
    if n == 0 {
        return Vec::new();
    }
    let mins: Vec<f64> = drafts.iter().map(|d| min_word_duration(&d.text)).collect();

    // 3. A stretch cannot hold more words than fit in it. If one is over-full while its
    //    neighbour has room, the DTW time of a word at the edge was off: move it across.
    let length = |k: usize| regions[k].1 - regions[k].0;
    let mut load = vec![0.0f64; regions.len()];
    for w in 0..n {
        load[region_of[w]] += mins[w];
    }
    for k in 0..regions.len().saturating_sub(1) {
        while load[k] > length(k) {
            let Some(w) = (0..n).rev().find(|&w| region_of[w] == k) else { break };
            if load[k + 1] + mins[w] > length(k + 1) {
                break;
            }
            region_of[w] = k + 1;
            load[k] -= mins[w];
            load[k + 1] += mins[w];
        }
    }
    for k in (1..regions.len()).rev() {
        while load[k] > length(k) {
            let Some(w) = (0..n).find(|&w| region_of[w] == k) else { break };
            if load[k - 1] + mins[w] > length(k - 1) {
                break;
            }
            region_of[w] = k - 1;
            load[k] -= mins[w];
            load[k - 1] += mins[w];
        }
    }

    // 4. The stretches with their words, and what is known about each word.
    let weights: Vec<f64> = drafts.iter().map(|d| speech_weight(&d.text)).collect();
    let mut groups: Vec<Group> = Vec::new();
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j < n && region_of[j] == region_of[i] {
            j += 1;
        }
        let (a, b) = regions[region_of[i]];
        // The first word begins where the sound begins, and the last one ends where it ends,
        // unless the stretch is much longer than its words (noise before or after the speech).
        let from = if raw[i] - a <= 0.8 { a } else { (raw[i] - 0.2).max(a) };
        let last = j - 1;
        let plausible = max_word_duration(&drafts[last].text);
        let to = if b - raw[last] <= plausible + 0.3 { b } else { (raw[last] + plausible).min(b) };
        let need: f64 = mins[i..j].iter().sum();
        let to = to.max((from + need).min(b));
        groups.push(Group { first: i, end: j, from, to });
        i = j;
    }
    // whisper's own timestamp prediction for a word start, when it gave one with a usable
    // probability, is a second witness next to DTW.
    let anchors: Vec<Option<f64>> = drafts.iter().map(|d| d.ts.filter(|&(_, pt)| pt >= ANCHOR_MIN_PROBABILITY).map(|(t, _)| t)).collect();

    // 5. Where inside a word the DTW time falls is a property of the model (measured: about
    //    the middle for base, the end for tiny). Use the value that explains this file best
    //    when the file has enough stretches to tell, the model's usual value otherwise.
    let prior = LENGTH_PRIOR;
    let mut fraction = default_fraction;
    if groups.len() >= 4 {
        let mut best = f64::INFINITY;
        for step in 0..=20 {
            let f = step as f64 * 0.05;
            let cost: f64 = groups.iter().map(|g| solve_group(g, &raw, &anchors, &weights, f, prior).1).sum();
            if cost < best {
                best = cost;
                fraction = f;
            }
        }
    }
    // 6. Boundaries inside each stretch.
    let mut starts = vec![0.0f64; n];
    let mut ends = vec![0.0f64; n];
    for g in &groups {
        let (solution, _) = solve_group(g, &raw, &anchors, &weights, fraction, prior);
        let mut desired = vec![g.from; g.end - g.first];
        for w in g.first + 1..g.end {
            // The quietest instant near the estimate: where a cut is least audible.
            desired[w - g.first] = activity.quietest_near(solution[w - g.first].clamp(g.from, g.to), 0.04, g.from, g.to);
        }
        let fitted = fit_boundaries(g.from, g.to, &desired, &mins[g.first..g.end]);
        for w in g.first..g.end {
            starts[w] = fitted[w - g.first];
            ends[w] = if w + 1 < g.end { fitted[w + 1 - g.first] } else { g.to };
        }
    }

    drafts
        .into_iter()
        .enumerate()
        .map(|(i, d)| Word { text: d.text, start: starts[i], end: ends[i], confidence: d.confidence })
        .collect()
}

/// Floor applied when repairing overlaps; real minimums come from [`min_word_duration`].
const MIN_WORD: f64 = 0.02;

/// Makes times non-decreasing and non-overlapping inside `0..=duration`.
pub(crate) fn make_monotonic(mut words: Vec<Word>, duration: f64) -> Vec<Word> {
    let duration = duration.max(0.0);
    let mut cursor = 0.0f64;
    for w in &mut words {
        let mut start = if w.start.is_finite() { w.start } else { cursor };
        let mut end = if w.end.is_finite() { w.end } else { start };
        start = start.clamp(cursor, duration);
        end = end.max(start + MIN_WORD).min(duration).max(start);
        w.start = start;
        w.end = end;
        cursor = end;
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::{RawSegment, RawToken};

    fn tok(text: &str, t0: f64, t1: f64) -> RawToken {
        RawToken { bytes: text.as_bytes().to_vec(), t0, t1, t_dtw: Some(t0), ts: None, p: 0.9 }
    }

    fn seg(tokens: Vec<RawToken>) -> RawSegment {
        RawSegment { tokens }
    }

    fn texts(segments: &[RawSegment]) -> Vec<String> {
        draft_words(segments).into_iter().map(|d| d.text).collect()
    }

    #[test]
    fn subword_tokens_merge_and_punctuation_attaches() {
        let s = seg(vec![
            tok(" Hello", 0.0, 0.4),
            tok(",", 0.4, 0.5),
            tok(" wor", 0.5, 0.7),
            tok("ld", 0.7, 0.9),
            tok(".", 0.9, 1.5),
            tok(" -", 1.5, 1.6),
            tok(" Don", 1.6, 1.8),
            tok("'t", 1.8, 1.9),
            tok("!", 1.9, 2.0),
        ]);
        let words = draft_words(&[s]);
        let t: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(t, ["Hello,", "world.-", "Don't!"]);
        // Trailing punctuation does not stretch the word.
        assert_eq!(words[1].t0, 0.5);
        assert_eq!(words[1].t1, 0.9);
        assert!(words.iter().all(|w| !w.text.starts_with(' ') && w.confidence > 0.0 && w.confidence <= 1.0));
    }

    #[test]
    fn opening_punctuation_goes_to_the_next_word() {
        let s = seg(vec![tok(" \"", 0.0, 0.1), tok("Hi", 0.1, 0.3), tok(",\"", 0.3, 0.4), tok(" ¿", 0.4, 0.5), tok(" Qué", 0.5, 0.8), tok("?", 0.8, 0.9)]);
        assert_eq!(texts(&[s]), ["\"Hi,\"", "¿Qué?"]);
    }

    #[test]
    fn non_speech_annotations_are_removed() {
        let a = seg(vec![tok(" [", 0.0, 0.1), tok("BL", 0.1, 0.2), tok("ANK", 0.2, 0.3), tok("_", 0.3, 0.4), tok("AUDIO", 0.4, 0.5), tok("]", 0.5, 0.6)]);
        let b = seg(vec![tok(" (", 0.0, 0.1), tok("up", 0.1, 0.2), tok("beat", 0.2, 0.3), tok(" music", 0.3, 0.4), tok(")", 0.4, 0.5), tok(" Okay", 0.6, 0.9), tok(".", 0.9, 1.0)]);
        let c = seg(vec![tok(" *", 0.0, 0.1), tok("s", 0.1, 0.2), tok("igh", 0.2, 0.3), tok("*", 0.3, 0.4), tok(" Right", 0.5, 0.8), tok(" ♪", 0.9, 1.0), tok(" [Music]", 1.0, 1.2)]);
        // An unclosed bracket must not swallow the next segment.
        let d = seg(vec![tok(" [", 0.0, 0.1), tok("noise", 0.1, 0.2)]);
        let e = seg(vec![tok(" Yes", 0.0, 0.3)]);
        assert_eq!(texts(&[a, b, c, d, e]), ["Okay.", "Right", "Yes"]);
    }

    #[test]
    fn split_utf8_sequences_are_reassembled() {
        let bytes = "é".as_bytes();
        let s = seg(vec![
            RawToken { bytes: [b" caf".as_slice(), &bytes[..1]].concat(), t0: 0.0, t1: 0.2, t_dtw: None, ts: None, p: 0.9 },
            RawToken { bytes: bytes[1..].to_vec(), t0: 0.2, t1: 0.3, t_dtw: None, ts: None, p: 0.9 },
        ]);
        assert_eq!(texts(&[s]), ["café"]);
    }

    #[test]
    fn spaceless_scripts_split_per_token() {
        let s = seg(vec![tok("今日", 0.0, 0.3), tok("は", 0.3, 0.4), tok("。", 0.4, 0.5)]);
        assert_eq!(texts(&[s]), ["今日", "は。"]);
    }

    #[test]
    fn monotonic_repairs_overlaps_and_clamps() {
        let w = |s: f64, e: f64| Word { text: "x".into(), start: s, end: e, confidence: 1.0 };
        let out = make_monotonic(vec![w(-1.0, 0.5), w(0.3, 0.4), w(f64::NAN, 2.0), w(9.0, 12.0)], 10.0);
        let mut cursor = 0.0;
        for word in &out {
            assert!(word.start >= cursor && word.end >= word.start && word.end <= 10.0, "{out:?}");
            cursor = word.end;
        }
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn silence_and_steady_hiss_have_no_speech() {
        assert_eq!(analyse(&vec![0.0; 16000 * 3]).regions, Some(vec![]));
        assert_eq!(analyse(&[]).regions, Some(vec![]));
        // Deterministic pseudo-noise at about -50 dBFS.
        let mut x = 12345u32;
        let hiss: Vec<f32> = (0..16000 * 3)
            .map(|_| {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                ((x >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.01
            })
            .collect();
        assert_eq!(analyse(&hiss).regions, Some(vec![]));
    }

    #[test]
    fn bursts_between_silence_are_found_with_their_edges() {
        let mut s = vec![0.0f32; 16000 * 4];
        for (from, to) in [(16000, 24000), (40000, 56000)] {
            for (i, v) in s[from..to].iter_mut().enumerate() {
                *v = 0.3 * (i as f32 * 0.35).sin();
            }
        }
        let regions = analyse(&s).regions.expect("contrast is obvious");
        assert_eq!(regions.len(), 2, "{regions:?}");
        for (found, truth) in regions.iter().zip([(1.0, 1.5), (2.5, 3.5)]) {
            assert!((found.0 - truth.0).abs() <= 0.04 && (found.1 - truth.1).abs() <= 0.04, "{regions:?}");
        }
    }

    #[test]
    fn words_snap_to_the_sound_and_hallucinations_in_silence_are_dropped() {
        // Sound at 1.0-1.5 and 2.5-3.5; DTW is 0.2 s late; a third segment sits in silence.
        let mut s = vec![0.0f32; 16000 * 8];
        for (from, to) in [(16000, 24000), (40000, 56000)] {
            for (i, v) in s[from..to].iter_mut().enumerate() {
                *v = 0.3 * (i as f32 * 0.35).sin();
            }
        }
        let activity = analyse(&s);
        let d = |text: &str, segment: usize, dtw: f64| Draft { text: text.into(), segment, dtw: Some(dtw), ts: None, t0: 0.0, t1: 0.0, confidence: 1.0 };
        let drafts = vec![d("one", 0, 1.2), d("two", 0, 1.45), d("three", 1, 2.7), d("four", 1, 3.2), d("Thanks.", 2, 6.0)];
        let words = time_words(drafts, &activity, Timing::DtwSnapped, 8.0, DEFAULT_LAG);
        let t: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(t, ["one", "two", "three", "four"]);
        assert!((words[0].start - 1.0).abs() <= 0.04, "{words:?}");
        assert!((words[1].end - 1.5).abs() <= 0.04, "{words:?}");
        assert!((words[2].start - 2.5).abs() <= 0.04, "{words:?}");
        assert!((words[3].end - 3.5).abs() <= 0.04, "{words:?}");
        assert!((words[3].start - 3.0).abs() <= 0.1, "lag-corrected inner boundary: {words:?}");
    }

    fn tone(spans: &[(usize, usize)], seconds: usize) -> Vec<f32> {
        let mut s = vec![0.0f32; 16000 * seconds];
        for &(from, to) in spans {
            for (i, v) in s[from..to].iter_mut().enumerate() {
                *v = 0.3 * (i as f32 * 0.35).sin();
            }
        }
        s
    }

    #[test]
    fn first_word_after_a_short_pause_is_not_squeezed_onto_the_previous_stretch() {
        // Two stretches 0.2 s apart (1.0-2.0 and 2.2-3.2). The DTW time of "Goodbye" (2.3) is
        // only 0.1 s into its stretch, so any constant lag correction puts it in the pause,
        // nearer to the previous stretch. It must still get its own audio.
        let activity = analyse(&tone(&[(16000, 32000), (35200, 51200)], 5));
        let d = |text: &str, dtw: f64| Draft { text: text.into(), segment: 0, dtw: Some(dtw), ts: None, t0: 0.0, t1: 0.0, confidence: 1.0 };
        let drafts = vec![d("lovely", 1.25), d("today.", 1.75), d("Goodbye", 2.3), d("everyone.", 2.95)];
        let words = time_words(drafts, &activity, Timing::DtwSnapped, 5.0, DEFAULT_LAG);
        assert!((words[1].end - 2.0).abs() <= 0.04, "{words:?}");
        assert!((words[2].start - 2.2).abs() <= 0.04, "Goodbye must start with its stretch: {words:?}");
        assert!(words[2].end - words[2].start >= 0.3, "Goodbye must keep its audio: {words:?}");
        assert!(words[3].end - words[3].start <= 0.7 && (words[3].end - 3.2).abs() <= 0.04, "{words:?}");
    }

    #[test]
    fn every_word_gets_a_plausible_minimum_even_with_useless_dtw_times() {
        // All DTW times identical: the stretch is shared out, nobody is left with a sliver.
        let activity = analyse(&tone(&[(16000, 48000)], 5));
        let d = |text: &str| Draft { text: text.into(), segment: 0, dtw: Some(1.5), ts: None, t0: 0.0, t1: 0.0, confidence: 1.0 };
        let texts = ["Absolutely", "nothing", "is", "known", "here"];
        let words = time_words(texts.iter().map(|t| d(t)).collect(), &activity, Timing::DtwSnapped, 5.0, DEFAULT_LAG);
        assert_eq!(words.len(), 5);
        for w in &words {
            assert!(w.end - w.start >= min_word_duration(&w.text) - 1e-9, "{words:?}");
            assert!(w.start >= 0.95 && w.end <= 3.05, "{words:?}");
        }
        assert!((words[0].start - 1.0).abs() <= 0.04, "{words:?}");
        assert!(words[0].end - words[0].start > words[2].end - words[2].start, "longer words get more time: {words:?}");
    }

    #[test]
    fn a_stretch_too_short_for_its_words_is_divided_in_proportion() {
        let mins = [0.2, 0.1, 0.1];
        let starts = fit_boundaries(1.0, 1.2, &[1.0, 1.0, 1.0], &mins);
        assert!((starts[1] - 1.1).abs() < 1e-9 && (starts[2] - 1.15).abs() < 1e-9, "{starts:?}");
        // With room to spare, the wishes are respected as far as the minimums allow.
        let starts = fit_boundaries(0.0, 2.0, &[0.0, 0.05, 1.95], &mins);
        assert!((starts[1] - 0.2).abs() < 1e-9 && (starts[2] - 1.9).abs() < 1e-9, "{starts:?}");
    }

    #[test]
    fn overfull_stretch_hands_its_edge_word_to_a_neighbour_with_room() {
        // A 0.1 s blip followed by a real stretch; DTW puts both words on the blip.
        let activity = analyse(&tone(&[(16000, 17600), (22400, 38400)], 4));
        let d = |text: &str, dtw: f64| Draft { text: text.into(), segment: 0, dtw: Some(dtw), ts: None, t0: 0.0, t1: 0.0, confidence: 1.0 };
        let words = time_words(vec![d("Yes", 1.05), d("certainly", 1.1)], &activity, Timing::DtwSnapped, 4.0, DEFAULT_LAG);
        assert!(words[0].end <= 1.15 && words[1].start >= 1.35, "{words:?}");
        assert!(words[1].end - words[1].start >= 0.2, "{words:?}");
    }
}
