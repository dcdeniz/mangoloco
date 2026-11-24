//! The editor: a project plus undo history, changed only through
//! [`Command`]s. The desktop app, the command-line tool, the assistant and
//! the tests all drive the same commands.

use crate::detect::{self, FillerOptions};
use crate::model::*;
use crate::script::{self, TrimSide};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const UNDO_LIMIT: usize = 100;

/// What to do with something a detection pass found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoveMode {
    Delete,
    Ignore,
    ReplaceWithGap,
    /// Keep the media, hide the text.
    HideText,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    // --- script -------------------------------------------------------------
    DeleteRange { start: usize, end: usize },
    IgnoreRange { start: usize, end: usize, ignored: bool },
    RemoveIgnored,
    ReplaceWithGap { start: usize, end: usize },
    InsertGap { at: usize, secs: Secs },
    MoveRange { start: usize, end: usize, dest: usize },
    CorrectRange { start: usize, end: usize, text: String },
    CorrectAll { from: String, to: String },
    HideText { start: usize, end: usize, hidden: bool },
    SetSpeaker { start: usize, end: usize, speaker: Option<String> },
    RenameSpeaker { from: String, to: String },
    SetParagraphBreak { at: usize, on: bool },
    SetSceneBreak { at: usize, on: bool },
    SetMarker { at: usize, name: Option<String> },
    SetNote { at: usize, note: Option<String> },
    SetHighlight { start: usize, end: usize, color: Option<u8> },
    SetStyle { start: usize, end: usize, bold: Option<bool>, italic: Option<bool> },
    SetGapDuration { at: usize, secs: Secs },
    ShortenGaps { longer_than: Secs, shorter_than: Option<Secs>, target: Secs },
    /// Restore media cut at the edit boundary before token `at`.
    RestoreAt { at: usize },
    /// Restore everything cut inside the range and un-ignore it.
    RestoreRange { start: usize, end: usize },
    /// Append or insert a whole asset into the script.
    InsertAsset { asset: AssetId, at: Option<usize> },
    PasteTokens { at: usize, tokens: Vec<Token> },
    SplitAt { time: Secs },
    TrimBoundary { at: usize, side: TrimSide, delta: Secs },
    RemoveFillers { options: FillerOptions, mode: RemoveMode },
    RemoveRetakes { min_words: usize, mode: RemoveMode },
    RemoveMatches { query: String, mode: RemoveMode },
    // --- composition --------------------------------------------------------
    NewComposition { name: String },
    DuplicateComposition { name: Option<String> },
    /// New composition from a token range of the active one ("duplicate to…").
    CompositionFromRange { start: usize, end: usize, name: String },
    RenameComposition { name: String },
    DeleteComposition,
    SelectComposition { id: CompositionId },
    SetCanvas { width: u32, height: u32, fill: bool },
    SetFps { fps: f64 },
    SetBackground { color: Rgba },
    SetScriptVideo { placement: VideoPlacement },
    SetScriptAudio { audio: AudioSettings },
    SetScene { scene: usize, settings: SceneSettings },
    SetAllTransitions { transition: Transition, secs: Secs },
    SetCaptions { captions: CaptionSettings },
    SetMasterLoudness { lufs: Option<f64> },
    // --- layers -------------------------------------------------------------
    AddLayer { layer: Layer },
    UpdateLayer { layer: Layer },
    RemoveLayer { id: LayerId },
    DuplicateLayer { id: LayerId },
    SplitLayer { id: LayerId, time: Secs },
    ReorderLayer { id: LayerId, delta: i32 },
    // --- markers and comments ----------------------------------------------
    AddComment { time: Secs, author: String, text: String },
    ReplyComment { id: CommentId, author: String, text: String },
    ResolveComment { id: CommentId, resolved: bool },
    DeleteComment { id: CommentId },
    // --- project ------------------------------------------------------------
    AddAsset { asset: MediaAsset },
    RemoveAsset { id: AssetId },
    RenameAsset { id: AssetId, name: String },
    SetAssetFolder { id: AssetId, folder: Option<String> },
    SetTranscript { asset: AssetId, transcript: Transcript },
    SetGlossary { glossary: Vec<GlossaryEntry> },
    RenameProject { name: String },
}

#[derive(Clone)]
struct Snapshot {
    project: Project,
    active: Option<CompositionId>,
    label: String,
}

