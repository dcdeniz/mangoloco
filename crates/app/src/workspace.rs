//! The main window: owns the app model and lays out the panes.

use crate::model::*;
use crate::theme;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mango_core::editor::RemoveMode;
use mango_core::{Command, Editor, ShapeKind};
use std::path::PathBuf;
use std::time::{Duration, Instant};

actions!(
    mango,
    [
        Undo,
        Redo,
        Save,
        SaveAs,
        OpenProject,
        NewProject,
        NewComposition,
        ImportMedia,
        ExportDialog,
        TogglePlay,
        Find,
        FindNext,
        FindPrev,
        JumpToTime,
        ToggleTimeline,
        ZoomIn,
        ZoomOut,
        ZoomFit,
        ToggleTheme,
        Quit,
        ShowShortcuts,
        SelectAll,
        Copy,
        Cut,
        Paste,
        DuplicateLayer,
        AddText,
        AddRectangle,
        AddComment,
        Highlight,
        NextScene,
        PrevScene,
        NextMarker,
        PrevMarker,
        DeleteForward,
        DeleteBackward,
        Ignore,
        Restore,
        ReplaceWithGap,
        CaretLeft,
        CaretRight,
        SelectLeft,
        SelectRight,
        ParagraphUp,
        ParagraphDown,
        SelectParagraphUp,
        SelectParagraphDown,
        Correct,
        SceneBreak,
        Marker,
        Speaker,
        ParagraphBreak,
        Split,
        Cancel,
        RateDown,
        RateReset,
        RateUp,
        FrameBack,
        FrameForward,
        Bold,
        Italic,
        DuplicateTo,
        ToggleLayerHidden,
        LayerForward,
        LayerBackward,
        ScriptBigger,
        ScriptSmaller,
    ]
);

pub fn bind_keys(cx: &mut App) {
    let ws = Some("Workspace");
    let ed = Some("ScriptEditor");
    let tl = Some("Timeline");
    let mut keys = vec![
        KeyBinding::new("cmd-z", Undo, ws),
        KeyBinding::new("cmd-shift-z", Redo, ws),
        KeyBinding::new("cmd-s", Save, ws),
        KeyBinding::new("cmd-shift-s", SaveAs, ws),
        KeyBinding::new("cmd-o", OpenProject, ws),
        KeyBinding::new("cmd-n", NewProject, ws),
        KeyBinding::new("cmd-alt-n", NewComposition, ws),
        KeyBinding::new("cmd-i", ImportMedia, ws),
        KeyBinding::new("cmd-shift-e", ExportDialog, ws),
        KeyBinding::new("cmd-f", Find, ws),
        KeyBinding::new("cmd-g", FindNext, ws),
        KeyBinding::new("cmd-shift-g", FindPrev, ws),
        KeyBinding::new("cmd-k", JumpToTime, ws),
        KeyBinding::new("cmd-shift-t", ToggleTimeline, ws),
        KeyBinding::new("cmd-=", ZoomIn, ws),
        KeyBinding::new("cmd--", ZoomOut, ws),
        KeyBinding::new("cmd-0", ZoomFit, ws),
        KeyBinding::new("alt-/", ShowShortcuts, ws),
        KeyBinding::new("cmd-d", DuplicateLayer, ws),
        KeyBinding::new("cmd-alt-t", AddText, ws),
        KeyBinding::new("cmd-alt-s", AddRectangle, ws),
        KeyBinding::new("cmd-shift-m", AddComment, ws),
        KeyBinding::new("cmd-shift-h", Highlight, ws),
        KeyBinding::new("cmd-shift-.", NextScene, ws),
        KeyBinding::new("cmd-shift-,", PrevScene, ws),
        KeyBinding::new("cmd-alt-.", NextMarker, ws),
        KeyBinding::new("cmd-alt-,", PrevMarker, ws),
        KeyBinding::new("cmd-shift-c", DuplicateTo, ws),
        KeyBinding::new("cmd-q", Quit, None),
    ];
    for ctx in [ed, tl] {
        keys.extend([
            KeyBinding::new("space", TogglePlay, ctx),
            KeyBinding::new("cmd-a", SelectAll, ctx),
            KeyBinding::new("cmd-c", Copy, ctx),
            KeyBinding::new("cmd-x", Cut, ctx),
            KeyBinding::new("cmd-v", Paste, ctx),
            KeyBinding::new("s", Split, ctx),
            KeyBinding::new("escape", Cancel, ctx),
            KeyBinding::new("shift-j", RateDown, ctx),
            KeyBinding::new("shift-k", RateReset, ctx),
            KeyBinding::new("shift-l", RateUp, ctx),
            KeyBinding::new("0", ToggleLayerHidden, ctx),
            KeyBinding::new("]", LayerForward, ctx),
            KeyBinding::new("[", LayerBackward, ctx),
        ]);
    }
    keys.extend([
        KeyBinding::new("delete", DeleteForward, ed),
        KeyBinding::new("backspace", DeleteBackward, ed),
        KeyBinding::new("cmd-backspace", Ignore, ed),
        KeyBinding::new("cmd-shift-backspace", Restore, ed),
        KeyBinding::new("shift-backspace", ReplaceWithGap, ed),
        KeyBinding::new("left", CaretLeft, ed),
        KeyBinding::new("right", CaretRight, ed),
        KeyBinding::new("shift-left", SelectLeft, ed),
        KeyBinding::new("shift-right", SelectRight, ed),
        KeyBinding::new("up", ParagraphUp, ed),
        KeyBinding::new("down", ParagraphDown, ed),
        KeyBinding::new("shift-up", SelectParagraphUp, ed),
        KeyBinding::new("shift-down", SelectParagraphDown, ed),
        KeyBinding::new("c", Correct, ed),
        KeyBinding::new("/", SceneBreak, ed),
        KeyBinding::new("#", Marker, ed),
        KeyBinding::new("@", Speaker, ed),
        KeyBinding::new("enter", ParagraphBreak, ed),
        KeyBinding::new("cmd-b", Bold, ed),
        KeyBinding::new("cmd-i", Italic, ed),
        KeyBinding::new("cmd-shift-=", ScriptBigger, ed),
        KeyBinding::new("cmd-shift--", ScriptSmaller, ed),
        KeyBinding::new("left", FrameBack, tl),
        KeyBinding::new("right", FrameForward, tl),
        KeyBinding::new("delete", DeleteForward, tl),
        KeyBinding::new("backspace", DeleteForward, tl),
    ]);
    cx.bind_keys(keys);
}

