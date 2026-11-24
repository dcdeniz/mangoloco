//! Recording (screen, camera, microphone) and text-to-speech.

use crate::jobs::Completion;
use crate::services::RecordDialog;
use crate::workspace::*;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mango_speech::record::{Devices, Options, Recorder};

#[derive(Default)]
pub struct CaptureState {
    pub open: bool,
    pub devices: Option<Devices>,
    pub screen: Option<u32>,
    pub camera: Option<u32>,
    pub microphone: Option<u32>,
    pub recorder: Option<Recorder>,
    pub error: Option<String>,
}

/// Where recordings and generated speech are kept: next to the project when
/// it has been saved, otherwise in the user's Movies folder.
pub fn media_dir(project: Option<&std::path::Path>) -> std::path::PathBuf {
    let dir = project
        .and_then(|p| p.parent())
        .map(|d| d.join("Media"))
        .or_else(|| std::env::home_dir().map(|h| h.join("Movies/Mango Loco")))
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{secs}")
}

impl Workspace {
    pub(crate) fn open_record(&mut self, _: &RecordDialog, _window: &mut Window, cx: &mut Context<Self>) {
        self.capture.open = true;
        self.capture.error = None;
        if self.capture.devices.is_none() {
            match mango_speech::record::devices() {
                Ok(d) => {
                    self.capture.microphone = d.microphones.first().map(|m| m.index);
                    self.capture.screen = d.screens.first().map(|s| s.index);
                    self.capture.devices = Some(d);
                }
                Err(e) => self.capture.error = Some(format!("Could not list capture devices: {e:#}")),
            }
        }
        cx.notify();
    }

    fn start_recording(&mut self, cx: &mut Context<Self>) {
        if self.capture.screen.is_none() && self.capture.camera.is_none() && self.capture.microphone.is_none() {
            self.capture.error = Some("Choose at least one thing to record".into());
            cx.notify();
            return;
        }
        let opts = Options {
            screen: self.capture.screen,
            camera: self.capture.camera,
            microphone: self.capture.microphone,
            fps: 30,
            capture_cursor: true,
            out_dir: media_dir(self.model.editor.path.as_deref()),
            base_name: format!("recording-{}", stamp()),
        };
        // Starting waits for the first data, so it can take a few seconds.
        match Recorder::start(&opts) {
            Ok(r) => {
                self.capture.recorder = Some(r);
                self.capture.error = None;
            }
            Err(e) => self.capture.error = Some(format!("{e:#}")),
        }
        cx.notify();
    }

    fn stop_recording(&mut self, keep: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rec) = self.capture.recorder.take() else { return };
        if keep {
            match rec.stop() {
                Ok(files) => {
                    self.capture.open = false;
                    self.import_paths(files, window, cx);
                }
                Err(e) => self.capture.error = Some(format!("{e:#}")),
            }
        } else if let Err(e) = rec.cancel() {
            self.capture.error = Some(format!("{e:#}"));
        }
        cx.notify();
    }

    pub(crate) fn render_record_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let recording = self.capture.recorder.as_ref().map(|r| r.elapsed());
        let label = |s: &'static str| div().pt_3().pb_1().text_xs().font_weight(FontWeight::BOLD).text_color(t.muted_foreground).child(s);
        let mut body = v_flex();
        if let Some(d) = &self.capture.devices {
            let pick = |id: &'static str, items: &[mango_speech::record::Device], current: Option<u32>, set: fn(&mut crate::services_capture::CaptureState, Option<u32>), cx: &mut Context<Self>| {
                let mut row = h_flex().flex_wrap().gap_1();
                let none = Button::new((id, 0usize)).small().label("None").disabled(recording.is_some()).on_click(cx.listener(move |this, _, _, cx| {
                    set(&mut this.capture, None);
                    cx.notify();
                }));
                row = row.child(if current.is_none() { none.primary() } else { none.outline() });
                for (i, dev) in items.iter().enumerate() {
                    let idx = dev.index;
                    let b = Button::new((id, i + 1)).small().label(dev.name.clone()).disabled(recording.is_some()).on_click(cx.listener(move |this, _, _, cx| {
                        set(&mut this.capture, Some(idx));
                        cx.notify();
                    }));
                    row = row.child(if current == Some(idx) { b.primary() } else { b.outline() });
                }
                row
            };
            body = body
                .child(label("SCREEN"))
                .child(pick("rec-screen", &d.screens, self.capture.screen, |c, v| c.screen = v, cx))
                .child(label("CAMERA"))
                .child(pick("rec-camera", &d.cameras, self.capture.camera, |c, v| c.camera = v, cx))
                .child(label("MICROPHONE"))
                .child(pick("rec-mic", &d.microphones, self.capture.microphone, |c, v| c.microphone = v, cx));
        }
        div().id("record-scrim").absolute().inset_0().flex().items_center().justify_center().bg(gpui_kit::black().opacity(0.5)).child(
            v_flex()
                .id("record-panel")
                .test_support()
                .w(px(560.))
                .p_5()
                .rounded_lg()
                .bg(t.popover)
                .border_1()
                .border_color(t.border)
                .child(div().text_lg().font_weight(FontWeight::BOLD).child("Record"))
                .child(div().text_xs().text_color(t.muted_foreground).child("Each source is saved as its own file and added to the project. macOS asks for Screen Recording, Camera and Microphone permission the first time."))
                .child(body)
                .when_some(self.capture.error.clone(), |d, e| d.child(div().id("record-error").test_support().pt_3().text_sm().text_color(crate::theme::hibiscus()).child(e)))
                .when_some(recording, |d, secs| d.child(div().pt_3().text_lg().text_color(crate::theme::hibiscus()).child(format!("● Recording {}", crate::model::format_time(secs)))))
                .child(
                    h_flex()
                        .pt_5()
                        .gap_2()
                        .justify_end()
                        .when(recording.is_none(), |d| {
                            d.child(Button::new("record-close").ghost().label("Close").on_click(cx.listener(|this, _, _, cx| {
                                this.capture.open = false;
                                cx.notify();
                            })))
                            .child(Button::new("record-start").primary().label("Start recording").on_click(cx.listener(|this, _, _, cx| this.start_recording(cx))))
                        })
                        .when(recording.is_some(), |d| {
                            d.child(Button::new("record-discard").ghost().label("Discard").on_click(cx.listener(|this, _, window, cx| this.stop_recording(false, window, cx))))
                                .child(Button::new("record-stop").primary().label("Stop and add to project").on_click(cx.listener(|this, _, window, cx| this.stop_recording(true, window, cx))))
                        }),
                ),
        )
    }

    /// Generates speech from text with a system voice and imports it.
    pub(crate) fn speak_text(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let text = text.trim().to_string();
        if text.is_empty() {
            return false;
        }
        let out = media_dir(self.model.editor.path.as_deref()).join(format!("speech-{}.wav", stamp()));
        self.jobs.start("Generating speech", move |_job| -> Completion {
            let result = mango_speech::tts::synthesize(&text, None, None, &out);
            Box::new(move |ws, window, cx| {
                match result {
                    Ok(_) => ws.import_paths(vec![out], window, cx),
                    Err(e) => ws.model.fail(format!("Could not generate speech: {e:#}")),
                }
                cx.notify();
            })
        });
        cx.notify();
        true
    }
}
