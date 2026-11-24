//! Text-based editing: operations on a composition's token list, and the
//! mapping between script positions, composition time and source time.

use crate::model::*;
use std::ops::Range;

/// Two tokens are contiguous in the source if the second starts where the
/// first ends, within this tolerance.
pub const CONTIGUITY_EPS: Secs = 0.002;
/// Silences shorter than this are folded into the neighbouring words.
pub const MIN_GAP: Secs = 0.08;
/// A pause this long starts a new paragraph when building a script.
pub const PARAGRAPH_PAUSE: Secs = 1.5;

// ---------------------------------------------------------------------------
// Building a script from a transcript
// ---------------------------------------------------------------------------

/// Turns an asset's transcript into script tokens that cover the *whole*
/// asset: words, with gap tokens for the silences between them, so that
/// playing every token back reproduces the source exactly.
pub fn tokens_from_asset(asset: &MediaAsset) -> Vec<Token> {
    let dur = asset.duration.max(0.0);
    let mut out: Vec<Token> = Vec::new();
    let mut cursor: Secs = 0.0;
    let words: &[TranscriptWord] = asset.transcript.as_ref().map(|t| t.words.as_slice()).unwrap_or(&[]);
    let mut last_speaker: Option<SpeakerId> = None;
    let mut first_word = true;
    let mut sentence_ended = false;
    let mut words_in_paragraph = 0usize;

    for w in words {
        let start = w.start.clamp(cursor, dur.max(cursor));
        let end = w.end.clamp(start, dur.max(start));
        let gap = start - cursor;
        let mut long_pause = false;
        if gap >= MIN_GAP {
            let mut g = Token::new(TokenKind::Gap, "", asset.id, cursor, start);
            // A leading silence belongs to the first speaker's paragraph.
            g.speaker = if first_word { w.speaker } else { last_speaker };
            g.paragraph_start = out.is_empty();
            out.push(g);
            long_pause = gap >= PARAGRAPH_PAUSE;
        }
        let word_start = if gap >= MIN_GAP { start } else { cursor };
        let mut t = Token::new(TokenKind::Word, w.text.trim(), asset.id, word_start, end);
        t.speaker = w.speaker;
        t.confidence = w.confidence;
        let speaker_changed = !first_word && w.speaker != last_speaker;
        if (first_word && out.is_empty())
            || speaker_changed
            || (long_pause && sentence_ended)
            || (sentence_ended && words_in_paragraph >= 90)
        {
            t.paragraph_start = true;
            words_in_paragraph = 0;
        }
        sentence_ended = ends_sentence(&t.text);
        words_in_paragraph += 1;
        last_speaker = w.speaker;
        first_word = false;
        cursor = end;
        out.push(t);
    }
    if dur - cursor >= MIN_GAP || out.is_empty() {
        let mut g = Token::new(TokenKind::Gap, "", asset.id, cursor, dur);
        g.speaker = last_speaker;
        if out.is_empty() {
            g.paragraph_start = true;
        }
        out.push(g);
    } else if let Some(last) = out.last_mut() {
        last.src_end = dur.max(last.src_end);
    }
    out
}

pub fn ends_sentence(text: &str) -> bool {
    matches!(text.trim_end_matches(['"', '\'', ')', '\u{201D}']).chars().last(), Some('.' | '?' | '!'))
}

/// Lower-cased word with surrounding punctuation removed, for comparisons.
pub fn normalize_word(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_lowercase()
        .replace('\u{2019}', "'")
}

// ---------------------------------------------------------------------------
// Time mapping
// ---------------------------------------------------------------------------

/// A contiguous stretch of the script track that plays one piece of source.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptClip {
    /// `None` for a gap clip (black + silence).
    pub asset: Option<AssetId>,
    pub src_start: Secs,
    pub src_end: Secs,
    pub comp_start: Secs,
    /// Token indices this clip was built from (includes any interleaved
    /// ignored tokens only if they sit strictly inside; never at the edges).
    pub tokens: Range<usize>,
    /// Index of the scene this clip belongs to.
    pub scene: usize,
}