pub struct Editor {
    pub project: Project,
    pub active: Option<CompositionId>,
    pub path: Option<PathBuf>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    dirty: bool,
    /// Bumped on every change; cheap way for views to know they are stale.
    pub revision: u64,
}

impl Editor {
    pub fn new(project: Project) -> Self {
        let active = project.compositions.first().map(|c| c.id);
        Self { project, active, path: None, undo: Vec::new(), redo: Vec::new(), dirty: false, revision: 0 }
    }

    pub fn new_project(name: &str) -> Self {
        let mut p = Project::new(name);
        p.compositions.push(Composition::new("Composition 1"));
        Self::new(p)
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn comp(&self) -> Option<&Composition> {
        self.active.and_then(|id| self.project.composition(id))
    }
    pub fn comp_mut(&mut self) -> Option<&mut Composition> {
        let id = self.active?;
        self.project.composition_mut(id)
    }
    fn comp_req(&mut self) -> Result<&mut Composition> {
        self.comp_mut().ok_or_else(|| anyhow!("no composition is open"))
    }

    // --- persistence ---------------------------------------------------------

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut project: Project = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if project.format_version > PROJECT_FORMAT_VERSION {
            bail!("this project was saved by a newer version (format {})", project.format_version);
        }
        // Media paths are stored relative to the project file when possible.
        if let Some(dir) = path.parent() {
            for a in &mut project.assets {
                if a.path.is_relative() {
                    a.path = dir.join(&a.path);
                }
            }
        }
        for c in &mut project.compositions {
            c.sync_scene_settings();
        }
        let mut e = Self::new(project);
        e.path = Some(path.to_path_buf());
        Ok(e)
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        let mut project = self.project.clone();
        project.format_version = PROJECT_FORMAT_VERSION;
        if let Some(dir) = path.parent() {
            for a in &mut project.assets {
                if let Ok(rel) = a.path.strip_prefix(dir) {
                    a.path = rel.to_path_buf();
                }
            }
        }
        let text = serde_json::to_string(&project)?;
        // Write to a sibling temp file, then rename, so a crash cannot leave a
        // half-written project.
        let tmp = path.with_extension("mango.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    // --- history -------------------------------------------------------------

    fn snapshot(&self, label: &str) -> Snapshot {
        Snapshot { project: self.project.clone(), active: self.active, label: label.to_string() }
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        let label = s.label.clone();
        self.redo.push(self.snapshot(&label));
        self.project = s.project;
        self.active = s.active;
        self.dirty = true;
        self.revision += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        let label = s.label.clone();
        self.undo.push(self.snapshot(&label));
        self.project = s.project;
        self.active = s.active;
        self.dirty = true;
        self.revision += 1;
        true
    }

    /// Replaces the whole project with an edited copy as one undo step (used
    /// when work was done on a copy, e.g. by the assistant). Returns false,
    /// changing nothing, when the copy is identical.
    pub fn commit_state(&mut self, label: &str, project: Project, active: Option<CompositionId>) -> bool {
        if project == self.project && active == self.active {
            return false;
        }
        let before = self.snapshot(label);
        self.project = project;
        self.active = active.filter(|id| self.project.composition(*id).is_some()).or_else(|| self.project.compositions.first().map(|c| c.id));
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        self.revision += 1;
        true
    }

    /// Number of undo steps available.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Applies a command as one undo step. On error nothing changes.
    pub fn apply(&mut self, cmd: Command) -> Result<()> {
        let label = command_label(&cmd);
        let before = self.snapshot(label);
        match self.exec(cmd) {
            Ok(()) => {
                if before.project != self.project || before.active != self.active {
                    self.undo.push(before);
                    if self.undo.len() > UNDO_LIMIT {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                    self.dirty = true;
                    self.revision += 1;
                }
                Ok(())
            }
            Err(e) => {
                self.project = before.project;
                self.active = before.active;
                Err(e)
            }
        }
    }

    /// Applies several commands as a single undo step. If any fails, all are
    /// rolled back.
    pub fn apply_all(&mut self, label: &str, cmds: Vec<Command>) -> Result<()> {
        let before = self.snapshot(label);
        for c in cmds {
            if let Err(e) = self.exec(c) {
                self.project = before.project;
                self.active = before.active;
                return Err(e);
            }
        }
        if before.project != self.project || before.active != self.active {
            self.undo.push(before);
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.dirty = true;
            self.revision += 1;
        }
        Ok(())
    }

    fn check_range(&self, start: usize, end: usize) -> Result<std::ops::Range<usize>> {
        let len = self.comp().map(|c| c.script.len()).unwrap_or(0);
        if start > end || end > len {
            bail!("range {start}..{end} is outside the script (0..{len})");
        }
        Ok(start..end)
    }

    fn remove(&mut self, ranges: Vec<std::ops::Range<usize>>, mode: RemoveMode) -> Result<()> {
        let comp = self.comp_req()?;
        // Back to front so earlier indices stay valid.
        let mut ranges = ranges;
        ranges.sort_by_key(|r| std::cmp::Reverse(r.start));
        for r in ranges {
            match mode {
                RemoveMode::Delete => {
                    comp.delete_range(r);
                }
                RemoveMode::Ignore => comp.set_ignored(r, true),
                RemoveMode::ReplaceWithGap => comp.replace_with_gap(r),
                RemoveMode::HideText => {
                    for t in &mut comp.script[r] {
                        t.hidden_text = true;
                    }
                }
            }
        }
        Ok(())
    }

    fn exec(&mut self, cmd: Command) -> Result<()> {
        use Command::*;
        match cmd {
            DeleteRange { start, end } => {
                let r = self.check_range(start, end)?;
                self.comp_req()?.delete_range(r);
            }
            IgnoreRange { start, end, ignored } => {
                let r = self.check_range(start, end)?;
                self.comp_req()?.set_ignored(r, ignored);
            }
            RemoveIgnored => {
                self.comp_req()?.remove_ignored();
            }
            ReplaceWithGap { start, end } => {
                let r = self.check_range(start, end)?;
                self.comp_req()?.replace_with_gap(r);
            }
            InsertGap { at, secs } => {
                if !(secs > 0.0 && secs <= 3600.0) {
                    bail!("gap length must be between 0 and 3600 seconds");
                }
                self.comp_req()?.insert_gap(at, secs);
            }
            MoveRange { start, end, dest } => {
                let r = self.check_range(start, end)?;
                self.comp_req()?.move_range(r, dest);
            }
            CorrectRange { start, end, text } => {
                let r = self.check_range(start, end)?;
                self.comp_req()?.correct_range(r, &text);
            }
            CorrectAll { from, to } => {
                if to.trim().is_empty() {
                    bail!("replacement text is empty");
                }
                for c in &mut self.project.compositions {
                    c.correct_all(&from, to.trim());
                }
                // Also fix the source transcripts so later inserts are right.
                let key = script::normalize_word(&from);
                for a in &mut self.project.assets {
                    if let Some(t) = &mut a.transcript {
                        for w in &mut t.words {
                            if script::normalize_word(&w.text) == key {
                                w.text = to.trim().to_string();
                            }
                        }
                    }
                }
            }
            HideText { start, end, hidden } => {
                let r = self.check_range(start, end)?;
                for t in &mut self.comp_req()?.script[r] {
                    if t.kind == TokenKind::Word {
                        t.hidden_text = hidden;
                    }
                }
            }
            SetSpeaker { start, end, speaker } => {
                let r = self.check_range(start, end)?;
                let id = speaker.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|n| self.project.ensure_speaker(n));
                self.comp_req()?.set_speaker(r, id);
            }
            RenameSpeaker { from, to } => {
                let to = to.trim();
                if to.is_empty() {
                    bail!("speaker name is empty");
                }
                let s = self.project.speakers.iter_mut().find(|s| s.name == from).ok_or_else(|| anyhow!("no speaker named {from}"))?;
                s.name = to.to_string();
            }
            SetParagraphBreak { at, on } => self.comp_req()?.set_paragraph_break(at, on),
            SetSceneBreak { at, on } => self.comp_req()?.set_scene_break(at, on),
            SetMarker { at, name } => {
                let r = self.check_range(at, at)?;
                self.comp_req()?.set_marker(r.start, name);
            }
            SetNote { at, note } => {
                let comp = self.comp_req()?;
                let t = comp.script.get_mut(at).ok_or_else(|| anyhow!("no token at {at}"))?;
                t.note = note.map(|n| n.trim().trim_matches(['(', ')']).to_string()).filter(|n| !n.is_empty());
            }
            SetHighlight { start, end, color } => {
                let r = self.check_range(start, end)?;
                self.comp_req()?.set_highlight(r, color);
            }
            SetStyle { start, end, bold, italic } => {
                let r = self.check_range(start, end)?;
                for t in &mut self.comp_req()?.script[r] {
                    if let Some(b) = bold {
                        t.bold = b;
                    }
                    if let Some(i) = italic {
                        t.italic = i;
                    }
                }
            }
            SetGapDuration { at, secs } => {
                if !(0.0..=3600.0).contains(&secs) {
                    bail!("gap length must be between 0 and 3600 seconds");
                }
                self.comp_req()?.set_gap_duration(at, secs);
            }
            ShortenGaps { longer_than, shorter_than, target } => {
                if target < 0.0 || longer_than < 0.0 {
                    bail!("durations must not be negative");
                }
                self.comp_req()?.shorten_gaps(longer_than, shorter_than, target);
            }
            RestoreAt { at } => {
                let id = self.active.ok_or_else(|| anyhow!("no composition is open"))?;
                let comp = self.project.composition(id).ok_or_else(|| anyhow!("no composition is open"))?;
                let toks = script::restore_at_boundary(&self.project, comp, at).ok_or_else(|| anyhow!("nothing to restore here"))?;
                self.comp_req()?.insert_tokens(at, toks);
            }
            RestoreRange { start, end } => {
                let r = self.check_range(start, end)?;
                let id = self.active.ok_or_else(|| anyhow!("no composition is open"))?;
                // Un-ignore, then restore each boundary inside the range, back to front.
                self.comp_req()?.set_ignored(r.clone(), false);
                let bounds: Vec<usize> = {
                    let comp = self.project.composition(id).unwrap();
                    comp.edit_boundaries().into_iter().filter(|b| *b > r.start && *b <= r.end.min(comp.script.len().saturating_sub(1))).collect()
                };
                for b in bounds.into_iter().rev() {
                    let comp = self.project.composition(id).unwrap();
                    if let Some(toks) = script::restore_at_boundary(&self.project, comp, b) {
                        self.comp_req()?.insert_tokens(b, toks);
                    }
                }
            }
            InsertAsset { asset, at } => {
                let a = self.project.asset(asset).ok_or_else(|| anyhow!("unknown file"))?.clone();
                if a.kind == MediaKind::Image {
                    bail!("images cannot go on the script track; add them as a layer");
                }
                let mut toks = script::tokens_from_asset(&a);
                let comp = self.comp_req()?;
                let empty = comp.script.is_empty();
                let at = at.unwrap_or(comp.script.len());
                if let Some(first) = toks.first_mut() {
                    first.paragraph_start = true;
                }
                comp.insert_tokens(at, toks);
                // An empty composition adopts the first video's frame size.
                if empty && a.has_video && a.width > 0 && a.height > 0 && comp.layers.is_empty() {
                    comp.width = a.width + a.width % 2;
                    comp.height = a.height + a.height % 2;
                    if a.fps > 1.0 && a.fps <= 120.0 {
                        comp.fps = a.fps;
                    }
                }
            }
            PasteTokens { at, tokens } => {
                for t in &tokens {
                    if t.kind != TokenKind::Blank && self.project.asset(t.asset).is_none() {
                        bail!("pasted text refers to a file that is not in this project");
                    }
                }
                self.comp_req()?.insert_tokens(at, tokens);
            }
            SplitAt { time } => {
                self.comp_req()?.split_token_at(time);
            }
            TrimBoundary { at, side, delta } => {
                let id = self.active.ok_or_else(|| anyhow!("no composition is open"))?;
                let comp = self.project.composition(id).ok_or_else(|| anyhow!("no composition is open"))?;
                let tok = match side {
                    TrimSide::Outgoing => (0..at.min(comp.script.len())).rev().map(|i| &comp.script[i]).find(|t| !t.ignored),
                    TrimSide::Incoming => comp.script.get(at..).and_then(|s| s.iter().find(|t| !t.ignored)),
                };
                let dur = tok.and_then(|t| self.project.asset(t.asset)).map(|a| a.duration).unwrap_or(f64::MAX);
                self.comp_req()?.trim_boundary(at, side, delta, dur);
            }
            RemoveFillers { options, mode } => {
                let hits = detect::find_fillers(self.comp_req()?, &options);
                self.remove(hits.into_iter().map(|h| h.tokens).collect(), mode)?;
            }
            RemoveRetakes { min_words, mode } => {
                let hits = detect::find_retakes(self.comp_req()?, min_words, 60);
                self.remove(hits.into_iter().map(|h| h.earlier).collect(), mode)?;
            }
            RemoveMatches { query, mode } => {
                let hits = script::find(self.comp_req()?, &query, &script::FindOptions { whole_words: true, ..Default::default() });
                if hits.is_empty() {
                    bail!("\"{query}\" was not found in the script");
                }
                self.remove(hits, mode)?;
            }
            NewComposition { name } => {
                let c = Composition::new(unique_name(&self.project, name.trim()));
                self.active = Some(c.id);
                self.project.compositions.push(c);
            }
            DuplicateComposition { name } => {
                let mut c = self.comp_req()?.clone();
                c.id = CompositionId::new();
                for l in &mut c.layers {
                    l.id = LayerId::new();
                }
                // Comments do not survive duplication.
                c.comments.clear();
                let base = name.unwrap_or_else(|| format!("{} (Copy)", c.name));
                c.name = unique_name(&self.project, base.trim());
                self.active = Some(c.id);
                self.project.compositions.push(c);
            }
            CompositionFromRange { start, end, name } => {
                let r = self.check_range(start, end)?;
                if r.is_empty() {
                    bail!("select some of the script first");
                }
                let src = self.comp_req()?.clone();
                let t0 = src.time_of_token(r.start);
                let t1 = src.time_of_token(r.end);
                let mut c = Composition::new(unique_name(&self.project, name.trim()));
                c.width = src.width;
                c.height = src.height;
                c.fps = src.fps;
                c.background = src.background;
                c.script_video = src.script_video.clone();
                c.script_audio = src.script_audio.clone();
                c.captions = src.captions.clone();
                c.script = src.script[r].to_vec();
                if let Some(f) = c.script.first_mut() {
                    f.paragraph_start = true;
                    f.scene_start = false;
                }
                // Carry layers that overlap the range, shifted and clipped.
                for l in &src.layers {
                    let (s, e) = (l.start.max(t0), l.end().min(t1));
                    if e - s > 0.01 {
                        let mut nl = l.clone();
                        nl.id = LayerId::new();
                        if let LayerContent::Video(m) | LayerContent::Audio(m) = &mut nl.content {
                            m.src_in += (s - l.start) * m.speed;
                        }
                        nl.start = s - t0;
                        nl.duration = e - s;
                        c.layers.push(nl);
                    }
                }
                c.sync_scene_settings();
                self.active = Some(c.id);
                self.project.compositions.push(c);
            }
            RenameComposition { name } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    bail!("name is empty");
                }
                self.comp_req()?.name = name;
            }
            DeleteComposition => {
                let id = self.active.ok_or_else(|| anyhow!("no composition is open"))?;
                self.project.compositions.retain(|c| c.id != id);
                self.active = self.project.compositions.first().map(|c| c.id);
            }
            SelectComposition { id } => {
                if self.project.composition(id).is_none() {
                    bail!("unknown composition");
                }
                self.active = Some(id);
            }
            SetCanvas { width, height, fill } => {
                if !(16..=8192).contains(&width) || !(16..=8192).contains(&height) {
                    bail!("canvas size must be between 16 and 8192 pixels");
                }
                let comp = self.comp_req()?;
                comp.width = width + width % 2;
                comp.height = height + height % 2;
                let fit = if fill { FitMode::Fill } else { FitMode::Fit };
                comp.script_video.fit = fit;
                for s in &mut comp.scenes {
                    if let Some(p) = &mut s.script_video {
                        p.fit = fit;
                    }
                }
            }
            SetFps { fps } => {
                if !(1.0..=120.0).contains(&fps) {
                    bail!("frame rate must be between 1 and 120");
                }
                self.comp_req()?.fps = fps;
            }
            SetBackground { color } => self.comp_req()?.background = color,
            SetScriptVideo { placement } => self.comp_req()?.script_video = placement,
            SetScriptAudio { audio } => self.comp_req()?.script_audio = audio,
            SetScene { scene, settings } => {
                let comp = self.comp_req()?;
                comp.sync_scene_settings();
                let slot = comp.scenes.get_mut(scene).ok_or_else(|| anyhow!("no scene {scene}"))?;
                let id = slot.id;
                *slot = settings;
                slot.id = id;
            }
            SetAllTransitions { transition, secs } => {
                let comp = self.comp_req()?;
                comp.sync_scene_settings();
                for s in comp.scenes.iter_mut().skip(1) {
                    s.transition = transition;
                    s.transition_secs = secs.clamp(0.05, 5.0);
                }
            }
            SetCaptions { captions } => self.comp_req()?.captions = captions,
            SetMasterLoudness { lufs } => {
                if let Some(l) = lufs {
                    if !(-40.0..=-5.0).contains(&l) {
                        bail!("loudness target must be between -40 and -5 LUFS");
                    }
                }
                self.comp_req()?.master_loudness = lufs;
            }
            AddLayer { layer } => {
                if layer.duration <= 0.0 {
                    bail!("layer has no duration");
                }
                match &layer.content {
                    LayerContent::Video(m) | LayerContent::Audio(m) => {
                        if self.project.asset(m.asset).is_none() {
                            bail!("layer refers to a file that is not in this project");
                        }
                    }
                    LayerContent::Image(i) => {
                        if self.project.asset(i.asset).is_none() {
                            bail!("layer refers to a file that is not in this project");
                        }
                    }
                    _ => {}
                }
                let comp = self.comp_req()?;
                if comp.layer(layer.id).is_some() {
                    bail!("a layer with this id already exists");
                }
                comp.layers.push(layer);
            }
            UpdateLayer { layer } => {
                let comp = self.comp_req()?;
                let slot = comp.layer_mut(layer.id).ok_or_else(|| anyhow!("unknown layer"))?;
                let mut layer = layer;
                layer.duration = layer.duration.max(0.01);
                layer.start = layer.start.max(0.0);
                *slot = layer;
            }
            RemoveLayer { id } => {
                let comp = self.comp_req()?;
                let n = comp.layers.len();
                comp.layers.retain(|l| l.id != id);
                if comp.layers.len() == n {
                    bail!("unknown layer");
                }
            }
            DuplicateLayer { id } => {
                let comp = self.comp_req()?;
                let mut l = comp.layer(id).ok_or_else(|| anyhow!("unknown layer"))?.clone();
                l.id = LayerId::new();
                l.name = format!("{} copy", l.name);
                l.transform.x = (l.transform.x + 0.03).min(1.0);
                l.transform.y = (l.transform.y + 0.03).min(1.0);
                comp.layers.push(l);
            }
            SplitLayer { id, time } => {
                let comp = self.comp_req()?;
                let l = comp.layer(id).ok_or_else(|| anyhow!("unknown layer"))?.clone();
                if time <= l.start + 0.01 || time >= l.end() - 0.01 {
                    bail!("the playhead is not inside this layer");
                }
                let mut right = l.clone();
                right.id = LayerId::new();
                right.start = time;
                right.duration = l.end() - time;
                right.fade_in = 0.0;
                right.animation_in = Animation::None;
                if let LayerContent::Video(m) | LayerContent::Audio(m) = &mut right.content {
                    m.src_in += (time - l.start) * m.speed;
                }
                let left = comp.layer_mut(id).unwrap();
                left.duration = time - l.start;
                left.fade_out = 0.0;
                left.animation_out = Animation::None;
                comp.layers.push(right);
            }
            ReorderLayer { id, delta } => {
                let comp = self.comp_req()?;
                let l = comp.layer_mut(id).ok_or_else(|| anyhow!("unknown layer"))?;
                l.track = (l.track + delta).clamp(1, 99);
            }
            AddComment { time, author, text } => {
                if text.trim().is_empty() {
                    bail!("comment is empty");
                }
                let comp = self.comp_req()?;
                let anchor = comp.source_at(time);
                comp.comments.push(Comment { id: CommentId::new(), time, anchor, author, text: text.trim().to_string(), resolved: false, replies: Vec::new() });
            }
            ReplyComment { id, author, text } => {
                let comp = self.comp_req()?;
                let c = comp.comments.iter_mut().find(|c| c.id == id).ok_or_else(|| anyhow!("unknown comment"))?;
                c.replies.push((author, text.trim().to_string()));
            }
            ResolveComment { id, resolved } => {
                let comp = self.comp_req()?;
                let c = comp.comments.iter_mut().find(|c| c.id == id).ok_or_else(|| anyhow!("unknown comment"))?;
                c.resolved = resolved;
            }
            DeleteComment { id } => {
                self.comp_req()?.comments.retain(|c| c.id != id);
            }
            AddAsset { asset } => {
                if self.project.asset(asset.id).is_some() {
                    bail!("this file is already in the project");
                }
                self.project.assets.push(asset);
            }
            RemoveAsset { id } => {
                let used = self.project.compositions.iter().any(|c| {
                    c.script.iter().any(|t| t.kind != TokenKind::Blank && t.asset == id)
                        || c.layers.iter().any(|l| match &l.content {
                            LayerContent::Video(m) | LayerContent::Audio(m) => m.asset == id,
                            LayerContent::Image(i) => i.asset == id,
                            _ => false,
                        })
                });
                if used {
                    bail!("this file is used in a composition; remove it there first");
                }
                self.project.assets.retain(|a| a.id != id);
            }
            RenameAsset { id, name } => {
                let a = self.project.asset_mut(id).ok_or_else(|| anyhow!("unknown file"))?;
                if name.trim().is_empty() {
                    bail!("name is empty");
                }
                a.name = name.trim().to_string();
            }
            SetAssetFolder { id, folder } => {
                let folder = folder.map(|f| f.trim().to_string()).filter(|f| !f.is_empty());
                if let Some(f) = &folder {
                    if !self.project.folders.contains(f) {
                        self.project.folders.push(f.clone());
                    }
                }
                self.project.asset_mut(id).ok_or_else(|| anyhow!("unknown file"))?.folder = folder;
            }
            SetTranscript { asset, transcript } => {
                let mut transcript = transcript;
                apply_glossary(&self.project.glossary, &mut transcript);
                let a = self.project.asset_mut(asset).ok_or_else(|| anyhow!("unknown file"))?;
                a.transcript = Some(transcript);
                // Re-text script tokens of this asset that have not been edited
                // structurally: rebuild any composition whose script still
                // covers the asset as untouched gap-only media.
                let a = a.clone();
                for c in &mut self.project.compositions {
                    retext_composition(c, &a);
                }
            }
            SetGlossary { glossary } => self.project.glossary = glossary,
            RenameProject { name } => {
                if name.trim().is_empty() {
                    bail!("name is empty");
                }
                self.project.name = name.trim().to_string();
            }
        }
        Ok(())
    }

