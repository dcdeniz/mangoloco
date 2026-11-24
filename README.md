# Mango Loco

A local macOS editor for audio and video that you edit by editing text: import a recording, it is transcribed on your Mac, and deleting words from the script cuts the media. Written in Rust with GPUI.

It is an original implementation in the same genre as Descript, built against a public feature inventory (`docs/feature-inventory.md`). `docs/coverage.md` says, item by item, what exists and what does not.

## Run it

Requirements: macOS on Apple Silicon, Rust (the pinned toolchain installs itself through `rust-toolchain.toml`), Xcode command-line tools, cmake, and ffmpeg (`brew install ffmpeg`).

```sh
cd crates/app
cargo run --release                 # the app
cargo run --release --bin mango -- --help   # the same engine without a window
./../../scripts/bundle.sh           # dist/Mango Loco.app
```

Speech models are downloaded on first transcription (about 148 MB for the default English model) into `~/Library/Application Support/MangoLoco/models`. To use models you already have, set `MANGO_MODELS_DIR`.

The assistant and AI tools use the Anthropic API. Add a key in the Assistant tab, or set `ANTHROPIC_API_KEY`. The key is stored in `~/Library/Application Support/MangoLoco/settings.json`, readable only by you.

## Layout

| Crate | What it does |
|---|---|
| `crates/core` | The editing engine: document model, text-based edits, undo, captions, transcript and timeline export, export planning. No I/O beyond project files. |
| `crates/media` | ffmpeg: probing, waveforms, still frames, the text/shape/caption rasteriser, the export runner. |
| `crates/speech` | Transcription (whisper.cpp), transcript alignment, text to speech, recording. |
| `crates/playback` | Live preview: audio engine and frame-accurate video decoding. |
| `crates/ai` | Claude: the chat co-editor and the one-shot tools. |
| `crates/app` | The GPUI app and the `mango` command-line tool. |

Every edit is a `mango_core::Command`. The window, the command line, the assistant and the tests all go through the same commands.

## Tests

```sh
cargo test                                   # core, media, speech, playback, ai (from the repo root)
cd crates/app && cargo test                  # app behaviour, headless UI, end to end
```

The media, speech and playback suites generate their own test recordings with ffmpeg and `say`. The speech and app end-to-end tests need the models in `models/`. Tests that call Claude run when a key is available and are skipped otherwise.

## Known issues

- **Word boundaries are approximate inside continuous speech.** Words after a pause start where the sound starts, and no word is shorter than is plausible. Inside a run of speech a boundary can be off by 40 to 60 ms: in the speech crate's measurement, cutting a single word out of the audio removed exactly that word for 48 of 61 words (all 16 that follow a pause). All of that was measured on synthesised speech. Use the timeline to nudge a cut that clips a neighbour.
- **Screen and camera recording have never been run.** The development session did not have macOS Screen Recording permission. Microphone recording was verified. Expect to debug the first real screen recording.
- **Effects and transitions are not previewed.** Visual effects, audio effects, scene transitions and ducking appear only in the exported file.
- **ffmpeg is not bundled.** Homebrew's ffmpeg links against Homebrew libraries, so the `.app` relies on ffmpeg being installed on the machine. Set `MANGO_BUNDLE_FFMPEG` to a folder of self-contained builds to include them.
- **The app is ad-hoc signed.** Other Macs will refuse to open it until it is signed with a Developer ID and notarised.
- **No autosave.** Save with Cmd+S.
- **The app icon is a Monster Energy product photo**, kept at the owner's request for internal use.