impl ScriptClip {
    pub fn duration(&self) -> Secs {
        self.src_end - self.src_start
    }
    pub fn comp_end(&self) -> Secs {
        self.comp_start + self.duration()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneSpan {
    pub index: usize,
    pub tokens: Range<usize>,
    pub start: Secs,
    pub end: Secs,
}

impl Composition {
    /// Composition start time of every token, plus the total at the end
    /// (`len + 1` entries).
    pub fn token_starts(&self) -> Vec<Secs> {
        let mut v = Vec::with_capacity(self.script.len() + 1);
        let mut t = 0.0;
        for tok in &self.script {
            v.push(t);
            t += tok.play_duration();
        }
        v.push(t);
        v
    }

    /// Length of the script track.
    pub fn script_duration(&self) -> Secs {
        self.script.iter().map(Token::play_duration).sum()
    }

    /// Length of the composition: the script, or the last layer if longer.
    pub fn duration(&self) -> Secs {
        let layers = self.layers.iter().map(Layer::end).fold(0.0, f64::max);
        self.script_duration().max(layers)
    }

    /// Index of the playing (non-ignored) token at composition time `t`.
    pub fn token_at_time(&self, t: Secs) -> Option<usize> {
        let mut acc = 0.0;
        let mut last = None;
        for (i, tok) in self.script.iter().enumerate() {
            let d = tok.play_duration();
            if d <= 0.0 {
                continue;
            }
            if t < acc + d {
                return Some(i);
            }
            acc += d;
            last = Some(i);
        }
        last
    }

    /// Composition time at which token `idx` starts (or the end for `len`).
    pub fn time_of_token(&self, idx: usize) -> Secs {
        self.script.iter().take(idx).map(Token::play_duration).sum()
    }

    /// Maps composition time to `(asset, source time)` on the script track.
    /// Returns `None` inside a gap clip or past the end.
    pub fn source_at(&self, t: Secs) -> Option<(AssetId, Secs)> {
        let mut acc = 0.0;
        for tok in &self.script {
            let d = tok.play_duration();
            if d <= 0.0 {
                continue;
            }
            if t < acc + d {
                if tok.kind == TokenKind::Blank {
                    return None;
                }
                return Some((tok.asset, tok.src_start + (t - acc)));
            }
            acc += d;
        }
        None
    }

    /// Composition time at which this source moment plays, if it survives.
    pub fn time_of_source(&self, asset: AssetId, src: Secs) -> Option<Secs> {
        let mut acc = 0.0;
        for tok in &self.script {
            let d = tok.play_duration();
            if d > 0.0 && tok.kind != TokenKind::Blank && tok.asset == asset && src >= tok.src_start - 1e-9 && src < tok.src_end {
                return Some(acc + (src - tok.src_start).max(0.0));
            }
            acc += d;
        }
        None
    }

    /// Scene spans. There is always at least one scene; the first token
    /// implicitly starts scene 0.
    pub fn scene_spans(&self) -> Vec<SceneSpan> {
        let starts = self.token_starts();
        let mut spans = Vec::new();
        let mut begin = 0usize;
        for (i, tok) in self.script.iter().enumerate() {
            if i > 0 && tok.scene_start {
                spans.push(SceneSpan { index: spans.len(), tokens: begin..i, start: starts[begin], end: starts[i] });
                begin = i;
            }
        }
        let n = self.script.len();
        spans.push(SceneSpan { index: spans.len(), tokens: begin..n, start: starts[begin], end: starts[n] });
        spans
    }

    pub fn scene_index_of_token(&self, idx: usize) -> usize {
        self.script.iter().take(idx + 1).skip(1).filter(|t| t.scene_start).count()
    }

    pub fn scene_at_time(&self, t: Secs) -> usize {
        let spans = self.scene_spans();
        spans.iter().rev().find(|s| t >= s.start).map(|s| s.index).unwrap_or(0)
    }

    /// Makes `self.scenes` have exactly one entry per scene span.
    pub fn sync_scene_settings(&mut self) {
        let n = self.script.iter().skip(1).filter(|t| t.scene_start).count() + 1;
        while self.scenes.len() < n {
            self.scenes.push(SceneSettings::default());
        }
        self.scenes.truncate(n);
    }

    /// The script track as clips: maximal runs of playing tokens that are
    /// contiguous in the same source and in the same scene.
    pub fn script_clips(&self) -> Vec<ScriptClip> {
        let mut clips: Vec<ScriptClip> = Vec::new();
        let mut t = 0.0;
        let mut scene = 0usize;
        for (i, tok) in self.script.iter().enumerate() {
            if i > 0 && tok.scene_start {
                scene += 1;
            }
            let d = tok.play_duration();
            if d <= 0.0 {
                continue;
            }
            let asset = if tok.kind == TokenKind::Blank { None } else { Some(tok.asset) };
            let extend = clips.last().is_some_and(|c| {
                c.scene == scene
                    && c.asset == asset
                    && c.tokens.end == i
                    && (asset.is_none() || (tok.src_start - c.src_end).abs() <= CONTIGUITY_EPS)
            });
            if extend {
                let c = clips.last_mut().unwrap();
                c.src_end = if asset.is_none() { c.src_end + d } else { tok.src_end };
                c.tokens.end = i + 1;
            } else {
                let (s, e) = if asset.is_none() { (0.0, d) } else { (tok.src_start, tok.src_end) };
                clips.push(ScriptClip { asset, src_start: s, src_end: e, comp_start: t, tokens: i..i + 1, scene });
            }
            t += d;
        }
        clips
    }

    /// Indices `i` such that there is a cut between token `i-1` and `i`:
    /// both play, but they are not contiguous in the source.
    pub fn edit_boundaries(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut prev: Option<&Token> = None;
        for (i, tok) in self.script.iter().enumerate() {
            if tok.ignored {
                if prev.is_some() {
                    // An ignored stretch is itself a cut; report it at the next playing token.
                }
                continue;
            }
            if let Some(p) = prev {
                let contiguous = p.kind != TokenKind::Blank
                    && tok.kind != TokenKind::Blank
                    && p.asset == tok.asset
                    && (tok.src_start - p.src_end).abs() <= CONTIGUITY_EPS;
                if !contiguous {
                    out.push(i);
                }
            }
            prev = Some(tok);
        }
        out
    }

    // -----------------------------------------------------------------------
    // Text
    // -----------------------------------------------------------------------

    /// Plain text of a token range (words only, ignored text skipped).
    pub fn text_of(&self, range: Range<usize>) -> String {
        let mut s = String::new();
        for tok in &self.script[clamp_range(&range, self.script.len())] {
            if !tok.shows_text() {
                continue;
            }
            if !s.is_empty() {
                s.push(if tok.paragraph_start { '\n' } else { ' ' });
            }
            s.push_str(&tok.text);
        }
        s
    }

    pub fn word_count(&self, range: Range<usize>) -> usize {
        self.script[clamp_range(&range, self.script.len())].iter().filter(|t| t.shows_text()).count()
    }

    pub fn range_duration(&self, range: Range<usize>) -> Secs {
        self.script[clamp_range(&range, self.script.len())].iter().map(Token::play_duration).sum()
    }

    /// Paragraphs as token ranges.
    pub fn paragraphs(&self) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        let mut begin = 0;
        for (i, tok) in self.script.iter().enumerate() {
            if i > 0 && (tok.paragraph_start || tok.scene_start || tok.marker.is_some()) {
                out.push(begin..i);
                begin = i;
            }
        }
        if !self.script.is_empty() {
            out.push(begin..self.script.len());
        }
        out
    }

    // -----------------------------------------------------------------------
    // Edits. These mutate directly; `Editor` wraps them with undo.
    // -----------------------------------------------------------------------

    /// Removes tokens. Structural flags on the first removed token (paragraph,
    /// scene, marker) are carried over to the token that takes its place.
    pub fn delete_range(&mut self, range: Range<usize>) -> Vec<Token> {
        let range = clamp_range(&range, self.script.len());
        if range.is_empty() {
            return Vec::new();
        }
        let removed: Vec<Token> = self.script.drain(range.clone()).collect();
        if let Some(next) = self.script.get_mut(range.start) {
            let any_par = removed.iter().any(|t| t.paragraph_start);
            let first_scene = removed.first().is_some_and(|t| t.scene_start);
            // Deleting across a paragraph break joins the paragraphs unless the
            // deletion began exactly at one.
            if removed.first().is_some_and(|t| t.paragraph_start) && any_par {
                next.paragraph_start = true;
            }
            if first_scene {
                next.scene_start = true;
            }
            if next.marker.is_none() {
                next.marker = removed.first().and_then(|t| t.marker.clone());
            }
        }
        if let Some(first) = self.script.first_mut() {
            first.paragraph_start = true;
        }
        self.sync_scene_settings();
        removed
    }

    pub fn set_ignored(&mut self, range: Range<usize>, ignored: bool) {
        let range = clamp_range(&range, self.script.len());
        for t in &mut self.script[range] {
            t.ignored = ignored;
        }
    }

    /// Deletes every ignored token.
    pub fn remove_ignored(&mut self) -> usize {
        let before = self.script.len();
        let mut i = 0;
        while i < self.script.len() {
            if self.script[i].ignored {
                self.delete_range(i..i + 1);
            } else {
                i += 1;
            }
        }
        before - self.script.len()
    }

    /// Replaces the range with one gap clip of the same playing duration.
    pub fn replace_with_gap(&mut self, range: Range<usize>) {
        let range = clamp_range(&range, self.script.len());
        if range.is_empty() {
            return;
        }
        let dur: Secs = self.script[range.clone()].iter().map(Token::play_duration).sum();
        let first = self.script[range.start].clone();
        let mut blank = Token::new(TokenKind::Blank, "", first.asset, 0.0, dur);
        blank.speaker = first.speaker;
        blank.paragraph_start = first.paragraph_start;
        blank.scene_start = first.scene_start;
        blank.marker = first.marker.clone();
        self.script.splice(range, [blank]);
        self.sync_scene_settings();
    }

    /// Inserts a gap clip of `dur` seconds before token `idx`.
    pub fn insert_gap(&mut self, idx: usize, dur: Secs) {
        let idx = idx.min(self.script.len());
        let asset = self.script.get(idx).or(self.script.last()).map(|t| t.asset).unwrap_or_default();
        let mut blank = Token::new(TokenKind::Blank, "", asset, 0.0, dur.max(0.0));
        blank.paragraph_start = self.script.is_empty();
        self.script.insert(idx, blank);
    }

    /// Inserts tokens at `idx`. The first inserted token starts a paragraph if
    /// the script was empty.
    pub fn insert_tokens(&mut self, idx: usize, mut tokens: Vec<Token>) -> Range<usize> {
        let idx = idx.min(self.script.len());
        let n = tokens.len();
        if n == 0 {
            return idx..idx;
        }
        if idx == 0 {
            tokens[0].paragraph_start = true;
            if let Some(old_first) = self.script.first_mut() {
                // Old first token keeps starting its own paragraph.
                old_first.paragraph_start = true;
            }
        }
        self.script.splice(idx..idx, tokens);
        self.sync_scene_settings();
        idx..idx + n
    }

    /// Moves a token range so that it begins before the token currently at
    /// `dest` (cut + paste). Returns the new range.
    pub fn move_range(&mut self, range: Range<usize>, dest: usize) -> Range<usize> {
        let range = clamp_range(&range, self.script.len());
        let dest = dest.min(self.script.len());
        if range.is_empty() || (dest >= range.start && dest <= range.end) {
            return range;
        }
        let moved: Vec<Token> = self.script.drain(range.clone()).collect();
        let at = if dest > range.end { dest - moved.len() } else { dest };
        let n = moved.len();
        self.script.splice(at..at, moved);
        if let Some(first) = self.script.first_mut() {
            first.paragraph_start = true;
        }
        self.sync_scene_settings();
        at..at + n
    }

    /// Changes a word's text without touching its media. An empty string
    /// hides the word from the transcript instead.
    pub fn correct_word(&mut self, idx: usize, text: &str) {
        if let Some(t) = self.script.get_mut(idx) {
            if t.kind != TokenKind::Word {
                return;
            }
            let text = text.trim();
            if text.is_empty() {
                t.hidden_text = true;
            } else {
                t.text = text.to_string();
                t.hidden_text = false;
                t.confidence = 1.0;
            }
        }
    }

    /// Replaces the text of a run of words with new text. Timing of the run is
    /// redistributed across the new words in proportion to their length, so
    /// "gonna" → "going to" yields two words over the same media.
    pub fn correct_range(&mut self, range: Range<usize>, text: &str) {
        let range = clamp_range(&range, self.script.len());
        let new_words: Vec<&str> = text.split_whitespace().collect();
        let word_idx: Vec<usize> = range.clone().filter(|&i| self.script[i].kind == TokenKind::Word).collect();
        if word_idx.is_empty() {
            return;
        }
        if new_words.is_empty() {
            for i in word_idx {
                self.script[i].hidden_text = true;
            }
            return;
        }
        if new_words.len() == word_idx.len() {
            for (i, w) in word_idx.iter().zip(&new_words) {
                self.correct_word(*i, w);
            }
            return;
        }
        // Different word count: only safe when the run is contiguous source.
        let first = word_idx[0];
        let last = *word_idx.last().unwrap();
        let contiguous = (first..=last).all(|i| {
            i == first || {
                let (a, b) = (&self.script[i - 1], &self.script[i]);
                a.asset == b.asset && (b.src_start - a.src_end).abs() <= CONTIGUITY_EPS && !b.ignored && !a.ignored
            }
        });
        if !contiguous {
            // Fall back: put all the text on the first word, hide the rest.
            self.correct_word(first, &new_words.join(" "));
            for &i in &word_idx[1..] {
                self.script[i].hidden_text = true;
            }
            return;
        }
        let template = self.script[first].clone();
        let (s, e) = (self.script[first].src_start, self.script[last].src_end);
        let total_chars: usize = new_words.iter().map(|w| w.chars().count().max(1)).sum();
        let mut cursor = s;
        let mut toks = Vec::with_capacity(new_words.len());
        for (k, w) in new_words.iter().enumerate() {
            let share = w.chars().count().max(1) as f64 / total_chars as f64;
            let end = if k + 1 == new_words.len() { e } else { cursor + (e - s) * share };
            let mut t = Token::new(TokenKind::Word, *w, template.asset, cursor, end);
            t.speaker = template.speaker;
            if k == 0 {
                t.paragraph_start = template.paragraph_start;
                t.scene_start = template.scene_start;
                t.marker = template.marker.clone();
                t.note = template.note.clone();
            }
            t.highlight = template.highlight;
            cursor = end;
            toks.push(t);
        }
        self.script.splice(first..last + 1, toks);
    }

    /// Replaces every word equal to `from` (case-insensitive, punctuation
    /// preserved) with `to`. Returns the number of replacements.
    pub fn correct_all(&mut self, from: &str, to: &str) -> usize {
        let key = normalize_word(from);
        if key.is_empty() {
            return 0;
        }
        let mut n = 0;
        for t in &mut self.script {
            if t.kind == TokenKind::Word && normalize_word(&t.text) == key {
                t.text = replace_core(&t.text, to);
                n += 1;
            }
        }
        n
    }

    pub fn set_speaker(&mut self, range: Range<usize>, speaker: Option<SpeakerId>) {
        let range = clamp_range(&range, self.script.len());
        if range.is_empty() {
            return;
        }
        let end = range.end;
        for t in &mut self.script[range.clone()] {
            t.speaker = speaker;
        }
        self.script[range.start].paragraph_start = true;
        if let Some(next) = self.script.get_mut(end) {
            if next.speaker != speaker {
                next.paragraph_start = true;
            }
        }
    }

    pub fn set_paragraph_break(&mut self, idx: usize, on: bool) {
        if idx == 0 {
            return;
        }
        if let Some(t) = self.script.get_mut(idx) {
            t.paragraph_start = on;
        }
    }

    pub fn set_scene_break(&mut self, idx: usize, on: bool) {
        if idx == 0 || idx >= self.script.len() {
            return;
        }
        if self.script[idx].scene_start == on {
            return;
        }
        let scene = self.scene_index_of_token(idx);
        self.script[idx].scene_start = on;
        if on {
            // New scene inherits nothing; insert default settings after `scene`.
            self.sync_scene_settings_insert(scene + 1);
        } else {
            self.sync_scene_settings_remove(scene);
        }
    }

    fn sync_scene_settings_insert(&mut self, at: usize) {
        while self.scenes.len() < at {
            self.scenes.push(SceneSettings::default());
        }
        self.scenes.insert(at.min(self.scenes.len()), SceneSettings::default());
        self.sync_scene_settings();
    }

    fn sync_scene_settings_remove(&mut self, at: usize) {
        if at < self.scenes.len() && at > 0 {
            self.scenes.remove(at);
        }
        self.sync_scene_settings();
    }

    pub fn set_marker(&mut self, idx: usize, name: Option<String>) {
        if let Some(t) = self.script.get_mut(idx) {
            t.marker = name.filter(|n| !n.trim().is_empty());
        }
    }

    /// Script markers resolved to composition time, in order.
    pub fn script_markers(&self) -> Vec<(usize, Secs, String)> {
        let starts = self.token_starts();
        self.script
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.marker.clone().map(|m| (i, starts[i], m)))
            .collect()
    }