/// What the one-line prompt bar is currently asking for.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    Correct,
    Marker,
    Speaker,
    Comment,
    JumpToTime,
    NewComposition,
    RenameComposition,
    DuplicateTo,
    AddText,
    EditLayerText,
    ApiKey,
    Translate,
    Speak,
    SocialPost,
    CorrectAll,
    RenameSpeaker,
    Glossary,
    Brainstorm,
    Outline,
    WriteScript,
}

impl Ask {
    fn label(&self) -> &'static str {
        match self {
            Ask::Correct => "Correct",
            Ask::Marker => "Marker",
            Ask::Speaker => "Speaker",
            Ask::Comment => "Comment",
            Ask::JumpToTime => "Jump to",
            Ask::NewComposition => "New composition",
            Ask::RenameComposition => "Rename",
            Ask::DuplicateTo => "Duplicate to",
            Ask::AddText => "Text",
            Ask::EditLayerText => "Text",
            Ask::ApiKey => "Anthropic API key",
            Ask::Translate => "Translate subtitles to",
            Ask::Speak => "Speak",
            Ask::SocialPost => "Post for",
            Ask::CorrectAll => "Correct everywhere",
            Ask::RenameSpeaker => "Rename speaker",
            Ask::Glossary => "Glossary",
            Ask::Brainstorm => "Brainstorm",
            Ask::Outline => "Outline",
            Ask::WriteScript => "Write a script",
        }
    }
    fn placeholder(&self) -> &'static str {
        match self {
            Ask::Correct => "Type the correct text. The media stays the same",
            Ask::Marker => "Marker name. Leave empty to remove it",
            Ask::Speaker => "Speaker name. Leave empty to clear it",
            Ask::Comment => "Write a comment",
            Ask::JumpToTime => "1:30, 90, or 1:02:03",
            Ask::NewComposition | Ask::RenameComposition | Ask::DuplicateTo => "Composition name",
            Ask::AddText | Ask::EditLayerText => "Text to show on screen",
            Ask::ApiKey => "sk-ant-… (stored on this Mac only)",
            Ask::Translate => "A language, for example Spanish",
            Ask::Speak => "Text to turn into speech with a system voice",
            Ask::SocialPost => "X, LinkedIn, Instagram, TikTok or Threads",
            Ask::CorrectAll => "wrong => right, for example mangoloco => Mango Loco",
            Ask::RenameSpeaker => "Old name => New name",
            Ask::Glossary => "Names and terms to spell this way, separated by commas",
            Ask::Brainstorm | Ask::Outline | Ask::WriteScript => "What is it about?",
        }
    }
}

