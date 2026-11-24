//! Mango Loco desktop app.

pub mod canvas;
pub mod inspector;
pub mod jobs;
pub mod model;
pub mod preview;
pub mod script_view;
pub mod services;
pub mod services_ai;
pub mod services_capture;
pub mod settings;
pub mod sidebar;
pub mod theme;
pub mod timeline;
pub mod workspace;

use gpui_kit::*;
use mango_core::Editor;
use std::path::PathBuf;

/// Opens the main window on `editor`.
pub fn open_main_window(editor: Editor, cx: &mut App) -> anyhow::Result<(AnyWindowHandle, Entity<workspace::Workspace>)> {
    let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("Mango Loco".into()), appears_transparent: true, traffic_light_position: Some(point(px(14.), px(15.))) }),
        window_min_size: Some(size(px(1040.), px(640.))),
        ..Default::default()
    };
    let (handle, ws) = gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| workspace::Workspace::new(editor, window, cx)))?;
    Ok((handle, ws))
}

/// Loads the project at `path`, or starts an empty one.
pub fn initial_editor(path: Option<PathBuf>) -> (Editor, Option<String>) {
    match path {
        Some(p) => match Editor::load(&p) {
            Ok(e) => (e, None),
            Err(e) => (Editor::new_project("Untitled"), Some(format!("Could not open {}: {e:#}", p.display()))),
        },
        None => (Editor::new_project("Untitled"), None),
    }
}

pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    theme::apply(cx, true);
    workspace::bind_keys(cx);
}