    /// Composition time at which a comment currently sits.
    pub fn comment_time(&self, c: &Comment) -> Secs {
        let comp = match self.comp() {
            Some(c) => c,
            None => return c.time,
        };
        c.anchor.and_then(|(a, s)| comp.time_of_source(a, s)).unwrap_or(c.time)
    }
}

fn unique_name(project: &Project, base: &str) -> String {
    let base = if base.is_empty() { "Composition" } else { base };
    if !project.compositions.iter().any(|c| c.name == base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} {n}")).find(|n| !project.compositions.iter().any(|c| &c.name == n)).unwrap()
}

/// Rewrites misrecognitions listed in the glossary.
pub fn apply_glossary(glossary: &[GlossaryEntry], t: &mut Transcript) {
    for g in glossary {
        let keys: Vec<String> = g.sounds_like.iter().map(|s| script::normalize_word(s)).filter(|s| !s.is_empty()).collect();
        let term_key = script::normalize_word(&g.term);
        for w in &mut t.words {
            let k = script::normalize_word(&w.text);
            if keys.contains(&k) || (k == term_key && w.text.trim_matches(|c: char| !c.is_alphanumeric()) != g.term) {
                // Keep trailing punctuation.
                let tail: String = w.text.chars().rev().take_while(|c| !c.is_alphanumeric()).collect::<Vec<_>>().into_iter().rev().collect();
                w.text = format!("{}{}", g.term, tail);
            }
        }
    }
}

