//! Writes a demo project with a synthetic transcript, for looking at the UI.
use mango_core::*;

fn main() {
    let out = std::env::args().nth(1).expect("usage: demo_project <out.mango>");
    let text = "Welcome back to the show. Um, today we're talking about how to edit video by editing text. \
So, uh, the idea is simple. You record, the recording gets transcribed, and then you just delete the words you don't want. \
I I think that's the part people love. You know, no more scrubbing through a timeline looking for one sentence. \
Let's start with the basics. First, import your recording. Then read through the script and cut anything that drags. \
After that we add captions, a title, and some music under the intro. \
That's really all there is to it. Thanks for watching, and see you next time.";
    let mut words = Vec::new();
    let mut t = 0.8;
    for (i, w) in text.split_whitespace().enumerate() {
        let d = 0.12 + 0.045 * w.len() as f64;
        words.push(TranscriptWord { text: w.to_string(), start: t, end: t + d, speaker: None, confidence: if i % 23 == 7 { 0.4 } else { 0.95 } });
        t += d + if w.ends_with('.') { if i % 3 == 0 { 1.6 } else { 0.5 } } else { 0.06 };
    }
    let asset = MediaAsset {
        id: AssetId::new(), name: "episode-12.mp4".into(), path: "episode-12.mp4".into(), kind: MediaKind::Video, duration: t + 1.0,
        width: 1920, height: 1080, fps: 30.0, has_audio: true, has_video: true, sample_rate: 48000, channels: 2, folder: None,
        transcript: Some(Transcript { language: "en".into(), words }),
    };
    let id = asset.id;
    let mut e = Editor::new_project("Episode 12");
    e.apply(Command::AddAsset { asset }).unwrap();
    e.apply(Command::InsertAsset { asset: id, at: None }).unwrap();
    let n = e.comp().unwrap().script.len();
    e.apply(Command::SetSpeaker { start: 0, end: n, speaker: Some("Deniz".into()) }).unwrap();
    let find = |e: &Editor, w: &str| e.comp().unwrap().script.iter().position(|t| t.text == w).unwrap();
    let i = find(&e, "Let's");
    e.apply(Command::SetSceneBreak { at: i, on: true }).unwrap();
    e.apply(Command::SetMarker { at: i, name: Some("The basics".into()) }).unwrap();
    let i = find(&e, "You");
    e.apply(Command::IgnoreRange { start: i, end: i + 3, ignored: true }).unwrap();
    let i = find(&e, "simple.");
    e.apply(Command::SetHighlight { start: i - 3, end: i + 1, color: Some(0) }).unwrap();
    let mut title = Layer::new("Title", 0.5, 4.0, LayerContent::Text(TextStyle { text: "Edit video by editing text".into(), ..Default::default() }));
    title.transform = Transform { x: 0.5, y: 0.2, w: 0.8, h: 0.2, ..Default::default() };
    title.track = 2;
    e.apply(Command::AddLayer { layer: title }).unwrap();
    let mut bar = Layer::new("Lower third", 6.0, 5.0, LayerContent::Shape(Shape { kind: ShapeKind::Rectangle, fill: Rgba::rgb(0xFF, 0x8A, 0x00), stroke: None, corner_radius: 0.2 }));
    bar.transform = Transform { x: 0.3, y: 0.82, w: 0.45, h: 0.1, ..Default::default() };
    e.apply(Command::AddLayer { layer: bar }).unwrap();
    e.apply(Command::AddComment { time: 12.0, author: "Deniz".into(), text: "Tighten this bit".into() }).unwrap();
    e.save(std::path::Path::new(&out)).unwrap();
    println!("{out}");
}
