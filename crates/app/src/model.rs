//! App behaviour, independent of the UI toolkit: selection, caret, clipboard,
//! playhead, find, and the user-level actions that turn into engine commands.
//! Views call these methods and repaint; tests call them directly.

use anyhow::{Result, anyhow, bail};
use mango_core::detect::{self, FillerHit, FillerOptions, RetakeHit};
use mango_core::editor::RemoveMode;
use mango_core::script::{self, FindOptions, TrimSide};
use mango_core::*;
use std::ops::Range;

/// Silences at least this long are shown in the script as a gap pill.
pub const VISIBLE_GAP: Secs = 0.75;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Range,
    Blade,
    Hand,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidePanel {
    Project,
    AiTools,
    Assistant,
    Comments,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectorTab {
    Composition,
    Layer,
    Captions,
    Audio,
    Scene,
}

/// A transient message for the status line.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub error: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FindState {
    pub query: String,
    pub options: FindOptions,
    pub hits: Vec<Range<usize>>,
    pub current: usize,
}

pub struct AppModel {
    pub editor: Editor,
    /// Selection in token indices. `anchor == head` and `!has_range` means a
    /// caret sitting on token `head`.
    pub anchor: usize,
    pub head: usize,
    pub has_range: bool,
    pub playhead: Secs,
    pub playing: bool,
    pub rate: f32,
    pub clipboard: Vec<Token>,
    pub selected_layer: Option<LayerId>,
    pub tool: Tool,
    pub side_panel: SidePanel,
    pub inspector: InspectorTab,
    pub show_timeline: bool,
    /// Timeline zoom: pixels per second.
    pub px_per_sec: f64,
    pub timeline_scroll: f64,
    pub find: Option<FindState>,
    pub toast: Option<Toast>,
    pub script_font_px: f32,
    pub follow_playhead: bool,
    pub author: String,
    /// The layer as it was before an in-progress drag, if any.
    live_origin: Option<Layer>,
}

impl AppModel {
    pub fn new(editor: Editor) -> Self {
        Self {
            editor,
            anchor: 0,
            head: 0,
            has_range: false,
            playhead: 0.0,
            playing: false,
            rate: 1.0,
            clipboard: Vec::new(),
            selected_layer: None,
            tool: Tool::Select,
            side_panel: SidePanel::Project,
            inspector: InspectorTab::Composition,
            show_timeline: true,
            px_per_sec: 80.0,
            timeline_scroll: 0.0,
            find: None,
            toast: None,
            script_font_px: 17.0,
            follow_playhead: true,
            live_origin: None,
            author: std::env::var("USER").unwrap_or_else(|_| "Me".into()),
        }
    }

    pub fn comp(&self) -> Option<&Composition> {
        self.editor.comp()
    }

    fn script_len(&self) -> usize {
        self.comp().map(|c| c.script.len()).unwrap_or(0)
    }