    pub fn set_highlight(&mut self, range: Range<usize>, color: Option<u8>) {
        let range = clamp_range(&range, self.script.len());
        for t in &mut self.script[range] {
            t.highlight = color;
        }
    }

    /// Token ranges of each highlighted run, optionally filtered by colour.
    pub fn highlights(&self, color: Option<u8>) -> Vec<Range<usize>> {
        let mut out: Vec<Range<usize>> = Vec::new();
        for (i, t) in self.script.iter().enumerate() {
            let hit = match (t.highlight, color) {
                (Some(h), Some(c)) => h == c,
                (Some(_), None) => true,
                _ => false,
            };
            if hit {
                match out.last_mut() {
                    Some(r) if r.end == i && self.script[i - 1].highlight == t.highlight => r.end = i + 1,
                    _ => out.push(i..i + 1),
                }
            }
        }
        out
    }

    /// Sets the playing length of a gap token. Shortening keeps the head and
    /// tail of the silence (so word decays and attacks survive); lengthening
    /// appends a gap clip.
    pub fn set_gap_duration(&mut self, idx: usize, dur: Secs) {
        let dur = dur.max(0.0);
        let Some(tok) = self.script.get(idx).cloned() else { return };
        match tok.kind {
            TokenKind::Blank => {
                self.script[idx].src_start = 0.0;
                self.script[idx].src_end = dur;
            }
            TokenKind::Gap => {
                let cur = tok.duration();
                if dur >= cur {
                    let extra = dur - cur;
                    if extra > 1e-6 {
                        let blank = Token::new(TokenKind::Blank, "", tok.asset, 0.0, extra);
                        self.script.insert(idx + 1, blank);
                    }
                } else if dur <= 1e-6 {
                    self.delete_range(idx..idx + 1);
                } else {
                    let half = dur / 2.0;
                    let mut head = tok.clone();
                    head.src_end = tok.src_start + half;
                    let mut tail = Token::new(TokenKind::Gap, "", tok.asset, tok.src_end - half, tok.src_end);
                    tail.speaker = tok.speaker;
                    self.script.splice(idx..idx + 1, [head, tail]);
                }
            }
            TokenKind::Word => {}
        }
    }