pub struct Workspace {
    pub model: AppModel,
    pub(crate) focus: FocusHandle,
    pub(crate) script_focus: FocusHandle,
    pub(crate) timeline_focus: FocusHandle,
    pub(crate) script_list: ListState,
    pub(crate) script_paragraphs: Vec<std::ops::Range<usize>>,
    pub(crate) script_revision: u64,
    pub(crate) script_comp: Option<mango_core::CompositionId>,
    pub(crate) dragging_script: bool,
    pub(crate) prompt: Option<Ask>,
    pub(crate) prompt_input: Entity<InputState>,
    pub(crate) find_input: Entity<InputState>,
    pub(crate) timeline: crate::timeline::TimelineState,
    pub(crate) canvas: crate::canvas::CanvasState,
    pub(crate) show_shortcuts: bool,
    pub jobs: crate::jobs::Jobs,
    pub preview: crate::preview::Preview,
    pub(crate) overlay: crate::services::OverlayState,
    pub export_panel: Option<crate::services::ExportChoice>,
    pub ai: crate::services_ai::AiState,
    pub capture: crate::services_capture::CaptureState,
    pub last_export: Option<PathBuf>,
    pub settings: crate::settings::Settings,
    last_tick: Instant,
    _subs: Vec<Subscription>,
    _tick: Task<()>,
}