/// After (re)transcription: wherever a composition plays a stretch of this
/// asset that carries no words yet, replace it with tokens from the new
/// transcript covering the same source range.
fn retext_composition(c: &mut Composition, asset: &MediaAsset) {
    let all = script::tokens_from_asset(asset);
    let mut i = 0;
    while i < c.script.len() {
        let t = &c.script[i];
        if t.kind != TokenKind::Gap || t.asset != asset.id {
            i += 1;
            continue;
        }
        let (s, e) = (t.src_start, t.src_end);
        let template = t.clone();
        let mut repl: Vec<Token> = Vec::new();
        for a in &all {
            let (ss, ee) = (a.src_start.max(s), a.src_end.min(e));
            if ee - ss <= 1e-6 {
                continue;
            }
            let mut n = a.clone();
            if n.kind == TokenKind::Word && (ee - ss) < a.duration() * 0.5 {
                n.kind = TokenKind::Gap;
                n.text.clear();
            }
            n.src_start = ss;
            n.src_end = ee;
            n.ignored = template.ignored;
            repl.push(n);
        }
        if repl.iter().all(|r| r.kind != TokenKind::Word) {
            i += 1;
            continue;
        }
        if let Some(first) = repl.first_mut() {
            first.paragraph_start |= template.paragraph_start;
            first.scene_start = template.scene_start;
            first.marker = template.marker.clone();
        }
        let n = repl.len();
        c.script.splice(i..i + 1, repl);
        i += n;
    }
    if let Some(f) = c.script.first_mut() {
        f.paragraph_start = true;
    }
}