    pub fn duration(&self) -> Secs {
        self.comp().map(|c| c.duration()).unwrap_or(0.0)
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), error: false });
    }

    pub fn fail(&mut self, err: impl std::fmt::Display) {
        self.toast = Some(Toast { text: err.to_string(), error: true });
    }

    /// Runs a command; on failure shows the error and returns false.
    pub fn run(&mut self, cmd: Command) -> bool {
        match self.editor.apply(cmd) {
            Ok(()) => {
                self.after_edit();
                true
            }
            Err(e) => {
                self.fail(e);
                false
            }
        }
    }

    /// Keeps selection, playhead and find results valid after any change.
    pub fn after_edit(&mut self) {
        let len = self.script_len();
        self.anchor = self.anchor.min(len);
        self.head = self.head.min(len);
        if self.anchor == self.head && len == 0 {
            self.has_range = false;
        }
        self.playhead = self.playhead.clamp(0.0, self.duration());
        if let Some(id) = self.selected_layer {
            if self.comp().and_then(|c| c.layer(id)).is_none() {
                self.selected_layer = None;
            }
        }
        if self.find.is_some() {
            self.refresh_find();
        }
    }

    // --- visibility -----------------------------------------------------------

    /// Whether a token is drawn in the script (words always; silences only
    /// when long, and gap clips always).
    pub fn is_visible(tok: &Token) -> bool {
        match tok.kind {
            TokenKind::Word => !tok.hidden_text,
            TokenKind::Blank => true,
            TokenKind::Gap => tok.duration() >= VISIBLE_GAP,
        }
    }

    fn next_visible(&self, from: usize) -> Option<usize> {
        let c = self.comp()?;
        (from..c.script.len()).find(|&i| Self::is_visible(&c.script[i]))
    }

    fn prev_visible(&self, before: usize) -> Option<usize> {
        let c = self.comp()?;
        (0..before.min(c.script.len())).rev().find(|&i| Self::is_visible(&c.script[i]))
    }

    // --- selection ------------------------------------------------------------

    /// The selected token range. With only a caret this is empty at the caret.
    pub fn selection(&self) -> Range<usize> {
        if self.has_range {
            let (a, b) = (self.anchor.min(self.head), self.anchor.max(self.head));
            a..(b + 1).min(self.script_len())
        } else {
            self.head..self.head
        }
    }

    /// The selection, or the token under the caret if nothing is selected.
    pub fn target(&self) -> Range<usize> {
        let r = self.selection();
        if !r.is_empty() {
            return r;
        }
        if self.head < self.script_len() { self.head..self.head + 1 } else { r }
    }

    pub fn is_selected(&self, idx: usize) -> bool {
        self.has_range && self.selection().contains(&idx)
    }

    /// Click on a token: places the caret there (and seeks), or extends the
    /// selection when `extend` is set.
    pub fn click_token(&mut self, idx: usize, extend: bool) {
        let idx = idx.min(self.script_len().saturating_sub(1));
        if extend {
            self.head = idx;
            self.has_range = true;
        } else {
            self.anchor = idx;
            self.head = idx;
            self.has_range = false;
            self.seek_to_token(idx);
        }
        self.selected_layer = None;
    }

    /// Drag across tokens while the mouse is held.
    pub fn drag_to_token(&mut self, idx: usize) {
        let idx = idx.min(self.script_len().saturating_sub(1));
        if idx != self.anchor || self.has_range {
            self.head = idx;
            self.has_range = true;
        }
    }

    /// Double-click: selects the whole sentence around the token.
    pub fn select_sentence(&mut self, idx: usize) {
        let Some(c) = self.comp() else { return };
        if c.script.is_empty() {
            return;
        }
        let idx = idx.min(c.script.len() - 1);
        let mut a = idx;
        while a > 0 {
            let prev = &c.script[a - 1];
            if c.script[a].paragraph_start || (prev.is_word() && script::ends_sentence(&prev.text)) {
                break;
            }
            a -= 1;
        }
        let mut b = idx;
        while b + 1 < c.script.len() {
            if (c.script[b].is_word() && script::ends_sentence(&c.script[b].text)) || c.script[b + 1].paragraph_start {
                break;
            }
            b += 1;
        }
        self.anchor = a;
        self.head = b;
        self.has_range = true;
    }

    /// Triple-click: selects the paragraph.
    pub fn select_paragraph(&mut self, idx: usize) {
        let Some(c) = self.comp() else { return };
        if let Some(p) = c.paragraphs().into_iter().find(|p| p.contains(&idx)) {
            self.anchor = p.start;
            self.head = p.end - 1;
            self.has_range = true;
        }
    }

    pub fn select_all(&mut self) {
        let n = self.script_len();
        if n > 0 {
            self.anchor = 0;
            self.head = n - 1;
            self.has_range = true;
        }
    }

    pub fn select_range(&mut self, r: Range<usize>) {
        if r.is_empty() {
            self.anchor = r.start;
            self.head = r.start;
            self.has_range = false;
        } else {
            self.anchor = r.start;
            self.head = r.end - 1;
            self.has_range = true;
        }
    }

    pub fn clear_selection(&mut self) {
        self.anchor = self.head;
        self.has_range = false;
    }

    /// Moves the caret one visible token left (-1) or right (+1).
    pub fn move_caret(&mut self, dir: i32, extend: bool) {
        let cur = self.head;
        let next = if dir < 0 { self.prev_visible(cur) } else { self.next_visible(cur + 1) };
        let Some(next) = next else { return };
        if extend {
            self.head = next;
            self.has_range = true;
        } else {
            self.anchor = next;
            self.head = next;
            self.has_range = false;
            self.seek_to_token(next);
        }
    }

    /// Moves the caret to the previous/next paragraph start.
    pub fn move_paragraph(&mut self, dir: i32, extend: bool) {
        let Some(c) = self.comp() else { return };
        let paras = c.paragraphs();
        let target = if dir < 0 {
            paras.iter().rev().map(|p| p.start).find(|s| *s < self.head)
        } else {
            paras.iter().map(|p| p.start).find(|s| *s > self.head)
        };
        let Some(t) = target else { return };
        let t = self.next_visible(t).unwrap_or(t);
        if extend {
            self.head = t;
            self.has_range = true;
        } else {
            self.anchor = t;
            self.head = t;
            self.has_range = false;
            self.seek_to_token(t);
        }
    }

    // --- transport ------------------------------------------------------------

    pub fn seek(&mut self, t: Secs) {
        self.playhead = t.clamp(0.0, self.duration());
    }

    pub fn seek_to_token(&mut self, idx: usize) {
        if let Some(c) = self.comp() {
            // An ignored token has no time of its own; land on the next playing one.
            self.playhead = c.time_of_token(idx).min(c.duration());
        }
    }

    pub fn toggle_play(&mut self) {
        if !self.playing && self.playhead >= self.duration() - 1e-3 {
            self.playhead = 0.0;
        }
        self.playing = !self.playing && self.duration() > 0.0;
    }

    /// Advances the playhead by `dt` wall seconds when no audio clock drives
    /// it. Returns true while still playing.
    pub fn tick(&mut self, dt: Secs) -> bool {
        if !self.playing {
            return false;
        }
        self.set_playhead_from_clock(self.playhead + dt * self.rate as f64);
        self.playing
    }

    /// Sets the playhead from the playback clock; stops at the end.
    pub fn set_playhead_from_clock(&mut self, t: Secs) {
        let d = self.duration();
        if t >= d {
            self.playhead = d;
            self.playing = false;
        } else {
            self.playhead = t.max(0.0);
        }
    }

    /// The word being spoken at the playhead.
    pub fn playing_token(&self) -> Option<usize> {
        self.comp().and_then(|c| c.token_at_time(self.playhead))
    }

    pub fn step_frames(&mut self, n: i64) {
        let fps = self.comp().map(|c| c.fps).unwrap_or(30.0);
        let frame = (self.playhead * fps).round() as i64 + n;
        self.seek(frame.max(0) as f64 / fps);
    }

    pub fn set_rate(&mut self, rate: f32) {
        self.rate = rate.clamp(0.5, 3.0);
    }

    pub fn jump_scene(&mut self, dir: i32) {
        let Some(c) = self.comp() else { return };
        let spans = c.scene_spans();
        let t = self.playhead;
        let target = if dir < 0 {
            spans.iter().rev().map(|s| s.start).find(|s| *s < t - 0.05).unwrap_or(0.0)
        } else {
            spans.iter().map(|s| s.start).find(|s| *s > t + 1e-6).unwrap_or(c.duration())
        };
        self.seek(target);
    }

    pub fn jump_marker(&mut self, dir: i32) {
        let Some(c) = self.comp() else { return };
        let times: Vec<Secs> = c.script_markers().into_iter().map(|m| m.1).collect();
        let t = self.playhead;
        let target = if dir < 0 { times.iter().rev().copied().find(|s| *s < t - 0.05) } else { times.iter().copied().find(|s| *s > t + 1e-6) };
        if let Some(t) = target {
            self.seek(t);
        }
    }

    /// Parses "90", "1:30", "1:02:03", "1:30.5" into seconds and seeks.
    pub fn jump_to_time(&mut self, text: &str) -> Result<()> {
        let t = parse_time(text).ok_or_else(|| anyhow!("\"{}\" is not a time. Try 1:30 or 90", text.trim()))?;
        if t > self.duration() + 1e-6 {
            bail!("the composition is only {} long", format_time(self.duration()));
        }
        self.seek(t);
        Ok(())
    }

    // --- script edits ---------------------------------------------------------

    /// Delete key. With a selection deletes it; otherwise the token under the
    /// caret (`forward`) or the visible token before it (Backspace).
    pub fn delete(&mut self, forward: bool) -> bool {
        let r = self.selection();
        let r = if !r.is_empty() {
            r
        } else if forward {
            self.target()
        } else {
            match self.prev_visible(self.head) {
                Some(p) => p..p + 1,
                None => return false,
            }
        };
        if r.is_empty() {
            return false;
        }
        let ok = self.run(Command::DeleteRange { start: r.start, end: r.end });
        if ok {
            let at = self.next_visible(r.start).or_else(|| self.prev_visible(r.start)).unwrap_or(0);
            self.select_range(at..at);
            self.seek_to_token(at);
        }
        ok
    }

    /// Strikes the selection through, or restores it if it is all ignored.
    pub fn toggle_ignore(&mut self) -> bool {
        let r = self.target();
        if r.is_empty() {
            return false;
        }
        let all_ignored = self.comp().is_some_and(|c| c.script[r.clone()].iter().all(|t| t.ignored));
        self.run(Command::IgnoreRange { start: r.start, end: r.end, ignored: !all_ignored })
    }

    pub fn replace_with_gap(&mut self) -> bool {
        let r = self.target();
        if r.is_empty() {
            return false;
        }
        let ok = self.run(Command::ReplaceWithGap { start: r.start, end: r.end });
        if ok {
            self.select_range(r.start..r.start);
        }
        ok
    }

    /// Restores media cut inside the selection, or at the caret's boundary.
    pub fn restore(&mut self) -> bool {
        let r = self.selection();
        if !r.is_empty() {
            return self.run(Command::RestoreRange { start: r.start, end: r.end });
        }
        // Nearest edit boundary at or after the caret, else just before it.
        let Some(c) = self.comp() else { return false };
        let b = c.edit_boundaries();
        let at = b.iter().copied().find(|i| *i >= self.head).or_else(|| b.iter().rev().copied().find(|i| *i < self.head));
        match at {
            Some(at) => self.run(Command::RestoreAt { at }),
            None => {
                self.fail("Nothing has been cut here");
                false
            }
        }
    }

    pub fn copy(&mut self) -> Option<String> {
        let r = self.target();
        if r.is_empty() {
            return None;
        }
        let c = self.comp()?;
        let (tokens, text) = (c.script[r.clone()].to_vec(), c.text_of(r));
        self.clipboard = tokens;
        Some(text)
    }

    pub fn cut(&mut self) -> Option<String> {
        let text = self.copy()?;
        let r = self.target();
        if self.run(Command::DeleteRange { start: r.start, end: r.end }) {
            self.select_range(r.start..r.start);
        }
        Some(text)
    }

    /// Pastes the internal clipboard before the caret (replacing a selection).
    pub fn paste(&mut self) -> bool {
        if self.clipboard.is_empty() {
            self.fail("Nothing to paste. Copy part of a script first");
            return false;
        }
        let r = self.selection();
        let mut tokens = self.clipboard.clone();
        for t in &mut tokens {
            t.scene_start = false;
            t.marker = None;
        }
        if let Some(f) = tokens.first_mut() {
            f.paragraph_start = false;
        }
        let n = tokens.len();
        let mut cmds = Vec::new();
        if !r.is_empty() {
            cmds.push(Command::DeleteRange { start: r.start, end: r.end });
        }
        cmds.push(Command::PasteTokens { at: r.start, tokens });
        match self.editor.apply_all("Paste", cmds) {
            Ok(()) => {
                self.after_edit();
                self.select_range(r.start..r.start + n);
                true
            }
            Err(e) => {
                self.fail(e);
                false
            }
        }
    }

    /// Applies a text correction to the selection (or the caret's word).
    pub fn correct(&mut self, text: &str) -> bool {
        let r = self.target();
        if r.is_empty() {
            return false;
        }
        self.run(Command::CorrectRange { start: r.start, end: r.end, text: text.to_string() })
    }

    /// The text a "correct" box should be pre-filled with.
    pub fn correction_seed(&self) -> String {
        self.comp().map(|c| c.text_of(self.target()).replace('\n', " ")).unwrap_or_default()
    }

    pub fn toggle_paragraph_break(&mut self) -> bool {
        let at = self.head;
        let on = self.comp().and_then(|c| c.script.get(at)).is_some_and(|t| t.paragraph_start);
        self.run(Command::SetParagraphBreak { at, on: !on })
    }

    pub fn toggle_scene_break(&mut self) -> bool {
        let at = self.head;
        if at == 0 {
            self.fail("The first scene always starts at the beginning");
            return false;
        }
        let on = self.comp().and_then(|c| c.script.get(at)).is_some_and(|t| t.scene_start);
        self.run(Command::SetSceneBreak { at, on: !on })
    }

    pub fn set_marker(&mut self, name: Option<String>) -> bool {
        self.run(Command::SetMarker { at: self.head, name })
    }

    pub fn set_speaker(&mut self, name: Option<String>) -> bool {
        let r = self.selection();
        // No selection: the caret's paragraph.
        let r = if r.is_empty() {
            match self.comp().and_then(|c| c.paragraphs().into_iter().find(|p| p.contains(&self.head))) {
                Some(p) => p,
                None => return false,
            }
        } else {
            r
        };
        self.run(Command::SetSpeaker { start: r.start, end: r.end, speaker: name })
    }

    pub fn highlight(&mut self, color: Option<u8>) -> bool {
        let r = self.target();
        !r.is_empty() && self.run(Command::SetHighlight { start: r.start, end: r.end, color })
    }

    pub fn toggle_bold(&mut self) -> bool {
        let r = self.target();
        if r.is_empty() {
            return false;
        }
        let all = self.comp().is_some_and(|c| c.script[r.clone()].iter().filter(|t| t.is_word()).all(|t| t.bold));
        self.run(Command::SetStyle { start: r.start, end: r.end, bold: Some(!all), italic: None })
    }

    pub fn toggle_italic(&mut self) -> bool {
        let r = self.target();
        if r.is_empty() {
            return false;
        }
        let all = self.comp().is_some_and(|c| c.script[r.clone()].iter().filter(|t| t.is_word()).all(|t| t.italic));
        self.run(Command::SetStyle { start: r.start, end: r.end, bold: None, italic: Some(!all) })
    }

    pub fn hide_text(&mut self) -> bool {
        let r = self.target();
        !r.is_empty() && self.run(Command::HideText { start: r.start, end: r.end, hidden: true })
    }

    pub fn duplicate_to_new_composition(&mut self, name: &str) -> bool {
        let r = self.selection();
        if r.is_empty() {
            self.fail("Select the part of the script to duplicate");
            return false;
        }
        let ok = self.run(Command::CompositionFromRange { start: r.start, end: r.end, name: name.to_string() });
        if ok {
            self.reset_view();
        }
        ok
    }

    pub fn add_comment(&mut self, text: &str) -> bool {
        let time = if self.has_range { self.comp().map(|c| c.time_of_token(self.selection().start)).unwrap_or(self.playhead) } else { self.playhead };
        self.run(Command::AddComment { time, author: self.author.clone(), text: text.to_string() })
    }

    pub fn undo(&mut self) -> bool {
        let label = self.editor.undo_label().map(str::to_string);
        let ok = self.editor.undo();
        if ok {
            self.after_edit();
            self.has_range = false;
            self.anchor = self.head;
            self.say(format!("Undid {}", label.unwrap_or_default().to_lowercase()));
        }
        ok
    }

    pub fn redo(&mut self) -> bool {
        let ok = self.editor.redo();
        if ok {
            self.after_edit();
            self.has_range = false;
            self.anchor = self.head;
        }
        ok
    }

    /// Resets caret and playhead after switching composition.
    pub fn reset_view(&mut self) {
        self.anchor = 0;
        self.head = 0;
        self.has_range = false;
        self.playhead = 0.0;
        self.playing = false;
        self.selected_layer = None;
        self.timeline_scroll = 0.0;
        self.find = None;
    }

    pub fn select_composition(&mut self, id: CompositionId) -> bool {
        if self.editor.active == Some(id) {
            return true;
        }
        // Switching is navigation, not an edit: no undo step.
        if self.editor.project.composition(id).is_none() {
            return false;
        }
        self.editor.active = Some(id);
        self.editor.revision += 1;
        self.reset_view();
        true
    }

    // --- timeline -------------------------------------------------------------

    /// Split at the playhead: the selected layer if one is selected, otherwise
    /// the script (adding a scene boundary there).
    pub fn split_at_playhead(&mut self) -> bool {
        let t = self.playhead;
        if let Some(id) = self.selected_layer {
            return self.run(Command::SplitLayer { id, time: t });
        }
        let before = self.editor.revision;
        if let Err(e) = self.editor.apply(Command::SplitAt { time: t }) {
            self.fail(e);
            return false;
        }
        let Some(idx) = self.comp().map(|c| {
            let starts = c.token_starts();
            (0..c.script.len()).find(|&i| !c.script[i].ignored && (starts[i] - t).abs() < 1e-4).unwrap_or(c.script.len())
        }) else {
            return false;
        };
        if idx == 0 || idx >= self.script_len() {
            self.fail("Move the playhead inside the script to split it");
            return false;
        }
        let ok = self.run(Command::SetSceneBreak { at: idx, on: true });
        ok || self.editor.revision != before
    }

    pub fn trim_cut(&mut self, boundary: usize, side: TrimSide, delta: Secs) -> bool {
        self.run(Command::TrimBoundary { at: boundary, side, delta })
    }

    pub fn zoom(&mut self, factor: f64) {
        self.px_per_sec = (self.px_per_sec * factor).clamp(2.0, 2000.0);
    }

    /// Zoom so the whole composition fits `width` pixels.
    pub fn zoom_to_fit(&mut self, width: f64) {
        let d = self.duration().max(1.0);
        self.px_per_sec = (width.max(100.0) / d).clamp(2.0, 2000.0);
        self.timeline_scroll = 0.0;
    }

    // --- layers ---------------------------------------------------------------

    pub fn layer(&self) -> Option<&Layer> {
        self.selected_layer.and_then(|id| self.comp()?.layer(id))
    }

    /// Default length for a new layer: the selection's span, else to the end
    /// of the current scene, else 5 seconds.
    fn new_layer_span(&self) -> (Secs, Secs) {
        let Some(c) = self.comp() else { return (0.0, 5.0) };
        let r = self.selection();
        if !r.is_empty() {
            let s = c.time_of_token(r.start);
            let e = c.time_of_token(r.end);
            if e - s > 0.05 {
                return (s, e - s);
            }
        }
        let t = self.playhead.min(c.duration());
        let scene_end = c.scene_spans().into_iter().find(|s| t >= s.start && t < s.end).map(|s| s.end);
        let d = match scene_end {
            Some(e) if e - t >= 0.5 => (e - t).min(5.0),
            _ => 5.0,
        };
        (t, d)
    }

    fn top_track(&self) -> i32 {
        self.comp().map(|c| c.layers.iter().map(|l| l.track).max().unwrap_or(0)).unwrap_or(0)
    }

    pub fn add_layer(&mut self, name: &str, content: LayerContent, transform: Transform) -> Option<LayerId> {
        if self.comp().is_none() {
            self.fail("Open a composition first");
            return None;
        }
        let (start, dur) = self.new_layer_span();
        let mut layer = Layer::new(name, start, dur, content);
        layer.track = (self.top_track() + 1).min(99);
        layer.transform = transform;
        let id = layer.id;
        if self.run(Command::AddLayer { layer }) {
            self.selected_layer = Some(id);
            self.inspector = InspectorTab::Layer;
            Some(id)
        } else {
            None
        }
    }

    pub fn add_text(&mut self, text: &str) -> Option<LayerId> {
        let style = TextStyle { text: text.to_string(), ..Default::default() };
        self.add_layer("Text", LayerContent::Text(style), Transform { x: 0.5, y: 0.2, w: 0.8, h: 0.2, ..Default::default() })
    }

    pub fn add_shape(&mut self, kind: ShapeKind) -> Option<LayerId> {
        let shape = Shape { kind, fill: Rgba::rgb(0xFF, 0x8A, 0x00), stroke: None, corner_radius: 0.0 };
        let name = match kind {
            ShapeKind::Rectangle => "Rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Line => "Line",
        };
        let tr = match kind {
            ShapeKind::Line => Transform { x: 0.5, y: 0.5, w: 0.4, h: 0.01, ..Default::default() },
            _ => Transform { x: 0.5, y: 0.5, w: 0.3, h: 0.3, ..Default::default() },
        };
        self.add_layer(name, LayerContent::Shape(shape), tr)
    }

    pub fn add_waveform(&mut self) -> Option<LayerId> {
        let st = WaveformStyle { kind: WaveformKind::Bars, color: Rgba::WHITE };
        let id = self.add_layer("Waveform", LayerContent::Waveform(st), Transform { x: 0.5, y: 0.75, w: 0.8, h: 0.2, ..Default::default() })?;
        self.stretch_layer_to_composition(id);
        Some(id)
    }

    pub fn add_progress_bar(&mut self) -> Option<LayerId> {
        let content = LayerContent::Progress { color: Rgba::rgb(0xFF, 0x8A, 0x00), track_color: Rgba::rgba(255, 255, 255, 60) };
        let id = self.add_layer("Progress bar", content, Transform { x: 0.5, y: 0.97, w: 1.0, h: 0.015, ..Default::default() })?;
        self.stretch_layer_to_composition(id);
        Some(id)
    }

    fn stretch_layer_to_composition(&mut self, id: LayerId) {
        let Some(c) = self.comp() else { return };
        let total = c.script_duration().max(1.0);
        if let Some(mut l) = c.layer(id).cloned() {
            l.start = 0.0;
            l.duration = total;
            // Fold into the add step so undo removes the layer in one go.
            if let Some(c) = self.editor.comp_mut() {
                if let Some(slot) = c.layer_mut(id) {
                    *slot = l;
                }
            }
        }
    }

    /// Adds a project file as a layer at the playhead.
    pub fn add_asset_layer(&mut self, asset: AssetId) -> Option<LayerId> {
        let a = self.editor.project.asset(asset)?.clone();
        let (start, span) = self.new_layer_span();
        let (content, dur, tr) = match a.kind {
            MediaKind::Image => (
                LayerContent::Image(ImageClip { asset, crop: Crop::default(), fit: FitMode::Fit, effects: Vec::new(), zoom_to: 1.0 }),
                span,
                Transform { x: 0.5, y: 0.5, w: 0.5, h: 0.5, ..Default::default() },
            ),
            MediaKind::Audio => (LayerContent::Audio(MediaClip::new(asset)), a.duration.max(0.1), Transform::default()),
            MediaKind::Video => (LayerContent::Video(MediaClip::new(asset)), a.duration.max(0.1), Transform { x: 0.75, y: 0.75, w: 0.4, h: 0.4, ..Default::default() }),
        };
        let mut layer = Layer::new(a.name.clone(), start, dur, content);
        layer.track = (self.top_track() + 1).min(99);
        layer.transform = tr;
        let id = layer.id;
        if self.run(Command::AddLayer { layer }) {
            self.selected_layer = Some(id);
            self.inspector = InspectorTab::Layer;
            Some(id)
        } else {
            None
        }
    }

    /// Applies a change to the selected layer as one undo step.
    pub fn edit_layer(&mut self, f: impl FnOnce(&mut Layer)) -> bool {
        let Some(mut l) = self.layer().cloned() else { return false };
        if l.locked {
            self.fail("This layer is locked");
            return false;
        }
        f(&mut l);
        self.run(Command::UpdateLayer { layer: l })
    }

    pub fn delete_layer(&mut self) -> bool {
        let Some(id) = self.selected_layer else { return false };
        let ok = self.run(Command::RemoveLayer { id });
        if ok {
            self.selected_layer = None;
        }
        ok
    }

    pub fn duplicate_layer(&mut self) -> bool {
        let Some(id) = self.selected_layer else { return false };
        let ok = self.run(Command::DuplicateLayer { id });
        if ok {
            self.selected_layer = self.comp().and_then(|c| c.layers.last()).map(|l| l.id);
        }
        ok
    }

    /// Moves a layer in time (drag on the timeline), clamped at zero.
    pub fn move_layer(&mut self, id: LayerId, new_start: Secs) -> bool {
        let Some(mut l) = self.comp().and_then(|c| c.layer(id)).cloned() else { return false };
        if l.locked {
            return false;
        }
        l.start = new_start.max(0.0);
        self.run(Command::UpdateLayer { layer: l })
    }

    /// Trims a layer edge. `left` moves the start (and the source in-point for
    /// media); otherwise the end.
    pub fn trim_layer(&mut self, id: LayerId, left: bool, delta: Secs) -> bool {
        let Some(mut l) = self.comp().and_then(|c| c.layer(id)).cloned() else { return false };
        if l.locked {
            return false;
        }
        apply_trim(&mut l, left, delta);
        self.run(Command::UpdateLayer { layer: l })
    }

    /// Changes a layer in place during a drag, without an undo step per mouse
    /// move. Call [`commit_live_edit`](Self::commit_live_edit) on release.
    pub fn live_edit_layer(&mut self, id: LayerId, f: impl FnOnce(&mut Layer)) -> bool {
        let Some(l) = self.editor.comp_mut().and_then(|c| c.layer_mut(id)) else { return false };
        if l.locked {
            return false;
        }
        if self.live_origin.as_ref().is_none_or(|o| o.id != id) {
            self.live_origin = Some(l.clone());
        }
        f(l);
        l.start = l.start.max(0.0);
        l.duration = l.duration.max(0.05);
        self.editor.revision += 1;
        true
    }

    pub fn live_trim(&mut self, id: LayerId, left: bool, delta: Secs) -> bool {
        self.live_edit_layer(id, |l| apply_trim(l, left, delta))
    }

    /// Turns the drag's accumulated change into a single undo step.
    pub fn commit_live_edit(&mut self) -> bool {
        let Some(orig) = self.live_origin.take() else { return false };
        let id = orig.id;
        let Some(new) = self.comp().and_then(|c| c.layer(id)).cloned() else { return false };
        if new == orig {
            return false;
        }
        if let Some(slot) = self.editor.comp_mut().and_then(|c| c.layer_mut(id)) {
            *slot = orig;
        }
        self.run(Command::UpdateLayer { layer: new })
    }

    pub fn is_live_editing(&self) -> bool {
        self.live_origin.is_some()
    }

    // --- find -----------------------------------------------------------------

    pub fn open_find(&mut self) {
        if self.find.is_none() {
            self.find = Some(FindState::default());
        }
    }

    pub fn close_find(&mut self) {
        self.find = None;
    }

    pub fn set_find_query(&mut self, q: &str) {
        let f = self.find.get_or_insert_with(FindState::default);
        f.query = q.to_string();
        f.current = 0;
        self.refresh_find();
        self.goto_find_hit();
    }

    fn refresh_find(&mut self) {
        let Some(f) = self.find.clone() else { return };
        let hits = self.comp().map(|c| script::find(c, &f.query, &f.options)).unwrap_or_default();
        if let Some(fs) = &mut self.find {
            fs.current = fs.current.min(hits.len().saturating_sub(1));
            fs.hits = hits;
        }
    }

    fn goto_find_hit(&mut self) {
        let Some(hit) = self.find.as_ref().and_then(|f| f.hits.get(f.current)).cloned() else { return };
        self.select_range(hit.clone());
        self.seek_to_token(hit.start);
    }

    pub fn find_next(&mut self, dir: i32) {
        if let Some(f) = &mut self.find {
            let n = f.hits.len();
            if n == 0 {
                return;
            }
            f.current = ((f.current as i64 + dir as i64).rem_euclid(n as i64)) as usize;
        }
        self.goto_find_hit();
    }

    /// Applies `mode` to every current find hit as one undo step.
    pub fn remove_find_hits(&mut self, mode: RemoveMode) -> bool {
        let Some(q) = self.find.as_ref().map(|f| f.query.clone()) else { return false };
        let ok = self.run(Command::RemoveMatches { query: q, mode });
        if ok {
            self.clear_selection();
        }
        ok
    }

    // --- detection ------------------------------------------------------------

    pub fn fillers(&self, opts: &FillerOptions) -> Vec<FillerHit> {
        self.comp().map(|c| detect::find_fillers(c, opts)).unwrap_or_default()
    }

    pub fn retakes(&self) -> Vec<RetakeHit> {
        self.comp().map(|c| detect::find_retakes(c, 3, 60)).unwrap_or_default()
    }

    pub fn remove_fillers(&mut self, opts: FillerOptions, mode: RemoveMode) -> bool {
        let n = self.fillers(&opts).len();
        if n == 0 {
            self.say("No filler words found");
            return false;
        }
        let ok = self.run(Command::RemoveFillers { options: opts, mode });
        if ok {
            self.clear_selection();
            self.say(format!("Removed {n} filler word{}", if n == 1 { "" } else { "s" }));
        }
        ok
    }

    pub fn remove_retakes(&mut self, mode: RemoveMode) -> bool {
        let n = self.retakes().len();
        if n == 0 {
            self.say("No retakes found");
            return false;
        }
        let ok = self.run(Command::RemoveRetakes { min_words: 3, mode });
        if ok {
            self.clear_selection();
            self.say(format!("Marked {n} retake{}", if n == 1 { "" } else { "s" }));
        }
        ok
    }

    pub fn shorten_gaps(&mut self, longer_than: Secs, target: Secs) -> bool {
        let before = self.duration();
        let ok = self.run(Command::ShortenGaps { longer_than, shorter_than: None, target });
        if ok {
            let saved = before - self.duration();
            if saved > 1e-6 {
                self.say(format!("Shortened word gaps, saving {}", format_time(saved)));
            } else {
                self.say("No word gaps longer than that");
            }
        }
        ok
    }

    // --- status ---------------------------------------------------------------

    /// "1,204 words · 6:32" for the selection, or the whole script.
    pub fn stats(&self) -> String {
        let Some(c) = self.comp() else { return String::new() };
        let r = self.selection();
        let (r, scope) = if r.is_empty() { (0..c.script.len(), "") } else { (r, " selected") };
        let words = c.word_count(r.clone());
        format!("{} word{}{} · {}", group_thousands(words), if words == 1 { "" } else { "s" }, scope, format_time(c.range_duration(r)))
    }
}