    /// Shortens every run of consecutive gap tokens longer than `longer_than`
    /// (and, if given, shorter than `shorter_than`) to `target`. Returns how
    /// many gaps were changed.
    pub fn shorten_gaps(&mut self, longer_than: Secs, shorter_than: Option<Secs>, target: Secs) -> usize {
        let mut changed = 0;
        let mut i = 0;
        while i < self.script.len() {
            if self.script[i].ignored || self.script[i].kind == TokenKind::Word {
                i += 1;
                continue;
            }
            // Run of silence tokens.
            let start = i;
            let mut total = 0.0;
            while i < self.script.len() && !self.script[i].ignored && self.script[i].kind != TokenKind::Word {
                total += self.script[i].duration();
                i += 1;
            }
            let at_edge = start == 0 || i == self.script.len();
            let in_window = total > longer_than + 1e-9 && shorter_than.is_none_or(|m| total < m);
            if !in_window || total <= target + 1e-9 || at_edge {
                continue;
            }
            // Keep `target/2` from the head of the first token and the tail of the last.
            let half = target / 2.0;
            let first = self.script[start].clone();
            let last = self.script[i - 1].clone();
            let mut repl: Vec<Token> = Vec::new();
            if first.kind == TokenKind::Gap && last.kind == TokenKind::Gap && first.duration() >= half && last.duration() >= half {
                let mut head = first.clone();
                head.src_end = first.src_start + half;
                let mut tail = Token::new(TokenKind::Gap, "", last.asset, last.src_end - half, last.src_end);
                tail.speaker = last.speaker;
                repl.push(head);
                repl.push(tail);
            } else {
                let mut blank = Token::new(TokenKind::Blank, "", first.asset, 0.0, target);
                blank.paragraph_start = first.paragraph_start;
                blank.scene_start = first.scene_start;
                blank.marker = first.marker.clone();
                repl.push(blank);
            }
            let n = repl.len();
            self.script.splice(start..i, repl);
            i = start + n;
            changed += 1;
        }
        self.sync_scene_settings();
        changed
    }

