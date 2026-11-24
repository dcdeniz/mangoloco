//! Background work: import, transcription, export, recording, assistant.

use crate::jobs::Completion;
use crate::workspace::*;
use gpui_kit::component::button::*;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mango_core::{AssetId, Command};

actions!(mango, [RecordDialog]);

/// What an export produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Mp4,
    Hevc,
    ProRes,
    WebM,
    Gif,
    Mp3,
    Wav,
    M4a,
    Srt,
    Vtt,
    Txt,
    Markdown,
    Html,
    Rtf,
    Edl,
    FcpXml,
    Chapters,
}

impl ExportKind {
    pub const MEDIA: [ExportKind; 8] = [ExportKind::Mp4, ExportKind::Hevc, ExportKind::ProRes, ExportKind::WebM, ExportKind::Gif, ExportKind::Mp3, ExportKind::Wav, ExportKind::M4a];
    pub const TEXT: [ExportKind; 9] =
        [ExportKind::Srt, ExportKind::Vtt, ExportKind::Txt, ExportKind::Markdown, ExportKind::Html, ExportKind::Rtf, ExportKind::Edl, ExportKind::FcpXml, ExportKind::Chapters];

    pub fn label(self) -> &'static str {
        match self {
            ExportKind::Mp4 => "MP4 (H.264)",
            ExportKind::Hevc => "MP4 (HEVC)",
            ExportKind::ProRes => "ProRes",
            ExportKind::WebM => "WebM",
            ExportKind::Gif => "GIF",
            ExportKind::Mp3 => "MP3",
            ExportKind::Wav => "WAV",
            ExportKind::M4a => "M4A",
            ExportKind::Srt => "Subtitles SRT",
            ExportKind::Vtt => "Subtitles VTT",
            ExportKind::Txt => "Transcript TXT",
            ExportKind::Markdown => "Transcript MD",
            ExportKind::Html => "Transcript HTML",
            ExportKind::Rtf => "Transcript RTF",
            ExportKind::Edl => "Timeline EDL",
            ExportKind::FcpXml => "Timeline FCPXML",
            ExportKind::Chapters => "Chapter list",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportKind::Mp4 | ExportKind::Hevc => "mp4",
            ExportKind::ProRes => "mov",
            ExportKind::WebM => "webm",
            ExportKind::Gif => "gif",
            ExportKind::Mp3 => "mp3",
            ExportKind::Wav => "wav",
            ExportKind::M4a => "m4a",
            ExportKind::Srt => "srt",
            ExportKind::Vtt => "vtt",
            ExportKind::Txt | ExportKind::Chapters => "txt",
            ExportKind::Markdown => "md",
            ExportKind::Html => "html",
            ExportKind::Rtf => "rtf",
            ExportKind::Edl => "edl",
            ExportKind::FcpXml => "fcpxml",
        }
    }

    pub fn is_media(self) -> bool {
        Self::MEDIA.contains(&self)
    }

    pub fn has_video(self) -> bool {
        matches!(self, ExportKind::Mp4 | ExportKind::Hevc | ExportKind::ProRes | ExportKind::WebM | ExportKind::Gif)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportChoice {
    pub kind: ExportKind,
    /// Output height; `None` = the composition's own size.
    pub height: Option<u32>,
    pub quality: mango_core::render::Quality,
    /// Export only the selected part of the script.
    pub selection_only: bool,
}

impl Default for ExportChoice {
    fn default() -> Self {
        Self { kind: ExportKind::Mp4, height: None, quality: mango_core::render::Quality::High, selection_only: false }
    }
}

/// Builds engine export settings for a media export.
pub fn export_settings(comp: &mango_core::Composition, choice: &ExportChoice, range: Option<(f64, f64)>) -> mango_core::render::ExportSettings {
    use mango_core::render::{AudioCodec, ExportSettings, VideoCodec};
    let mut s = ExportSettings::for_composition(comp);
    if let Some(h) = choice.height {
        if h < comp.height {
            s = s.with_height(comp, h);
        }
    }
    s.quality = choice.quality;
    s.range = range;
    let (v, a) = match choice.kind {
        ExportKind::Mp4 => (Some(VideoCodec::H264), Some(AudioCodec::Aac)),
        ExportKind::Hevc => (Some(VideoCodec::Hevc), Some(AudioCodec::Aac)),
        ExportKind::ProRes => (Some(VideoCodec::ProRes), Some(AudioCodec::Wav)),
        ExportKind::WebM => (Some(VideoCodec::Vp9), Some(AudioCodec::Opus)),
        ExportKind::Gif => (Some(VideoCodec::Gif), None),
        ExportKind::Mp3 => (None, Some(AudioCodec::Mp3)),
        ExportKind::Wav => (None, Some(AudioCodec::Wav)),
        _ => (None, Some(AudioCodec::Aac)),
    };
    s.video = v;
    s.audio = a;
    if choice.kind == ExportKind::Gif {
        // GIFs get large fast: cap the size and frame rate.
        if s.height > 480 {
            s = s.with_height(comp, 480);
        }
        s.fps = s.fps.min(15.0);
    }
    s.title = Some(comp.name.clone());
    s
}

/// Produces the text of a non-media export.
pub fn export_text(project: &mango_core::Project, comp: &mango_core::Composition, kind: ExportKind) -> String {
    use mango_core::captions::{self, CueOptions};
    use mango_core::transcript_export::{self as tx, TranscriptFormat, TranscriptOptions};
    let transcript = |format| tx::export_transcript(project, comp, &TranscriptOptions { format, ..Default::default() });
    let cues = || captions::cues(comp, &CueOptions::default());
    match kind {
        ExportKind::Srt => captions::to_srt(&cues(), Some(project), false),
        ExportKind::Vtt => captions::to_vtt(&cues(), Some(project), false),
        ExportKind::Txt => transcript(TranscriptFormat::Txt),
        ExportKind::Markdown => transcript(TranscriptFormat::Markdown),
        ExportKind::Html => transcript(TranscriptFormat::Html),
        ExportKind::Rtf => transcript(TranscriptFormat::Rtf),
        ExportKind::Edl => mango_core::timeline_export::to_edl(project, comp),
        ExportKind::FcpXml => mango_core::timeline_export::to_fcpxml(project, comp),
        ExportKind::Chapters => tx::chapters_text(comp),
        _ => String::new(),
    }
}

/// Converts straight RGBA to the premultiplied BGRA the renderer draws.
pub fn rgba_to_image(frame: &mango_media::Frame) -> Option<std::sync::Arc<RenderImage>> {
    let mut px = frame.rgba.clone();
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        let (r, g, b) = (p[0] as u32 * a / 255, p[1] as u32 * a / 255, p[2] as u32 * a / 255);
        p[0] = b as u8;
        p[1] = g as u8;
        p[2] = r as u8;
    }
    let buf = image::RgbaImage::from_raw(frame.width, frame.height, px)?;
    Some(std::sync::Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

/// Finds the speech model to use: the preferred one, else any installed one.
pub fn installed_model(preferred: &str) -> Option<std::path::PathBuf> {
    mango_speech::models::find(preferred).or_else(|| mango_speech::models::installed().into_iter().next().map(|(_, p)| p))
}

impl Workspace {
    // --- import ---------------------------------------------------------------

    pub(crate) fn import_dialog(&mut self, _: &ImportMedia, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Import".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                this.update_in(cx, |this, window, cx| this.import_paths(paths, window, cx)).ok();
            }
        })
        .detach();
    }

    /// Imports files: project files open as projects, everything else is
    /// probed in the background and added to the project.
    pub fn import_paths(&mut self, paths: Vec<std::path::PathBuf>, _window: &mut Window, cx: &mut Context<Self>) {
        for path in paths {
            if path.extension().is_some_and(|e| e == "mango") {
                if self.model.editor.is_dirty() {
                    self.model.fail("Save this project first (Cmd+S), then open another");
                } else {
                    self.load_project(path, cx);
                }
                continue;
            }
            if self.model.editor.project.assets.iter().any(|a| a.path == path) {
                self.model.say(format!("{} is already in the project", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
                continue;
            }
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            self.jobs.start(format!("Importing {name}"), move |_job| -> Completion {
                let result = mango_media::probe::probe(&path);
                Box::new(move |ws, window, cx| {
                    match result {
                        Ok(asset) => ws.add_imported(asset, window, cx),
                        Err(e) => ws.model.fail(format!("Could not import {name}: {e:#}")),
                    }
                    cx.notify();
                })
            });
        }
        cx.notify();
    }

    fn add_imported(&mut self, asset: mango_core::MediaAsset, window: &mut Window, cx: &mut Context<Self>) {
        let id = asset.id;
        let is_image = asset.kind == mango_core::MediaKind::Image;
        let has_audio = asset.has_audio;
        let name = asset.name.clone();
        // The first recording goes straight into an empty script, as one undo step with the import.
        let into_script = !is_image && self.model.comp().is_some_and(|c| c.script.is_empty());
        let mut cmds = vec![Command::AddAsset { asset }];
        if into_script {
            cmds.push(Command::InsertAsset { asset: id, at: None });
        }
        match self.model.editor.apply_all("Import", cmds) {
            Ok(()) => {
                self.model.after_edit();
                self.model.say(format!("Imported {name}"));
                if has_audio && self.settings.transcribe_on_import {
                    self.transcribe_asset(id, window, cx);
                }
            }
            Err(e) => self.model.fail(format!("Could not import {name}: {e:#}")),
        }
    }

    // --- overlay and still pictures --------------------------------------------

    /// Re-rasterises titles, shapes and captions when what should be on
    /// screen changed, and loads pictures for image layers.
    pub(crate) fn refresh_overlay(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.model.comp() else { return };
        let t = self.model.playhead;
        let rev = self.model.editor.revision;
        if self.overlay.cues_revision != rev || self.overlay.cues_comp != self.model.editor.active {
            self.overlay.cues = if c.captions.enabled { mango_media::raster::caption_cues(c) } else { Vec::new() };
            self.overlay.cues_revision = rev;
            self.overlay.cues_comp = self.model.editor.active;
        }
        // What is visible now, reduced to a signature so playback only
        // re-rasterises when something actually changes.
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        rev.hash(&mut h);
        self.model.editor.active.hash(&mut h);
        let mut any = false;
        let mut animated = false;
        for l in &c.layers {
            let generated = matches!(l.content, mango_core::LayerContent::Text(_) | mango_core::LayerContent::Shape(_) | mango_core::LayerContent::Waveform(_) | mango_core::LayerContent::Progress { .. });
            if generated && !l.hidden && t >= l.start && t < l.end() {
                any = true;
                l.id.hash(&mut h);
                animated |= matches!(l.content, mango_core::LayerContent::Waveform(_) | mango_core::LayerContent::Progress { .. });
            }
        }
        if let Some((i, cue)) = self.overlay.cues.iter().enumerate().find(|(_, q)| t >= q.start && t < q.end) {
            any = true;
            i.hash(&mut h);
            if c.captions.highlight != mango_core::CaptionHighlight::None {
                cue.words.iter().rposition(|w| t >= w.start).hash(&mut h);
            }
        }
        if animated {
            ((t * 15.0) as i64).hash(&mut h);
        }
        let sig = h.finish();
        if sig != self.overlay.signature {
            self.overlay.signature = sig;
            let new = if any {
                // Rasterise at the size the frame is drawn at (capped), keeping the canvas aspect.
                let fb = self.canvas.frame_bounds.get();
                let scale = window_scale_hint();
                let w = ((f32::from(fb.size.width) * scale) as u32).clamp(320, 1280);
                let hh = ((w as f64 * c.height as f64 / c.width.max(1) as f64).round() as u32).max(2);
                let frame = mango_media::raster::render_overlay(&self.model.editor.project, c, &self.overlay.cues, t, w, hh);
                rgba_to_image(&frame)
            } else {
                None
            };
            if let Some(old) = std::mem::replace(&mut self.canvas.overlay, new) {
                cx.drop_image(old, None);
            }
        }
        // Image layers: decode each file once.
        let wanted: Vec<(mango_core::LayerId, std::path::PathBuf)> = c
            .layers
            .iter()
            .filter_map(|l| match &l.content {
                mango_core::LayerContent::Image(im) => self.model.editor.project.asset(im.asset).map(|a| (l.id, a.path.clone())),
                _ => None,
            })
            .collect();
        for (id, path) in wanted {
            if self.canvas.layer_pictures.iter().any(|(l, _)| *l == id) {
                continue;
            }
            if !self.overlay.stills.contains_key(&path) {
                let img = mango_media::frames::frame_at(&path, 0.0, 1280, 1280).ok().and_then(|f| rgba_to_image(&f));
                self.overlay.stills.insert(path.clone(), img);
            }
            if let Some(Some(img)) = self.overlay.stills.get(&path) {
                self.canvas.layer_pictures.push((id, img.clone()));
            }
        }
    }

    // --- export ---------------------------------------------------------------

    pub(crate) fn open_export(&mut self, _: &ExportDialog, _window: &mut Window, cx: &mut Context<Self>) {
        if self.model.comp().is_none_or(|c| c.duration() <= 0.0) {
            self.model.fail("There is nothing to export yet. Import a recording first");
        } else {
            self.export_panel = Some(self.export_panel.take().unwrap_or_default());
        }
        cx.notify();
    }

    fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = self.export_panel.clone() else { return };
        let Some(c) = self.model.comp() else { return };
        let dir = self.model.editor.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())).or_else(|| std::env::home_dir().map(|h| h.join("Movies"))).unwrap_or_default();
        let name = format!("{}.{}", c.name.replace(['/', ':'], "-"), choice.kind.extension());
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut path))) = rx.await {
                if path.extension().is_none() {
                    path.set_extension(choice.kind.extension());
                }
                this.update_in(cx, |this, _window, cx| this.export_to(path, choice, cx)).ok();
            }
        })
        .detach();
    }

    /// Runs an export to `path`. Text formats are written immediately; media
    /// exports run as a background job.
    pub fn export_to(&mut self, path: std::path::PathBuf, choice: ExportChoice, cx: &mut Context<Self>) {
        let Some(c) = self.model.comp().cloned() else { return };
        let project = self.model.editor.project.clone();
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.export_panel = None;
        if !choice.kind.is_media() {
            let text = export_text(&project, &c, choice.kind);
            match std::fs::write(&path, text) {
                Ok(()) => self.model.say(format!("Exported {file}")),
                Err(e) => self.model.fail(format!("Could not write {file}: {e}")),
            }
            cx.notify();
            return;
        }
        let sel = self.model.selection();
        let range = (choice.selection_only && !sel.is_empty()).then(|| (c.time_of_token(sel.start), c.time_of_token(sel.end)));
        let settings = export_settings(&c, &choice, range);
        self.jobs.start(format!("Exporting {file}"), move |job| -> Completion {
            let result = mango_media::export::export(
                &project,
                &c,
                &settings,
                &path,
                &mut |p| {
                    job.set_progress(p.fraction as f32);
                    job.set_note(match p.stage {
                        mango_media::export::Stage::Preparing => "Preparing".to_string(),
                        mango_media::export::Stage::Chunk { index, of } => format!("Cutting the script, part {} of {}", index + 1, of),
                        mango_media::export::Stage::Final => "Encoding".to_string(),
                    });
                },
                &job.cancel,
            );
            Box::new(move |ws, _window, cx| {
                match result {
                    Ok(report) => {
                        ws.model.say(format!("Exported {file} ({}, {:.1} MB)", crate::model::format_time(report.duration), report.bytes as f64 / 1_000_000.0));
                        ws.last_export = Some(report.path);
                    }
                    Err(e) if mango_media::is_cancelled(&e) => ws.model.say("Export cancelled"),
                    Err(e) => ws.model.fail(format!("Export failed: {e:#}")),
                }
                cx.notify();
            })
        });
        cx.notify();
    }

    pub(crate) fn render_export_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let choice = self.export_panel.clone().unwrap_or_default();
        let has_selection = !self.model.selection().is_empty();
        let comp_h = self.model.comp().map(|c| c.height).unwrap_or(1080);
        let chip = |id: (&'static str, usize), text: String, on: bool, set: Box<dyn Fn(&mut ExportChoice)>, cx: &mut Context<Self>| {
            let b = Button::new(id).small().label(text).on_click(cx.listener(move |this, _, _, cx| {
                if let Some(c) = &mut this.export_panel {
                    set(c);
                }
                cx.notify();
            }));
            if on { b.primary() } else { b.outline() }
        };
        let mut media = h_flex().flex_wrap().gap_1();
        for (i, k) in ExportKind::MEDIA.into_iter().enumerate() {
            media = media.child(chip(("exp-kind", i), k.label().to_string(), choice.kind == k, Box::new(move |c| c.kind = k), cx));
        }
        let mut text = h_flex().flex_wrap().gap_1();
        for (i, k) in ExportKind::TEXT.into_iter().enumerate() {
            text = text.child(chip(("exp-text", i), k.label().to_string(), choice.kind == k, Box::new(move |c| c.kind = k), cx));
        }
        let mut sizes = h_flex().flex_wrap().gap_1().child(chip(("exp-size", 0), format!("Original ({comp_h}p)"), choice.height.is_none(), Box::new(|c| c.height = None), cx));
        for (i, h) in [2160u32, 1080, 720, 480].into_iter().enumerate() {
            if h < comp_h {
                sizes = sizes.child(chip(("exp-size", i + 1), format!("{h}p"), choice.height == Some(h), Box::new(move |c| c.height = Some(h)), cx));
            }
        }
        use mango_core::render::Quality;
        let mut quality = h_flex().gap_1();
        for (i, (q, n)) in [(Quality::High, "High"), (Quality::Medium, "Medium"), (Quality::Low, "Small file")].into_iter().enumerate() {
            quality = quality.child(chip(("exp-q", i), n.to_string(), choice.quality == q, Box::new(move |c| c.quality = q), cx));
        }
        let label = |s: &'static str| div().pt_3().pb_1().text_xs().font_weight(FontWeight::BOLD).text_color(t.muted_foreground).child(s);
        div()
            .id("export-scrim")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui_kit::black().opacity(0.5))
            .child(
                v_flex()
                    .id("export-panel")
                    .test_support()
                    .w(px(560.))
                    .p_5()
                    .rounded_lg()
                    .bg(t.popover)
                    .border_1()
                    .border_color(t.border)
                    .child(div().text_lg().font_weight(FontWeight::BOLD).child("Export"))
                    .child(label("VIDEO AND AUDIO"))
                    .child(media)
                    .child(label("TEXT AND TIMELINES"))
                    .child(text)
                    .when(choice.kind.has_video() && choice.kind != ExportKind::Gif, |d| d.child(label("SIZE")).child(sizes))
                    .when(choice.kind.is_media() && !matches!(choice.kind, ExportKind::Wav | ExportKind::Gif), |d| d.child(label("QUALITY")).child(quality))
                    .when(choice.kind.is_media() && has_selection, |d| {
                        d.child(label("RANGE")).child(
                            h_flex()
                                .gap_1()
                                .child(chip(("exp-range", 0), "Whole composition".into(), !choice.selection_only, Box::new(|c| c.selection_only = false), cx))
                                .child(chip(("exp-range", 1), "Selection only".into(), choice.selection_only, Box::new(|c| c.selection_only = true), cx)),
                        )
                    })
                    .child(
                        h_flex()
                            .pt_5()
                            .gap_2()
                            .justify_end()
                            .child(Button::new("export-cancel").ghost().label("Cancel").on_click(cx.listener(|this, _, _, cx| {
                                this.export_panel = None;
                                cx.notify();
                            })))
                            .child(Button::new("export-go").primary().label("Export…").on_click(cx.listener(|this, _, window, cx| this.start_export(window, cx)))),
                    ),
            )
    }

    pub(crate) fn render_jobs(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let rows: Vec<(u64, String, f32, String)> = self.jobs.list.iter().map(|j| (j.id, j.label.clone(), j.shared.progress(), j.shared.note())).collect();
        v_flex().w_full().children(rows.into_iter().map(|(id, label, progress, note)| {
            v_flex()
                .id(("job", id as usize))
                .test_support()
                .mx_3()
                .mt_2()
                .p_2()
                .gap_1()
                .rounded_md()
                .bg(t.muted)
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().text_xs().font_weight(FontWeight::SEMIBOLD).truncate().child(label))
                        .child(div().text_xs().text_color(t.muted_foreground).child(format!("{:.0}%", progress * 100.0)))
                        .child(Button::new(("job-cancel", id as usize)).ghost().xsmall().label("Cancel").on_click(cx.listener(move |this, _, _, cx| {
                            this.jobs.cancel(id);
                            cx.notify();
                        }))),
                )
                .child(Progress::new(("job-progress", id as usize)).value(progress * 100.0))
                .when(!note.is_empty(), |d| d.child(div().text_xs().text_color(t.muted_foreground).truncate().child(note)))
        }))
    }

    /// Transcribes a project file in the background, downloading the speech
    /// model first if none is installed.
    pub fn transcribe_asset(&mut self, id: AssetId, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(asset) = self.model.editor.project.asset(id) else { return };
        if !asset.has_audio {
            self.model.fail(format!("{} has no sound to transcribe", asset.name));
            cx.notify();
            return;
        }
        let (path, name) = (asset.path.clone(), asset.name.clone());
        let glossary: Vec<String> = self.model.editor.project.glossary.iter().map(|g| g.term.clone()).collect();
        let preferred = self.settings.speech_model.clone();
        let language = Some(self.settings.language.trim().to_string()).filter(|l| !l.is_empty());
        self.jobs.start(format!("Transcribing {name}"), move |job| -> Completion {
            let result = (|| -> anyhow::Result<mango_core::Transcript> {
                let (model, downloaded) = match installed_model(&preferred) {
                    Some(m) => (m, false),
                    None => {
                        let info = mango_speech::models::catalog()
                            .into_iter()
                            .find(|m| m.id == preferred)
                            .or_else(|| mango_speech::models::catalog().into_iter().find(|m| m.id == "base.en"))
                            .ok_or_else(|| anyhow::anyhow!("no speech model is available to download"))?;
                        job.set_note(format!("Downloading the {} speech model ({} MB), once", info.label, info.bytes / 1_000_000));
                        let m = mango_speech::models::download(&info, &mut |p| job.set_progress(p * 0.5), &job.cancel)?;
                        (m, true)
                    }
                };
                job.set_note("Listening");
                let mut opts = mango_speech::transcribe::Options::new(model);
                opts.language = language;
                opts.glossary = glossary;
                let base = if downloaded { 0.5 } else { 0.0 };
                mango_speech::transcribe::transcribe(&path, &opts, &mut |p| job.set_progress(base + p * (1.0 - base)), &job.cancel)
            })();
            Box::new(move |ws, _window, cx| {
                match result {
                    Ok(transcript) => {
                        let n = transcript.words.len();
                        if ws.model.run(Command::SetTranscript { asset: id, transcript }) || n == 0 {
                            ws.model.say(if n == 0 { format!("No speech found in {name}") } else { format!("Transcribed {name}: {} words", crate::model::group_thousands(n)) });
                        }
                    }
                    Err(e) if mango_speech::is_cancelled(&e) => ws.model.say("Transcription cancelled"),
                    Err(e) => ws.model.fail(format!("Could not transcribe {name}: {e:#}")),
                }
                cx.notify();
            })
        });
        cx.notify();
    }
}

/// Pixel density to rasterise overlays at.
fn window_scale_hint() -> f32 {
    2.0
}

/// Cached state for the rasterised overlay.
#[derive(Default)]
pub struct OverlayState {
    pub cues: Vec<mango_core::captions::Cue>,
    pub cues_revision: u64,
    pub cues_comp: Option<mango_core::CompositionId>,
    pub signature: u64,
    /// Decoded still images by file, `None` when the file could not be read.
    pub stills: std::collections::HashMap<std::path::PathBuf, Option<std::sync::Arc<RenderImage>>>,
}
