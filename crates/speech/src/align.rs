//! "Import transcript": fitting the user's own text onto recognised word timings.

use mango_core::Transcript;

/// One word of the user's text with its time in the media.
#[derive(Clone, Debug, PartialEq)]
pub struct AlignedWord {
    /// The word exactly as the user wrote it, punctuation included.
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// Speaker name from the most recent `Name: ...` line, if any.
    pub speaker: Option<String>,
    /// True when the recogniser heard this very word (ignoring case and punctuation) and the
    /// times are the recogniser's. False when the times were interpolated.
    pub matched: bool,
}

/// Lower-case letters and digits only; apostrophes and other punctuation are dropped, so
/// "Don't," equals "dont".
fn normalise(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Recognises a `Name: text` line and returns (name, rest). The name must look like a name:
/// short, starting with a letter, no sentence punctuation, and the colon must be followed by
/// a space or the end of the line (so "10:30" and "http://..." are not speakers).
fn speaker_prefix(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let (name, rest) = (line[..colon].trim(), &line[colon + 1..]);
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let words = name.split_whitespace().count();
    let ok = !name.is_empty()
        && name.chars().count() <= 40
        && words <= 4
        && name.chars().next().is_some_and(char::is_alphabetic)
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '-' | '\'' | '_' | '(' | ')'))
        && !name.ends_with('.')
        // "Dr." and "Prof." are fine inside a name; "Great. Note: ..." is a sentence, not a speaker.
        && name.split_whitespace().all(|w| !w.ends_with('.') || w.chars().count() <= 5);
    ok.then_some((name, rest))
}

struct UserWord {
    text: String,
    norm: String,
    speaker: Option<String>,
}

fn parse_user_text(user_text: &str) -> Vec<UserWord> {
    let mut out: Vec<UserWord> = Vec::new();
    let mut speaker: Option<String> = None;
    let mut pending_prefix = String::new();
    for line in user_text.lines() {
        let mut body = line.trim();
        if let Some((name, rest)) = speaker_prefix(body) {
            speaker = Some(name.to_string());
            body = rest;
        }
        for token in body.split_whitespace() {
            let norm = normalise(token);
            if norm.is_empty() {
                // Free-standing punctuation ("-", "...") belongs to the neighbouring word.
                match out.last_mut() {
                    Some(prev) => prev.text.push_str(token),
                    None => pending_prefix.push_str(token),
                }
                continue;
            }
            let mut text = std::mem::take(&mut pending_prefix);
            text.push_str(token);
            out.push(UserWord { text, norm, speaker: speaker.clone() });
        }
    }
    out
}