impl Workspace {
    pub fn new(editor: Editor, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt_input = cx.new(|cx| InputState::new(window, cx));
        let find_input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in script"));
        let subs = vec![
            cx.subscribe_in(&prompt_input, window, |this, _input, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.submit_prompt(window, cx);
                }
            }),
            cx.subscribe_in(&find_input, window, |this, input, ev: &InputEvent, _window, cx| match ev {
                InputEvent::Change => {
                    let q = input.read(cx).value().to_string();
                    this.model.set_find_query(&q);
                    this.reveal_caret();
                    cx.notify();
                }
                InputEvent::PressEnter { shift, .. } => {
                    this.model.find_next(if *shift { -1 } else { 1 });
                    this.reveal_caret();
                    cx.notify();
                }
                _ => {}
            }),
        ];
        let tick = cx.spawn_in(window, async move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let alive = this.update_in(cx, |this, window, cx| {
                    if this.ai.drain_events() || this.capture.recorder.is_some() {
                        cx.notify();
                    }
                    if !this.jobs.is_empty() {
                        for done in this.jobs.take_finished() {
                            done(this, window, cx);
                        }
                        cx.notify();
                    }
                    let now = Instant::now();
                    let dt = now.duration_since(this.last_tick).as_secs_f64();
                    this.last_tick = now;
                    this.prepare_audio(cx);
                    let was_playing = this.model.playing;
                    let mut changed = this.preview.sync(&mut this.model, dt.min(0.25));
                    changed |= this.preview.frame_ready.swap(false, std::sync::atomic::Ordering::Relaxed);
                    if let Some(note) = this.preview.notice.take() {
                        this.model.fail(note);
                        changed = true;
                    }
                    if was_playing && this.model.follow_playhead {
                        this.follow_playhead();
                    }
                    if changed {
                        this.refresh_pictures(cx);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let script_focus = cx.focus_handle();
        script_focus.focus(window, cx);
        Self {
            model: AppModel::new(editor),
            focus: cx.focus_handle(),
            script_focus,
            timeline_focus: cx.focus_handle(),
            script_list: ListState::new(0, ListAlignment::Top, px(600.)),
            script_paragraphs: Vec::new(),
            script_revision: u64::MAX,
            script_comp: None,
            dragging_script: false,
            prompt: None,
            prompt_input,
            find_input,
            timeline: Default::default(),
            canvas: Default::default(),
            show_shortcuts: false,
            jobs: Default::default(),
            preview: crate::preview::Preview::new(),
            overlay: Default::default(),
            export_panel: None,
            ai: crate::services_ai::AiState::new(window, cx),
            capture: Default::default(),
            last_export: None,
            settings: crate::settings::Settings::load(),
            last_tick: Instant::now(),
            _subs: subs,
            _tick: tick,
        }
    }

    // --- preview --------------------------------------------------------------

    /// Starts decoding audio for any project file that has not been prepared.
    fn prepare_audio(&mut self, cx: &mut Context<Self>) {
        for path in self.preview.missing_audio(&self.model) {
            let cache = self.preview.cache.clone();
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            self.jobs.start(format!("Preparing audio for {name}"), move |job| {
                let result = cache.ensure(&path, &mut |p| job.set_progress(p), &job.cancel).map(|_| ());
                Box::new(move |ws, _window, cx| {
                    match result {
                        Ok(()) => ws.preview.invalidate(),
                        Err(e) if e.downcast_ref::<mango_playback::pcm::Cancelled>().is_some() => {
                            // Allow another attempt later.
                            ws.preview.requested.remove(&path);
                        }
                        Err(e) => ws.model.fail(format!("Could not read the sound of {name}: {e:#}")),
                    }
                    cx.notify();
                })
            });
            cx.notify();
        }
    }

    /// Pulls new decoded pictures into the canvas.
    pub(crate) fn refresh_pictures(&mut self, cx: &mut Context<Self>) {
        if let Some(pic) = self.preview.script_picture(&self.model) {
            let new = pic.and_then(|f| crate::preview::to_image(&f));
            if let Some(old) = std::mem::replace(&mut self.canvas.picture, new) {
                cx.drop_image(old, None);
            }
        }
        self.refresh_overlay(cx);
        for (id, pic) in self.preview.layer_pictures(&self.model) {
            if let Some(i) = self.canvas.layer_pictures.iter().position(|(l, _)| *l == id) {
                let (_, old) = self.canvas.layer_pictures.remove(i);
                cx.drop_image(old, None);
            }
            if let Some(img) = pic.and_then(|f| crate::preview::to_image(&f)) {
                self.canvas.layer_pictures.push((id, img));
            }
        }
    }

    // --- prompt bar -----------------------------------------------------------

    pub fn open_prompt(&mut self, kind: Ask, seed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = kind.placeholder();
        self.prompt = Some(kind);
        let seed = seed.to_string();
        self.prompt_input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
            input.set_value(seed, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    pub fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        self.script_focus.focus(window, cx);
        cx.notify();
    }

    pub fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.prompt.clone() else { return };
        let text = self.prompt_input.read(cx).value().to_string();
        let trimmed = text.trim();
        let named = |s: &str| if s.is_empty() { None } else { Some(s.to_string()) };
        let ok = match kind {
            Ask::Correct => self.model.correct(trimmed),
            Ask::Marker => self.model.set_marker(named(trimmed)),
            Ask::Speaker => self.model.set_speaker(named(trimmed)),
            Ask::Comment => self.model.add_comment(trimmed),
            Ask::JumpToTime => match self.model.jump_to_time(trimmed) {
                Ok(()) => true,
                Err(e) => {
                    self.model.fail(e);
                    false
                }
            },
            Ask::NewComposition => {
                let ok = self.model.run(Command::NewComposition { name: trimmed.to_string() });
                if ok {
                    self.model.reset_view();
                }
                ok
            }
            Ask::RenameComposition => self.model.run(Command::RenameComposition { name: trimmed.to_string() }),
            Ask::DuplicateTo => self.model.duplicate_to_new_composition(trimmed),
            Ask::AddText => !trimmed.is_empty() && self.model.add_text(trimmed).is_some(),
            Ask::CorrectAll | Ask::RenameSpeaker => match trimmed.split_once("=>") {
                Some((from, to)) if !from.trim().is_empty() && !to.trim().is_empty() => {
                    let (from, to) = (from.trim().to_string(), to.trim().to_string());
                    if kind == Ask::CorrectAll {
                        let ok = self.model.run(Command::CorrectAll { from: from.clone(), to: to.clone() });
                        if ok {
                            self.model.say(format!("Corrected every “{from}” to “{to}”"));
                        }
                        ok
                    } else {
                        self.model.run(Command::RenameSpeaker { from, to })
                    }
                }
                _ => {
                    self.model.fail("Write it as: old => new");
                    false
                }
            },
            Ask::Glossary => {
                let glossary = trimmed.split(',').map(str::trim).filter(|t| !t.is_empty()).map(|t| mango_core::GlossaryEntry { term: t.to_string(), sounds_like: Vec::new() }).collect::<Vec<_>>();
                let n = glossary.len();
                let ok = self.model.run(Command::SetGlossary { glossary });
                if ok {
                    self.model.say(format!("Glossary has {n} term{}. It applies to the next transcription", if n == 1 { "" } else { "s" }));
                }
                ok || n == self.model.editor.project.glossary.len()
            }
            Ask::Brainstorm | Ask::Outline | Ask::WriteScript => self.write_with_ai(kind.clone(), trimmed, cx),
            Ask::ApiKey => self.set_api_key(trimmed),
            Ask::Translate => self.translate_subtitles(trimmed, cx),
            Ask::Speak => self.speak_text(trimmed, cx),
            Ask::SocialPost => self.social_post(trimmed, cx),
            Ask::EditLayerText => {
                let t = text.clone();
                self.model.edit_layer(|l| {
                    if let mango_core::LayerContent::Text(s) = &mut l.content {
                        s.text = t;
                    }
                })
            }
        };
        if ok {
            self.close_prompt(window, cx);
        } else {
            // Keep the prompt open so the text can be fixed.
            cx.notify();
        }
    }

    // --- scrolling ------------------------------------------------------------

    /// Scrolls the script so the caret's paragraph is visible.
    pub fn reveal_caret(&mut self) {
        let head = self.model.head;
        self.reveal_token(head);
    }

    pub fn reveal_token(&mut self, token: usize) {
        self.sync_paragraphs();
        if let Some(ix) = self.script_paragraphs.iter().position(|p| p.contains(&token)) {
            // Paragraphs that have never been laid out have no height yet, so
            // "reveal" cannot work out where they are; jump straight to them.
            match self.script_list.bounds_for_item(ix) {
                Some(_) => self.script_list.scroll_to_reveal_item(ix),
                None => self.script_list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.) }),
            }
        }
    }

    fn follow_playhead(&mut self) {
        if let Some(tok) = self.model.playing_token() {
            self.reveal_token(tok);
        }
    }

    /// Recomputes the paragraph list when the script changed.
    pub fn sync_paragraphs(&mut self) {
        let rev = self.model.editor.revision;
        let comp = self.model.editor.active;
        if rev == self.script_revision && comp == self.script_comp {
            return;
        }
        let paragraphs = self.model.comp().map(|c| c.paragraphs()).unwrap_or_default();
        if paragraphs.len() != self.script_paragraphs.len() || comp != self.script_comp {
            let top = self.script_list.logical_scroll_top();
            self.script_list.reset(paragraphs.len());
            if comp == self.script_comp {
                self.script_list.scroll_to(top);
            }
        } else {
            self.script_list.remeasure();
        }
        self.script_paragraphs = paragraphs;
        self.script_revision = rev;
        self.script_comp = comp;
    }

    // --- files ----------------------------------------------------------------

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        match self.model.editor.path.clone() {
            Some(path) => self.save_to(path, cx),
            None => self.save_as(&SaveAs, window, cx),
        }
    }

    fn save_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match self.model.editor.save(&path) {
            Ok(()) => self.model.say(format!("Saved {}", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())),
            Err(e) => self.model.fail(format!("Could not save: {e:#}")),
        }
        cx.notify();
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self
            .model
            .editor
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .or_else(|| std::env::home_dir().map(|h| h.join("Documents")))
            .unwrap_or_default();
        let name = format!("{}.mango", self.model.editor.project.name);
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut path))) = rx.await {
                if path.extension().is_none() {
                    path.set_extension("mango");
                }
                this.update_in(cx, |this, _window, cx| this.save_to(path, cx)).ok();
            }
        })
        .detach();
    }

    fn open_project(&mut self, _: &OpenProject, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Open".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.into_iter().next() {
                    this.update_in(cx, |this, _window, cx| this.load_project(path, cx)).ok();
                }
            }
        })
        .detach();
    }

    pub fn load_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match Editor::load(&path) {
            Ok(editor) => {
                self.model = AppModel::new(editor);
                self.script_revision = u64::MAX;
                self.script_comp = None;
            }
            Err(e) => self.model.fail(format!("Could not open {}: {e:#}", path.display())),
        }
        cx.notify();
    }

    fn new_project(&mut self, _: &NewProject, _window: &mut Window, cx: &mut Context<Self>) {
        if self.model.editor.is_dirty() {
            self.model.fail("Save this project first (Cmd+S), then create a new one");
        } else {
            self.model = AppModel::new(Editor::new_project("Untitled"));
            self.script_revision = u64::MAX;
            self.script_comp = None;
        }
        cx.notify();
    }

    // --- actions --------------------------------------------------------------

    /// Runs a model action, then repaints and keeps the caret in view.
    pub(crate) fn act(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut AppModel)) {
        f(&mut self.model);
        self.reveal_caret();
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.model.copy() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.notify();
    }

    fn cut(&mut self, _: &Cut, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.model.cut() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.notify();
    }

    fn delete_forward(&mut self, _: &DeleteForward, _window: &mut Window, cx: &mut Context<Self>) {
        if self.model.selected_layer.is_some() {
            self.act(cx, |m| {
                m.delete_layer();
            });
        } else {
            self.act(cx, |m| {
                m.delete(true);
            });
        }
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_shortcuts {
            self.show_shortcuts = false;
        } else if self.export_panel.is_some() {
            self.export_panel = None;
        } else if self.capture.open && self.capture.recorder.is_none() {
            self.capture.open = false;
        } else if self.model.find.is_some() {
            self.model.close_find();
            self.script_focus.focus(window, cx);
        } else if self.model.playing {
            self.model.playing = false;
        } else {
            self.model.clear_selection();
            self.model.selected_layer = None;
        }
        cx.notify();
    }

    fn find(&mut self, _: &Find, window: &mut Window, cx: &mut Context<Self>) {
        self.model.open_find();
        self.find_input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        let dark = theme::is_dark(cx);
        theme::apply(cx, !dark);
        cx.notify();
    }

    // --- render ---------------------------------------------------------------

    fn render_top_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let active = self.model.editor.active;
        let comps: Vec<(mango_core::CompositionId, String)> = self.model.editor.project.compositions.iter().map(|c| (c.id, c.name.clone())).collect();
        let dirty = self.model.editor.is_dirty();
        h_flex()
            .h(px(46.))
            .flex_none()
            .w_full()
            .px_3()
            .gap_3()
            .items_center()
            .bg(t.title_bar)
            .border_b_1()
            .border_color(t.border)
            // Leave room for the traffic lights.
            .pl(px(84.))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().size(px(10.)).rounded_full().bg(theme::mango()))
                    .child(div().font_weight(FontWeight::BOLD).child("Mango Loco"))
                    .child(div().text_color(t.muted_foreground).child(format!("{}{}", self.model.editor.project.name, if dirty { " •" } else { "" }))),
            )
            .child(
                h_flex().id("comp-tabs").gap_1().flex_1().overflow_x_scroll().children(comps.into_iter().enumerate().map(|(i, (id, name))| {
                    let on = Some(id) == active;
                    div()
                        .id(("comp-tab", i)).test_support()
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .text_sm()
                        .flex_none()
                        .when(on, |d| d.bg(t.muted).font_weight(FontWeight::SEMIBOLD))
                        .when(!on, |d| d.text_color(t.muted_foreground).hover(|s| s.bg(t.muted.opacity(0.5))))
                        .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                            if ev.click_count() >= 2 {
                                let seed = this.model.comp().map(|c| c.name.clone()).unwrap_or_default();
                                this.open_prompt(Ask::RenameComposition, &seed, window, cx);
                            } else {
                                this.model.select_composition(id);
                                cx.notify();
                            }
                        }))
                        .child(name)
                })),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_none()
                    .child(Button::new("new-comp").ghost().small().label("+ Composition").on_click(cx.listener(|this, _, window, cx| {
                        let n = this.model.editor.project.compositions.len() + 1;
                        this.open_prompt(Ask::NewComposition, &format!("Composition {n}"), window, cx);
                    })))
                    .child(Button::new("dup-comp").ghost().small().label("Duplicate").tooltip("Duplicate this composition").on_click(cx.listener(|this, _, _, cx| {
                        this.act(cx, |m| {
                            if m.run(Command::DuplicateComposition { name: None }) {
                                m.reset_view();
                            }
                        })
                    })))
                    .child(Button::new("del-comp").ghost().small().label("Delete").tooltip("Delete this composition").disabled(self.model.editor.project.compositions.len() < 2).on_click(cx.listener(|this, _, _, cx| {
                        this.act(cx, |m| {
                            if m.run(Command::DeleteComposition) {
                                m.reset_view();
                                m.say("Composition deleted. Undo brings it back");
                            }
                        })
                    })))
                    .child(
                        Button::new("undo")
                            .ghost()
                            .small()
                            .label("Undo")
                            .disabled(!self.model.editor.can_undo())
                            .on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                                m.undo();
                            }))),
                    )
                    .child(
                        Button::new("redo")
                            .ghost()
                            .small()
                            .label("Redo")
                            .disabled(!self.model.editor.can_redo())
                            .on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| {
                                m.redo();
                            }))),
                    )
                    .child(Button::new("export").primary().small().label("Export").on_click(|_, window, cx| window.dispatch_action(Box::new(ExportDialog), cx))),
            )
    }

    fn render_prompt_bar(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let kind = self.prompt.clone()?;
        let t = cx.theme().clone();
        Some(
            h_flex()
                .id("prompt-bar").test_support()
                .w_full()
                .flex_none()
                .px_3()
                .py_2()
                .gap_2()
                .items_center()
                .bg(t.muted)
                .border_b_1()
                .border_color(t.border)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.close_prompt(window, cx);
                    }
                }))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::mango()).child(kind.label()))
                .child(div().flex_1().child(Input::new(&self.prompt_input).id("prompt-input")))
                .child(Button::new("prompt-ok").primary().small().label("Apply").on_click(cx.listener(|this, _, window, cx| this.submit_prompt(window, cx))))
                .child(Button::new("prompt-cancel").ghost().small().label("Cancel").on_click(cx.listener(|this, _, window, cx| this.close_prompt(window, cx)))),
        )
    }

    fn render_find_bar(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let f = self.model.find.as_ref()?;
        let t = cx.theme().clone();
        let count = if f.query.trim().is_empty() {
            String::new()
        } else if f.hits.is_empty() {
            "No matches".to_string()
        } else {
            format!("{} of {}", f.current + 1, f.hits.len())
        };
        let has_hits = !f.hits.is_empty();
        Some(
            h_flex()
                .id("find-bar").test_support()
                .w_full()
                .flex_none()
                .px_3()
                .py_2()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(t.border)
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.model.close_find();
                        this.script_focus.focus(window, cx);
                        cx.notify();
                    }
                }))
                .child(div().w(px(260.)).child(Input::new(&self.find_input).id("find-input")))
                .child(div().id("find-count").test_support().text_sm().text_color(t.muted_foreground).w(px(90.)).child(count))
                .child(Button::new("find-prev").ghost().small().label("Previous").disabled(!has_hits).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| m.find_next(-1)))))
                .child(Button::new("find-next").ghost().small().label("Next").disabled(!has_hits).on_click(cx.listener(|this, _, _, cx| this.act(cx, |m| m.find_next(1)))))
                .child(Button::new("find-delete").ghost().small().label("Delete all").disabled(!has_hits).on_click(cx.listener(|this, _, _, cx| {
                    this.act(cx, |m| {
                        m.remove_find_hits(RemoveMode::Delete);
                    })
                })))
                .child(Button::new("find-ignore").ghost().small().label("Ignore all").disabled(!has_hits).on_click(cx.listener(|this, _, _, cx| {
                    this.act(cx, |m| {
                        m.remove_find_hits(RemoveMode::Ignore);
                    })
                })))
                .child(div().flex_1())
                .child(Button::new("find-close").ghost().small().label("Done").on_click(cx.listener(|this, _, window, cx| {
                    this.model.close_find();
                    this.script_focus.focus(window, cx);
                    cx.notify();
                }))),
        )
    }

    fn render_status_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let toast = self.model.toast.clone();
        h_flex()
            .id("status-bar")
            .h(px(26.))
            .flex_none()
            .w_full()
            .px_3()
            .gap_4()
            .items_center()
            .text_xs()
            .bg(t.status_bar)
            .border_t_1()
            .border_color(t.border)
            .child(div().id("stats").test_support().text_color(t.muted_foreground).child(self.model.stats()))
            .child(div().flex_1())
            .when_some(toast, |d, toast| d.child(div().id("toast").test_support().text_color(if toast.error { theme::hibiscus() } else { t.foreground }).child(toast.text)))
            .child(div().text_color(t.muted_foreground).child(format!("{}x", self.model.rate)))
    }

    fn render_shortcuts(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let rows: &[(&str, &str)] = &[
            ("Play / pause", "Space"),
            ("Delete selection", "Delete"),
            ("Ignore (strike through)", "Cmd+Delete"),
            ("Restore removed media", "Shift+Cmd+Delete"),
            ("Replace with gap", "Shift+Delete"),
            ("Correct text", "C"),
            ("Scene boundary", "/"),
            ("Marker", "#"),
            ("Speaker", "@"),
            ("Paragraph break", "Enter"),
            ("Split at playhead", "S"),
            ("Move by word / paragraph", "Left Right / Up Down"),
            ("Extend selection", "Shift+arrows"),
            ("Find", "Cmd+F"),
            ("Jump to time", "Cmd+K"),
            ("Playback speed", "Shift+J / K / L"),
            ("Next / previous scene", "Shift+Cmd+. / ,"),
            ("Next / previous marker", "Opt+Cmd+. / ,"),
            ("Add text / rectangle", "Opt+Cmd+T / S"),
            ("Comment", "Shift+Cmd+M"),
            ("Highlight", "Shift+Cmd+H"),
            ("Duplicate selection to new composition", "Shift+Cmd+C"),
            ("Duplicate layer", "Cmd+D"),
            ("Hide layer / layer order", "0 / [ ]"),
            ("Timeline zoom", "Cmd+= / Cmd+- / Cmd+0"),
            ("Toggle timeline", "Shift+Cmd+T"),
            ("Import / export", "Cmd+I / Shift+Cmd+E"),
            ("Undo / redo", "Cmd+Z / Shift+Cmd+Z"),
        ];
        div()
            .id("shortcuts").test_support()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::black().opacity(0.5))
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_shortcuts = false;
                cx.notify();
            }))
            .child(
                v_flex()
                    .w(px(520.))
                    .max_h(px(620.))
                    .p_5()
                    .gap_1()
                    .rounded_lg()
                    .bg(t.popover)
                    .border_1()
                    .border_color(t.border)
                    .child(div().text_lg().font_weight(FontWeight::BOLD).pb_2().child("Keyboard shortcuts"))
                    .children(rows.iter().map(|(what, keys)| {
                        h_flex().justify_between().text_sm().py(px(2.)).child(div().child(*what)).child(div().text_color(t.muted_foreground).child(*keys))
                    })),
            )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_paragraphs();
        let t = cx.theme().clone();
        let (bg, fg) = (t.background, t.foreground);
        let show_timeline = self.model.show_timeline;
        let top = self.render_top_bar(cx).into_any_element();
        let prompt = self.render_prompt_bar(cx).map(|e| e.into_any_element());
        let find = self.render_find_bar(cx).map(|e| e.into_any_element());
        let sidebar = self.render_sidebar(window, cx).into_any_element();
        let script = self.render_script(window, cx).into_any_element();
        let right = self.render_right(window, cx).into_any_element();
        let timeline = show_timeline.then(|| self.render_timeline(window, cx).into_any_element());
        let status = self.render_status_bar(cx).into_any_element();
        let shortcuts = self.show_shortcuts.then(|| self.render_shortcuts(cx).into_any_element());
        let export = self.export_panel.is_some().then(|| self.render_export_panel(cx).into_any_element());
        let record = self.capture.open.then(|| self.render_record_panel(cx).into_any_element());

        v_flex()
            .id("workspace")
            .size_full()
            .relative()
            .bg(bg)
            .text_color(fg)
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.act(cx, |m| {
                m.undo();
            })))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.act(cx, |m| {
                m.redo();
            })))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::new_project))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::import_dialog))
            .on_action(cx.listener(Self::open_export))
            .on_action(cx.listener(Self::open_record))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| this.import_paths(paths.paths().to_vec(), window, cx)))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::delete_forward))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.act(cx, |m| {
                m.paste();
            })))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.act(cx, |m| m.select_all())))
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| {
                this.last_tick = Instant::now();
                this.model.toggle_play();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.act(cx, |m| m.find_next(1))))
            .on_action(cx.listener(|this, _: &FindPrev, _, cx| this.act(cx, |m| m.find_next(-1))))
            .on_action(cx.listener(|this, _: &JumpToTime, window, cx| this.open_prompt(Ask::JumpToTime, "", window, cx)))
            .on_action(cx.listener(|this, _: &NewComposition, window, cx| {
                let n = this.model.editor.project.compositions.len() + 1;
                this.open_prompt(Ask::NewComposition, &format!("Composition {n}"), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleTimeline, _, cx| {
                this.model.show_timeline = !this.model.show_timeline;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
                this.model.zoom(1.4);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
                this.model.zoom(1.0 / 1.4);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomFit, _, cx| {
                let w = this.timeline.width();
                this.model.zoom_to_fit(w);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ShowShortcuts, _, cx| {
                this.show_shortcuts = !this.show_shortcuts;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DuplicateLayer, _, cx| this.act(cx, |m| {
                m.duplicate_layer();
            })))
            .on_action(cx.listener(|this, _: &AddText, window, cx| this.open_prompt(Ask::AddText, "", window, cx)))
            .on_action(cx.listener(|this, _: &AddRectangle, _, cx| this.act(cx, |m| {
                m.add_shape(ShapeKind::Rectangle);
            })))
            .on_action(cx.listener(|this, _: &AddComment, window, cx| this.open_prompt(Ask::Comment, "", window, cx)))
            .on_action(cx.listener(|this, _: &Highlight, _, cx| {
                this.act(cx, |m| {
                    let r = m.target();
                    let on = m.comp().is_some_and(|c| c.script[r].iter().all(|t| t.highlight.is_some()));
                    m.highlight(if on { None } else { Some(0) });
                })
            }))
            .on_action(cx.listener(|this, _: &NextScene, _, cx| this.act(cx, |m| m.jump_scene(1))))
            .on_action(cx.listener(|this, _: &PrevScene, _, cx| this.act(cx, |m| m.jump_scene(-1))))
            .on_action(cx.listener(|this, _: &NextMarker, _, cx| this.act(cx, |m| m.jump_marker(1))))
            .on_action(cx.listener(|this, _: &PrevMarker, _, cx| this.act(cx, |m| m.jump_marker(-1))))
            .on_action(cx.listener(|this, _: &DuplicateTo, window, cx| {
                let seed = this.model.comp().map(|c| format!("{} clip", c.name)).unwrap_or_default();
                this.open_prompt(Ask::DuplicateTo, &seed, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Split, _, cx| this.act(cx, |m| {
                m.split_at_playhead();
            })))
            .on_action(cx.listener(|this, _: &RateDown, _, cx| this.act(cx, |m| m.set_rate(m.rate - 0.25))))
            .on_action(cx.listener(|this, _: &RateReset, _, cx| this.act(cx, |m| m.set_rate(1.0))))
            .on_action(cx.listener(|this, _: &RateUp, _, cx| this.act(cx, |m| m.set_rate(m.rate + 0.25))))
            .on_action(cx.listener(|this, _: &FrameBack, _, cx| this.act(cx, |m| m.step_frames(-1))))
            .on_action(cx.listener(|this, _: &FrameForward, _, cx| this.act(cx, |m| m.step_frames(1))))
            .on_action(cx.listener(|this, _: &ToggleLayerHidden, _, cx| this.act(cx, |m| {
                m.edit_layer(|l| l.hidden = !l.hidden);
            })))
            .on_action(cx.listener(|this, _: &LayerForward, _, cx| this.act(cx, |m| {
                if let Some(id) = m.selected_layer {
                    m.run(Command::ReorderLayer { id, delta: 1 });
                }
            })))
            .on_action(cx.listener(|this, _: &LayerBackward, _, cx| this.act(cx, |m| {
                if let Some(id) = m.selected_layer {
                    m.run(Command::ReorderLayer { id, delta: -1 });
                }
            })))
            .child(top)
            .children(prompt)
            .children(find)
            .child(h_flex().flex_1().min_h_0().w_full().child(sidebar).child(script).child(right))
            .children(timeline)
            .child(status)
            .children(shortcuts)
            .children(export)
            .children(record)
    }
}