    /// Splits token `idx` at composition time `t` if `t` falls strictly inside
    /// it. Returns the index of the token that now starts at `t`.
    pub fn split_token_at(&mut self, t: Secs) -> usize {
        let starts = self.token_starts();
        for i in 0..self.script.len() {
            let tok = &self.script[i];
            let d = tok.play_duration();
            if d <= 0.0 {
                continue;
            }
            let (s, e) = (starts[i], starts[i] + d);
            if (t - s).abs() < 1e-6 {
                return i;
            }
            if t > s && t < e - 1e-6 {
                let cut = tok.src_start + (t - s);
                let mut right = tok.clone();
                right.src_start = cut;
                right.paragraph_start = false;
                right.scene_start = false;
                right.marker = None;
                right.note = None;
                if tok.kind == TokenKind::Word {
                    // The text stays with the left half; the right half is silence-like media.
                    right.kind = TokenKind::Gap;
                    right.text.clear();
                }
                self.script[i].src_end = cut;
                self.script.insert(i + 1, right);
                return i + 1;
            }
        }
        self.script.len()
    }

    /// Trims the source range of the clip edge at a cut. `idx` is an edit
    /// boundary (first token after the cut). Positive `delta` extends the
    /// outgoing side later / the incoming side earlier is expressed by `side`.
    pub fn trim_boundary(&mut self, idx: usize, side: TrimSide, delta: Secs, asset_duration: Secs) {
        match side {
            TrimSide::Outgoing => {
                // Find the last playing token before idx.
                if let Some(i) = (0..idx.min(self.script.len())).rev().find(|&i| !self.script[i].ignored) {
                    let t = &mut self.script[i];
                    if t.kind == TokenKind::Blank {
                        t.src_end = (t.src_end + delta).max(t.src_start + 0.01);
                    } else {
                        t.src_end = (t.src_end + delta).clamp(t.src_start + 0.01, asset_duration.max(t.src_start + 0.01));
                    }
                }
            }
            TrimSide::Incoming => {
                if let Some(i) = (idx..self.script.len()).find(|&i| !self.script[i].ignored) {
                    let t = &mut self.script[i];
                    if t.kind == TokenKind::Blank {
                        t.src_end = (t.src_end - delta).max(t.src_start + 0.01);
                    } else {
                        t.src_start = (t.src_start + delta).clamp(0.0, t.src_end - 0.01);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TrimSide {
    Outgoing,
    Incoming,
}

/// Replaces the alphanumeric core of `original` with `to`, keeping leading and
/// trailing punctuation and matching leading capitalisation.
fn replace_core(original: &str, to: &str) -> String {
    let is_core = |c: char| c.is_alphanumeric() || c == '\'' || c == '\u{2019}';
    let start = original.find(is_core).unwrap_or(0);
    let end = original.rfind(is_core).map(|i| i + original[i..].chars().next().unwrap().len_utf8()).unwrap_or(original.len());
    let core = &original[start..end];
    let mut to = to.to_string();
    if core.chars().next().is_some_and(char::is_uppercase) && to.chars().next().is_some_and(char::is_lowercase) && core.chars().skip(1).any(char::is_lowercase) {
        let mut c = to.chars();
        to = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
    }
    format!("{}{}{}", &original[..start], to, &original[end..])
}

pub fn clamp_range(r: &Range<usize>, len: usize) -> Range<usize> {
    let s = r.start.min(len);
    let e = r.end.clamp(s, len);
    s..e
}

// ---------------------------------------------------------------------------
// Restore
// ---------------------------------------------------------------------------

/// Rebuilds the tokens that were cut between the playing tokens either side
/// of edit boundary `idx`, from the asset's full token list. Returns `None`
/// when the two sides come from different assets or nothing is missing.
pub fn restore_at_boundary(project: &Project, comp: &Composition, idx: usize) -> Option<Vec<Token>> {
    let prev = (0..idx.min(comp.script.len())).rev().map(|i| &comp.script[i]).find(|t| !t.ignored)?;
    let next = comp.script.get(idx)?;
    if prev.kind == TokenKind::Blank || next.kind == TokenKind::Blank || prev.asset != next.asset {
        return None;
    }
    if next.src_start - prev.src_end <= CONTIGUITY_EPS {
        return None;
    }
    let asset = project.asset(prev.asset)?;
    let all = tokens_from_asset(asset);
    let mut out: Vec<Token> = Vec::new();
    for mut t in all {
        let s = t.src_start.max(prev.src_end);
        let e = t.src_end.min(next.src_start);
        if e - s <= 1e-6 {
            continue;
        }
        // Keep a word only if most of it lies in the missing range.
        if t.kind == TokenKind::Word && (e - s) < t.duration() * 0.5 {
            t.kind = TokenKind::Gap;
            t.text.clear();
        }
        t.src_start = s;
        t.src_end = e;
        t.paragraph_start = false;
        out.push(t);
    }
    (!out.is_empty()).then_some(out)
}

// ---------------------------------------------------------------------------
// Find
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FindOptions {
    #[serde(default)]
    pub match_case: bool,
    #[serde(default)]
    pub whole_words: bool,
    #[serde(default)]
    pub include_ignored: bool,
}

/// Finds a phrase in the script. Matching is across word tokens, ignoring
/// gaps and punctuation. Returns token ranges (inclusive of interior gaps).
pub fn find(comp: &Composition, query: &str, opts: &FindOptions) -> Vec<Range<usize>> {
    let norm = |s: &str| -> String {
        let n: String = s.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').replace('\u{2019}', "'");
        if opts.match_case { n } else { n.to_lowercase() }
    };
    let q: Vec<String> = query.split_whitespace().map(norm).filter(|s| !s.is_empty()).collect();
    if q.is_empty() {
        return Vec::new();
    }
    let words: Vec<(usize, String)> = comp
        .script
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TokenKind::Word && !t.hidden_text && (opts.include_ignored || !t.ignored))
        .map(|(i, t)| (i, norm(&t.text)))
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + q.len() <= words.len() {
        let hit = q.iter().enumerate().all(|(k, qw)| {
            let w = &words[i + k].1;
            if opts.whole_words || q.len() > 1 {
                // In a multi-word phrase the first word may match as a suffix
                // and the last as a prefix unless whole words are required.
                if opts.whole_words {
                    w == qw
                } else if k == 0 && q.len() > 1 {
                    w.ends_with(qw.as_str())
                } else if k + 1 == q.len() {
                    w.starts_with(qw.as_str())
                } else {
                    w == qw
                }
            } else {
                w.contains(qw.as_str())
            }
        });
        if hit {
            out.push(words[i].0..words[i + q.len() - 1].0 + 1);
            i += q.len();
        } else {
            i += 1;
        }
    }
    out
}
