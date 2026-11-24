//! Opens the real app on a project, optionally seeks, and writes a PNG of
//! the window. Usage: screenshot <project.mango|-> <out.png> [seek secs] [side tab] [inspector tab]
use gpui_kit::*;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let project = args.get(1).filter(|p| p.as_str() != "-").map(std::path::PathBuf::from);
    let out = std::path::PathBuf::from(args.get(2).expect("output png"));
    let seek: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let side: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let insp: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx: &mut App| {
        mangoloco::init(cx);
        let (editor, err) = mangoloco::initial_editor(project.clone());
        if let Some(e) = err {
            eprintln!("{e}");
        }
        let (handle, ws) = mangoloco::open_main_window(editor, cx).expect("window");
        ws.update(cx, |ws, cx| {
            use mangoloco::model::{InspectorTab, SidePanel};
            ws.model.seek(seek);
            ws.model.side_panel = [SidePanel::Project, SidePanel::AiTools, SidePanel::Assistant, SidePanel::Comments][side.min(3)];
            ws.model.inspector = [InspectorTab::Composition, InspectorTab::Layer, InspectorTab::Captions, InspectorTab::Audio, InspectorTab::Scene][insp.min(4)];
            if insp == 1 {
                ws.model.selected_layer = ws.model.comp().and_then(|c| c.layers.first()).map(|l| l.id);
            }
            cx.notify();
        });
        cx.spawn(async move |cx: &mut AsyncApp| {
            // Give decoders and the rasteriser time to deliver.
            cx.background_executor().timer(Duration::from_millis(3500)).await;
            let _ = ws.update(cx, |ws, _| eprintln!("overlay={} picture={} layer_pics={} toast={:?} jobs={}", ws.canvas_debug().0, ws.canvas_debug().1, ws.canvas_debug().2, ws.model.toast, ws.jobs.list.len()));
            // The window may be behind others and not repainting; draw a fresh frame first.
            let img = cx.update_window(handle, |_, window, cx| {
                use gpui_kit::test::TestWindowExt;
                window.render_frame(cx);
                window.render_to_image()
            });
            match img {
                Ok(Ok(img)) => {
                    img.save(&out).expect("save png");
                    println!("{} {}x{}", out.display(), img.width(), img.height());
                }
                other => eprintln!("capture failed: {:?}", other.map(|r| r.map(|_| ()))),
            }
            let _ = ws.update(cx, |ws, _| ws.jobs.shutdown());
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