/// Pairs of (user index, reference index) for words that are equal, in order, from a
/// minimum-edit-distance alignment. The search is limited to a band around the diagonal so
/// that hour-long transcripts stay cheap; inside the band the alignment is optimal.
fn matched_pairs(user: &[&str], reference: &[&str]) -> Vec<(usize, usize)> {
    let (n, m) = (user.len(), reference.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let half = (n.abs_diff(m) + 256).max(256).min(m.max(1));
    let width = 2 * half + 1;
    // Column window for row i: [lo(i), lo(i) + width), clipped to 0..=m.
    let lo = |i: usize| -> usize {
        let centre = (i as u128 * m as u128 / n as u128) as usize;
        centre.saturating_sub(half)
    };
    const INF: u32 = u32::MAX / 2;
    const DIAG: u8 = 0;
    const UP: u8 = 1; // user word unpaired
    const LEFT: u8 = 2; // reference word unpaired
    let mut dirs = vec![LEFT; (n + 1) * width];
    let mut prev = vec![INF; width];
    let mut cur = vec![INF; width];
    for i in 0..=n {
        let l = lo(i);
        let pl = if i > 0 { lo(i - 1) } else { 0 };
        for k in 0..width {
            let j = l + k;
            if j > m {
                cur[k] = INF;
                continue;
            }
            let mut best = INF;
            let mut dir = LEFT;
            if i == 0 && j == 0 {
                best = 0;
            }
            if i > 0 && j > 0 && j - 1 >= pl && j - 1 - pl < width {
                let c = prev[j - 1 - pl];
                if c < INF {
                    // An equal pair is free; a differing pair costs slightly less than
                    // dropping one word on each side.
                    let cost = c + if user[i - 1] == reference[j - 1] { 0 } else { 3 };
                    if cost < best {
                        best = cost;
                        dir = DIAG;
                    }
                }
            }
            if i > 0 && j >= pl && j - pl < width {
                let c = prev[j - pl];
                if c < INF && c + 2 < best {
                    best = c + 2;
                    dir = UP;
                }
            }
            if k > 0 {
                let c = cur[k - 1];
                if c < INF && c + 2 < best {
                    best = c + 2;
                    dir = LEFT;
                }
            }
            cur[k] = best;
            dirs[i * width + k] = dir;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        let l = lo(i);
        if j < l || j - l >= width {
            break; // outside the band: leave the rest unpaired
        }
        match dirs[i * width + (j - l)] {
            DIAG => {
                if user[i - 1] == reference[j - 1] {
                    pairs.push((i - 1, j - 1));
                }
                i -= 1;
                j -= 1;
            }
            UP => i -= 1,
            _ => j -= 1,
        }
    }
    pairs.reverse();
    pairs
}

/// Produces word timings for the user's words.
///
/// `reference` is what the recogniser heard for the same media; `user_text` is the text the
/// user wants to see; `duration` is the media duration in seconds. Every word of the user's
/// text is returned, in order. Words the recogniser also heard take its times exactly
/// (`matched = true`). The others share the time between their matched neighbours, in
/// proportion to their length: when the recogniser heard different words there, they take
/// over exactly that stretch; when it heard nothing, they sit in the gap next to the word
/// they follow, at a plausible speaking length rather than stretched across a long silence.
/// With nothing recognised at all, the words are spread evenly over the whole duration.
/// Times never go backwards and stay within `0..=duration`.
///
/// Lines of the form `Name: text` set the speaker for the words that follow.
pub fn align_text(reference: &Transcript, user_text: &str, duration: f64) -> Vec<AlignedWord> {
    let user = parse_user_text(user_text);
    if user.is_empty() {
        return Vec::new();
    }
    let duration = if duration.is_finite() { duration.max(0.0) } else { 0.0 };
    let clamp = |t: f64| if t.is_finite() { t.clamp(0.0, duration) } else { 0.0 };
    // Reference words that carry something comparable; keep their original indices.
    let reference_words: Vec<(String, f64, f64)> = reference
        .words
        .iter()
        .map(|w| (normalise(&w.text), clamp(w.start), clamp(w.end).max(clamp(w.start))))
        .filter(|w| !w.0.is_empty())
        .collect();
    let user_norm: Vec<&str> = user.iter().map(|w| w.norm.as_str()).collect();
    let ref_norm: Vec<&str> = reference_words.iter().map(|w| w.0.as_str()).collect();
    let pairs = matched_pairs(&user_norm, &ref_norm);

    let mut out: Vec<AlignedWord> = user
        .iter()
        .map(|w| AlignedWord { text: w.text.clone(), start: 0.0, end: 0.0, speaker: w.speaker.clone(), matched: false })
        .collect();
    for &(u, r) in &pairs {
        out[u].start = reference_words[r].1;
        out[u].end = reference_words[r].2;
        out[u].matched = true;
    }

    // Fill each run of unmatched user words between two anchors.
    let mut anchors: Vec<(isize, isize)> = Vec::with_capacity(pairs.len() + 2);
    anchors.push((-1, -1));
    anchors.extend(pairs.iter().map(|&(u, r)| (u as isize, r as isize)));
    anchors.push((user.len() as isize, reference_words.len() as isize));
    for pair in anchors.windows(2) {
        let ((u0, r0), (u1, r1)) = (pair[0], pair[1]);
        if u1 - u0 <= 1 {
            continue;
        }
        let left = if r0 >= 0 { reference_words[r0 as usize].2 } else { 0.0 };
        let right = if (r1 as usize) < reference_words.len() { reference_words[r1 as usize].1 } else { duration };
        // Prefer the stretch the recogniser assigned to whatever it heard here instead.
        let (mut from, mut to) = if r1 - r0 > 1 {
            (reference_words[(r0 + 1) as usize].1, reference_words[(r1 - 1) as usize].2)
        } else {
            (left, right)
        };
        from = from.clamp(left.min(right), right.max(left));
        to = to.clamp(from, right.max(from));
        let run = (u0 + 1) as usize..u1 as usize;
        if r1 - r0 <= 1 && !pairs.is_empty() {
            // The recogniser heard nothing here, so the gap may be a long silence. Do not
            // stretch the words across it: give them a plausible length next to the
            // neighbour they follow (or, at the very start, the one they precede).
            let need: f64 = run.clone().map(|i| 0.3 + 0.09 * user[i].norm.chars().count() as f64).sum();
            if to - from > need {
                if r0 < 0 {
                    from = to - need;
                } else {
                    to = from + need;
                }
            }
        }
        let weights: Vec<f64> = run.clone().map(|i| user[i].norm.chars().count().max(1) as f64).collect();
        let total: f64 = weights.iter().sum();
        let mut t = from;
        for (k, i) in run.enumerate() {
            let len = (to - from) * weights[k] / total;
            out[i].start = t;
            out[i].end = t + len;
            t += len;
        }
    }

    // Guarantee order even if the reference itself was not monotonic.
    let mut cursor = 0.0f64;
    for w in &mut out {
        w.start = clamp(w.start).max(cursor);
        w.end = clamp(w.end).max(w.start);
        cursor = w.end;
    }
    out
}

/// The distinct speaker names in order of first appearance, for creating speakers.
pub fn speaker_names(words: &[AlignedWord]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for w in words {
        if let Some(name) = &w.speaker {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    names
}
