//! `mango`: the editor without the window. Imports, transcribes, applies
//! edit commands and exports, using the same engine as the app.

use anyhow::{Context, Result, anyhow, bail};
use mango_core::{Command, Editor, MediaKind};
use mangoloco::services::{ExportChoice, ExportKind, export_settings, export_text, installed_model};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const USAGE: &str = "mango: edit audio and video by editing text

  mango new <project.mango> [name]
  mango import <project.mango> <file>... [--no-transcribe] [--model <id>]
  mango transcribe <project.mango> [--model <id>]
  mango info <project.mango>
  mango script <project.mango>              print the script with token numbers
  mango apply <project.mango> <command.json|json>...
  mango export <project.mango> <out> [--kind <kind>] [--height <px>]
  mango models                              list speech models

Export kinds: mp4 hevc prores webm gif mp3 wav m4a srt vtt txt md html rtf edl fcpxml chapters
(the kind is taken from the output extension when not given)";

fn open(path: &Path) -> Result<Editor> {
    Editor::load(path).with_context(|| format!("opening {}", path.display()))
}

fn flag(args: &mut Vec<String>, name: &str) -> bool {
    match args.iter().position(|a| a == name) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

fn option(args: &mut Vec<String>, name: &str) -> Result<Option<String>> {
    match args.iter().position(|a| a == name) {
        Some(i) => {
            if i + 1 >= args.len() {
                bail!("{name} needs a value");
            }
            args.remove(i);
            Ok(Some(args.remove(i)))
        }
        None => Ok(None),
    }
}

fn transcribe(editor: &mut Editor, id: mango_core::AssetId, model_id: &str) -> Result<usize> {
    let asset = editor.project.asset(id).ok_or_else(|| anyhow!("unknown file"))?.clone();
    let model = installed_model(model_id).ok_or_else(|| {
        anyhow!("no speech model is installed. Put a ggml model in {} (see `mango models`)", mango_speech::models::models_dir().display())
    })?;
    let mut opts = mango_speech::transcribe::Options::new(model);
    opts.glossary = editor.project.glossary.iter().map(|g| g.term.clone()).collect();
    let mut last = -1i32;
    let t = mango_speech::transcribe::transcribe(
        &asset.path,
        &opts,
        &mut |p| {
            let pct = (p * 100.0) as i32 / 10 * 10;
            if pct != last {
                last = pct;
                eprint!("\rTranscribing {} {pct}%", asset.name);
            }
        },
        &AtomicBool::new(false),
    )?;
    eprintln!();
    let n = t.words.len();
    editor.apply(Command::SetTranscript { asset: id, transcript: t })?;
    Ok(n)
}

fn kind_from(name: &str) -> Option<ExportKind> {
    Some(match name.to_lowercase().as_str() {
        "mp4" | "m4v" => ExportKind::Mp4,
        "hevc" => ExportKind::Hevc,
        "prores" | "mov" => ExportKind::ProRes,
        "webm" => ExportKind::WebM,
        "gif" => ExportKind::Gif,
        "mp3" => ExportKind::Mp3,
        "wav" => ExportKind::Wav,
        "m4a" => ExportKind::M4a,
        "srt" => ExportKind::Srt,
        "vtt" => ExportKind::Vtt,
        "txt" => ExportKind::Txt,
        "md" => ExportKind::Markdown,
        "html" => ExportKind::Html,
        "rtf" => ExportKind::Rtf,
        "edl" => ExportKind::Edl,
        "fcpxml" => ExportKind::FcpXml,
        "chapters" => ExportKind::Chapters,
        _ => return None,
    })
}

fn run(mut args: Vec<String>) -> Result<()> {
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" || args[0] == "help" {
        println!("{USAGE}");
        return Ok(());
    }
    let cmd = args.remove(0);
    if cmd == "models" {
        let installed = mango_speech::models::installed();
        println!("Models folder: {}", mango_speech::models::models_dir().display());
        for m in mango_speech::models::catalog() {
            let here = installed.iter().any(|(i, _)| i.id == m.id);
            println!("  {:<16} {:>5} MB  {}{}", m.id, m.bytes / 1_000_000, m.label, if here { "  [installed]" } else { "" });
        }
        return Ok(());
    }
    if args.is_empty() {
        bail!("missing project file\n\n{USAGE}");
    }
    let project = PathBuf::from(args.remove(0));
    match cmd.as_str() {
        "new" => {
            if project.exists() {
                bail!("{} already exists", project.display());
            }
            let name = args.first().cloned().unwrap_or_else(|| project.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into()));
            let mut e = Editor::new_project(&name);
            e.save(&project)?;
            println!("Created {}", project.display());
        }
        "import" => {
            let no_transcribe = flag(&mut args, "--no-transcribe");
            let model = option(&mut args, "--model")?.unwrap_or_else(|| "base.en".into());
            if args.is_empty() {
                bail!("nothing to import");
            }
            let mut e = if project.exists() { open(&project)? } else { Editor::new_project(&project.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()) };
            for f in args {
                let path = std::fs::canonicalize(&f).with_context(|| format!("{f} does not exist"))?;
                let asset = mango_media::probe::probe(&path)?;
                let (id, kind, name, has_audio) = (asset.id, asset.kind, asset.name.clone(), asset.has_audio);
                e.apply(Command::AddAsset { asset })?;
                if kind != MediaKind::Image && e.comp().is_some_and(|c| c.script.is_empty()) {
                    e.apply(Command::InsertAsset { asset: id, at: None })?;
                }
                println!("Imported {name}");
                if has_audio && !no_transcribe {
                    let n = transcribe(&mut e, id, &model)?;
                    println!("Transcribed {name}: {n} words");
                }
            }
            e.save(&project)?;
        }
        "transcribe" => {
            let model = option(&mut args, "--model")?.unwrap_or_else(|| "base.en".into());
            let mut e = open(&project)?;
            let ids: Vec<_> = e.project.assets.iter().filter(|a| a.has_audio).map(|a| a.id).collect();
            for id in ids {
                let n = transcribe(&mut e, id, &model)?;
                println!("{n} words");
            }
            e.save(&project)?;
        }
        "info" => {
            let e = open(&project)?;
            println!("{}: {} file(s), {} composition(s)", e.project.name, e.project.assets.len(), e.project.compositions.len());
            for a in &e.project.assets {
                println!("  file  {:<28} {:?} {:.2}s {}x{} {}", a.name, a.kind, a.duration, a.width, a.height, if a.transcript.is_some() { "transcribed" } else { "" });
            }
            for c in &e.project.compositions {
                println!("  comp  {:<28} {}x{} {:.2}s {} words {} layers {} scenes", c.name, c.width, c.height, c.duration(), c.word_count(0..c.script.len()), c.layers.len(), c.scene_spans().len());
            }
        }
        "script" => {
            let e = open(&project)?;
            let c = e.comp().ok_or_else(|| anyhow!("the project has no composition"))?;
            let starts = c.token_starts();
            for (i, t) in c.script.iter().enumerate() {
                if t.paragraph_start && i > 0 {
                    println!();
                }
                let text = match t.kind {
                    mango_core::TokenKind::Word => t.text.clone(),
                    mango_core::TokenKind::Gap => format!("<{:.2}s>", t.duration()),
                    mango_core::TokenKind::Blank => format!("<gap clip {:.2}s>", t.duration()),
                };
                println!("{i:>5} {:>8.2} {}{}", starts[i], if t.ignored { "~" } else { " " }, text);
            }
        }
        "apply" => {
            if args.is_empty() {
                bail!("give at least one command as JSON or a file containing JSON");
            }
            let mut e = open(&project)?;
            let mut cmds = Vec::new();
            for a in &args {
                let text = if Path::new(a).is_file() { std::fs::read_to_string(a)? } else { a.clone() };
                let text = text.trim();
                if text.starts_with('[') {
                    cmds.extend(serde_json::from_str::<Vec<Command>>(text).context("parsing the command list")?);
                } else {
                    cmds.push(serde_json::from_str::<Command>(text).with_context(|| format!("parsing command {text}"))?);
                }
            }
            let n = cmds.len();
            e.apply_all("Apply", cmds)?;
            e.save(&project)?;
            println!("Applied {n} command(s)");
        }
        "export" => {
            let kind_name = option(&mut args, "--kind")?;
            let height = option(&mut args, "--height")?.map(|h| h.parse::<u32>()).transpose().context("--height must be a number")?;
            let out = PathBuf::from(args.first().ok_or_else(|| anyhow!("missing output file"))?);
            let kind_name = kind_name.or_else(|| out.extension().map(|e| e.to_string_lossy().to_string())).ok_or_else(|| anyhow!("give --kind or an output file extension"))?;
            let kind = kind_from(&kind_name).ok_or_else(|| anyhow!("unknown export kind {kind_name}"))?;
            let e = open(&project)?;
            let c = e.comp().ok_or_else(|| anyhow!("the project has no composition"))?;
            if !kind.is_media() {
                std::fs::write(&out, export_text(&e.project, c, kind))?;
                println!("Wrote {}", out.display());
                return Ok(());
            }
            let settings = export_settings(c, &ExportChoice { kind, height, ..Default::default() }, None);
            let mut last = -1i32;
            let report = mango_media::export::export(
                &e.project,
                c,
                &settings,
                &out,
                &mut |p| {
                    let pct = (p.fraction * 100.0) as i32 / 5 * 5;
                    if pct != last {
                        last = pct;
                        eprint!("\rExporting {pct}%");
                    }
                },
                &AtomicBool::new(false),
            )?;
            eprintln!();
            println!("Wrote {} ({:.2}s, {:.1} MB)", report.path.display(), report.duration, report.bytes as f64 / 1e6);
        }
        other => bail!("unknown command {other}\n\n{USAGE}"),
    }
    Ok(())
}

fn main() {
    if let Err(e) = run(std::env::args().skip(1).collect()) {
        eprintln!("mango: {e:#}");
        std::process::exit(1);
    }
}
