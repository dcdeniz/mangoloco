use gpui_kit::*;
use mangoloco::workspace::*;

fn menus() -> Vec<Menu> {
    vec![
        Menu::new("Mango Loco").items([
            MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
            MenuItem::action("Switch Light / Dark", ToggleTheme),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Mango Loco", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Project", NewProject),
            MenuItem::action("New Composition", NewComposition),
            MenuItem::action("Open…", OpenProject),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::action("Save As…", SaveAs),
            MenuItem::separator(),
            MenuItem::action("Import Media…", ImportMedia),
            MenuItem::action("Export…", ExportDialog),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Undo", Undo),
            MenuItem::action("Redo", Redo),
            MenuItem::separator(),
            MenuItem::action("Cut", Cut),
            MenuItem::action("Copy", Copy),
            MenuItem::action("Paste", Paste),
            MenuItem::action("Select All", SelectAll),
            MenuItem::separator(),
            MenuItem::action("Ignore", Ignore),
            MenuItem::action("Restore Removed Media", Restore),
            MenuItem::action("Replace with Gap", ReplaceWithGap),
            MenuItem::action("Correct Text", Correct),
            MenuItem::separator(),
            MenuItem::action("Find…", Find),
            MenuItem::action("Jump to Time…", JumpToTime),
        ]),
        Menu::new("Insert").items([
            MenuItem::action("Scene Boundary", SceneBreak),
            MenuItem::action("Marker", Marker),
            MenuItem::action("Speaker Label", Speaker),
            MenuItem::action("Comment", AddComment),
            MenuItem::separator(),
            MenuItem::action("Text", AddText),
            MenuItem::action("Rectangle", AddRectangle),
        ]),
        Menu::new("View").items([
            MenuItem::action("Show / Hide Timeline", ToggleTimeline),
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Zoom to Fit", ZoomFit),
            MenuItem::separator(),
            MenuItem::action("Larger Script Text", ScriptBigger),
            MenuItem::action("Smaller Script Text", ScriptSmaller),
        ]),
        Menu::new("Playback").items([
            MenuItem::action("Play / Pause", TogglePlay),
            MenuItem::action("Split at Playhead", Split),
            MenuItem::separator(),
            MenuItem::action("Next Scene", NextScene),
            MenuItem::action("Previous Scene", PrevScene),
            MenuItem::action("Next Marker", NextMarker),
            MenuItem::action("Previous Marker", PrevMarker),
            MenuItem::separator(),
            MenuItem::action("Slower", RateDown),
            MenuItem::action("Normal Speed", RateReset),
            MenuItem::action("Faster", RateUp),
        ]),
    ]
}

fn main() {
    let path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx: &mut App| {
        mangoloco::init(cx);
        cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
        cx.set_menus(menus());
        let (editor, error) = mangoloco::initial_editor(path);
        match mangoloco::open_main_window(editor, cx) {
            Ok((_, ws)) => {
                if let Some(e) = error {
                    ws.update(cx, |ws, cx| {
                        ws.model.fail(e);
                        cx.notify();
                    });
                }
            }
            Err(e) => {
                eprintln!("Mango Loco could not open its window: {e:#}");
                cx.quit();
            }
        }
        cx.activate(true);
    });
}