fn apply_trim(l: &mut Layer, left: bool, delta: Secs) {
    if left {
        let d = delta.clamp(-l.start, l.duration - 0.05);
        if let LayerContent::Video(m) | LayerContent::Audio(m) = &mut l.content {
            let d2 = d.max(-m.src_in / m.speed.max(0.05));
            m.src_in += d2 * m.speed;
            l.start += d2;
            l.duration -= d2;
        } else {
            l.start += d;
            l.duration -= d;
        }
    } else {
        l.duration = (l.duration + delta).max(0.05);
    }
}

pub fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `m:ss` or `h:mm:ss`.
pub fn format_time(t: Secs) -> String {
    let s = t.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// `m:ss.d` for the transport readout.
pub fn format_time_precise(t: Secs) -> String {
    let ds = (t.max(0.0) * 10.0).floor() as u64;
    let s = ds / 10;
    if s >= 3600 {
        format!("{}:{:02}:{:02}.{}", s / 3600, (s / 60) % 60, s % 60, ds % 10)
    } else {
        format!("{}:{:02}.{}", s / 60, s % 60, ds % 10)
    }
}

pub fn parse_time(text: &str) -> Option<Secs> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for (i, p) in parts.iter().enumerate() {
        let v: f64 = p.trim().parse().ok()?;
        if v < 0.0 || !v.is_finite() || (i > 0 && v >= 60.0) {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}