pub fn command_label(cmd: &Command) -> &'static str {
    use Command::*;
    match cmd {
        DeleteRange { .. } => "Delete",
        IgnoreRange { ignored: true, .. } => "Ignore",
        IgnoreRange { .. } => "Restore ignored",
        RemoveIgnored => "Remove ignored text",
        ReplaceWithGap { .. } => "Replace with gap",
        InsertGap { .. } => "Insert gap",
        MoveRange { .. } => "Move",
        CorrectRange { .. } => "Correct",
        CorrectAll { .. } => "Correct all",
        HideText { .. } => "Remove from transcript",
        SetSpeaker { .. } => "Change speaker",
        RenameSpeaker { .. } => "Rename speaker",
        SetParagraphBreak { .. } => "Paragraph break",
        SetSceneBreak { .. } => "Scene break",
        SetMarker { .. } => "Marker",
        SetNote { .. } => "Note",
        SetHighlight { .. } => "Highlight",
        SetStyle { .. } => "Text style",
        SetGapDuration { .. } => "Edit word gap",
        ShortenGaps { .. } => "Shorten word gaps",
        RestoreAt { .. } | RestoreRange { .. } => "Restore removed media",
        InsertAsset { .. } => "Add to script",
        PasteTokens { .. } => "Paste",
        SplitAt { .. } => "Split",
        TrimBoundary { .. } => "Trim",
        RemoveFillers { .. } => "Remove filler words",
        RemoveRetakes { .. } => "Remove retakes",
        RemoveMatches { .. } => "Remove matches",
        NewComposition { .. } => "New composition",
        DuplicateComposition { .. } => "Duplicate composition",
        CompositionFromRange { .. } => "Duplicate to new composition",
        RenameComposition { .. } => "Rename composition",
        DeleteComposition => "Delete composition",
        SelectComposition { .. } => "Switch composition",
        SetCanvas { .. } => "Change canvas size",
        SetFps { .. } => "Change frame rate",
        SetBackground { .. } => "Change background",
        SetScriptVideo { .. } => "Change video placement",
        SetScriptAudio { .. } => "Change audio",
        SetScene { .. } => "Change scene",
        SetAllTransitions { .. } => "Apply transition to all",
        SetCaptions { .. } => "Change captions",
        SetMasterLoudness { .. } => "Change loudness",
        AddLayer { .. } => "Add layer",
        UpdateLayer { .. } => "Change layer",
        RemoveLayer { .. } => "Delete layer",
        DuplicateLayer { .. } => "Duplicate layer",
        SplitLayer { .. } => "Split layer",
        ReorderLayer { .. } => "Reorder layer",
        AddComment { .. } => "Add comment",
        ReplyComment { .. } => "Reply",
        ResolveComment { .. } => "Resolve comment",
        DeleteComment { .. } => "Delete comment",
        AddAsset { .. } => "Add file",
        RemoveAsset { .. } => "Remove file",
        RenameAsset { .. } => "Rename file",
        SetAssetFolder { .. } => "Move file",
        SetTranscript { .. } => "Transcribe",
        SetGlossary { .. } => "Change glossary",
        RenameProject { .. } => "Rename project",
    }
}
