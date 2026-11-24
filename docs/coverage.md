# Coverage against the feature inventory

Status of every item in `feature-inventory.md` (335 items, from Descript's help centre) as of 2026-11-23.

- **Tested**: built, and exercised by an automated test.
- **Built**: built and reachable in the app, no automated test of its own.
- **Partial**: some of it exists; the note says what is missing.
- **Not built**: absent. Most are cloud, multi-user or third-party-ML features that a local app cannot reproduce; the rest were not reached.

Nothing here is stubbed: a feature that is not built has no button pretending otherwise.

## Summary

| Area | Tested | Built | Partial | Not built |
|---|---|---|---|---|
| Projects (PRJ, 26) | 5 | 0 | 4 | 17 |
| Media (MED, 22) | 1 | 4 | 3 | 14 |
| Layouts (LAY, 15) | 0 | 0 | 1 | 14 |
| Recording (REC, 34) | 0 | 1 | 2 | 31 |
| Transcription (TRN, 22) | 4 | 3 | 6 | 9 |
| Script editing (TXT, 26) | 10 | 6 | 7 | 3 |
| Sequences (SEQ, 12) | 0 | 0 | 0 | 12 |
| Timeline (TML, 28) | 7 | 10 | 1 | 10 |
| Canvas (CNV, 38) | 3 | 10 | 9 | 16 |
| Audio (AUD, 17) | 2 | 4 | 5 | 6 |
| AI (AI, 53) | 9 | 4 | 9 | 31 |
| Collaboration (COL, 8) | 0 | 0 | 1 | 7 |
| Export (EXP, 18) | 3 | 1 | 6 | 8 |
| Preferences (PRF, 11) | 0 | 1 | 2 | 8 |
| **Total (330)** | **44** | **44** | **56** | **186** |

So 144 of 330 items exist in some form (88 fully, 56 in part) and 186 do not.

The five plan-limit items (PLN) do not apply: there are no plans, credits, watermarks or caps.

## Projects

- **Tested:** PRJ-01 project container · PRJ-02 several compositions per project, non-destructive · PRJ-04 new project and composition · PRJ-22 word count and duration of selection or script · PRJ-25 mixed sample rates (everything is resampled to 48 kHz)
- **Partial:** PRJ-03 composition actions (rename, duplicate, delete, "duplicate to"; no audio-only toggle or flatten) · PRJ-06 aspect ratio (five presets; no custom pixel size, always "fill") · PRJ-07 resolution and frame rate (frame rate on the composition, resolution chosen at export) · PRJ-20 duplicate (a selection to a new composition; not between projects)
- **Not built:** PRJ-05 audio-only compositions · PRJ-08 to PRJ-15 drive, workspaces, folders, trash, search · PRJ-16 autosave (saving is manual, Cmd+S, written atomically) · PRJ-17 to PRJ-19 version history, conflict handling, backups · PRJ-21 multiple windows · PRJ-23 project settings · PRJ-24 proxies · PRJ-26 ownership

## Media

- **Tested:** MED-05 multichannel audio downmixed to stereo
- **Built:** MED-02 audio formats and MED-03 video formats (whatever the installed ffmpeg reads) · MED-04 images (PNG, JPEG, GIF, HEIC and others; no PDF) · MED-07 activity list with progress and cancel
- **Partial:** MED-01 import (file picker, drag from Finder, command line; no link, cloud or partner imports) · MED-08 project panel (file list only; no folders, search, filter or sort) · MED-09 file actions (transcribe, re-transcribe, add to script, add as layer, remove; none of the others)
- **Not built:** MED-06 PDF slides · MED-10 to MED-15 replace, repair, sync offset, flatten, detach audio · MED-16 to MED-19 media library and stock media · MED-20 visual roles · MED-21 save frame as PNG · MED-22 partner imports

## Layouts and templates

- **Partial:** LAY-01 scene layouts (six fixed placements of the speaker video per scene: full, fit, left half, right half, corner, framed; no packs, fonts or placeholders)
- **Not built:** LAY-02 to LAY-15

## Recording

- **Built:** REC-02 camera, screen and microphone capture through ffmpeg. The microphone was recorded for real during testing. Screen and camera recording have **never been run**, because this development session lacked macOS Screen Recording permission. No system audio.
- **Partial:** REC-01 (one recorder, opened from the Project tab) · REC-04 controls (start, stop, discard; no pause or restart)
- **Not built:** REC-03 record into script at the playhead or as a replacement (recordings arrive as files) · REC-05 to REC-15 framing aids, menu-bar recorder, region capture, quick share, teleprompter, Studio Sound while recording, recovery · REC-20 to REC-38 Rooms and remote recording

## Transcription

- **Tested:** TRN-01 automatic transcription on import and on demand, on this Mac (whisper.cpp) · TRN-11 correct one word or phrase, or every instance ("Correct everywhere") · TRN-14 remove from transcript ("Hide text") · TRN-15 glossary (edited in the Tools tab, applied on transcription and fed to the recogniser as a hint)
- **Built:** TRN-04 re-transcribe · TRN-09 speaker labels (add or change for a paragraph or selection, rename) · TRN-21 uncertain words are underlined
- **Partial:** TRN-02 languages (multilingual models are in the catalogue; only the English models were tested) · TRN-03 language choice (a settings-file field, no UI) · TRN-05 model choice (settings file or `mango --model`; no UI) · TRN-08 speaker detection (a text-only guess by Claude under "Label speakers"; nothing listens to voices) · TRN-16 import your own transcript (the alignment code is tested but not connected to the UI) · TRN-17 word bar (shown on the timeline; boundaries cannot be dragged)
- **Not built:** TRN-06 · TRN-07 number formatting · TRN-10 speaker voices and avatars · TRN-12 separate correct mode · TRN-13 capitalisation and punctuation clicks · TRN-18 realign · TRN-19 multitrack · TRN-20 mic-bleed fix · TRN-22 split stereo

Word boundaries inside continuous speech can be 40 to 60 ms off; see the README's known issues.

## Script editing

- **Tested:** TXT-01 delete text to cut media · TXT-03 ignore · TXT-04 restore removed media · TXT-07 replace with gap clip · TXT-12 find in script · TXT-13 and TXT-14 delete or ignore every match · TXT-17 markers · TXT-18 scene boundaries with `/` · TXT-25 a new layer spans the selection
- **Built:** TXT-06 delete all ignored text · TXT-11 long silences shown as editable gap pills · TXT-16 bold and italic · TXT-20 toolbar above the script · TXT-23 the word being spoken is highlighted during playback · TXT-24 script text size
- **Partial:** TXT-02 cut, copy, paste (within a project; not across projects) · TXT-05 edit boundaries (computed and used by restore; not drawn in the script) · TXT-09 inline notes (stored and rendered; no way to add one in the UI) · TXT-10 word gaps (shorten all; setting one gap by hand exists as a command only) · TXT-15 highlights (one colour, toggle; no palette, filter or copy) · TXT-19 scenes (dividers in the script; no thumbnails or dragging) · TXT-26 (only the script track is transcribed, as in the original)
- **Not built:** TXT-08 write mode · TXT-21 replace script track · TXT-22 filler underlines · find across the project

## Sequences, multitrack, multicam

- **Not built:** SEQ-01 to SEQ-12. There are no sequences; each file is edited on its own.

## Timeline

- **Tested:** TML-05 move layers · TML-09 trim layer edges · TML-10 split at the playhead (script or selected layer) · TML-11 gap clips · TML-13 microfades at cuts · TML-23 transport (play, pause, scene jumps, speed) · TML-24 frame stepping
- **Built:** TML-01 show or hide · TML-03 zoom in, out, fit · TML-14 layer fades · TML-16 layer speed · TML-17 loop · TML-18 snapping to playhead, scenes and other layers · TML-19 layer order and hide · TML-20 markers and comments on the ruler · TML-21 level meter · TML-22 follows the playhead
- **Partial:** TML-07 blade (as "split at playhead"; no draggable edit points)
- **Not built:** TML-02 storyboard · TML-04 tool palette · TML-06 range tool · TML-08 slip · TML-12 freeze frame · TML-15 clip crossfades (scene transitions exist) · TML-25 reorder scenes · TML-26 · TML-27 · TML-28

## Canvas

- **Tested:** CNV-18 captions built from the script · CNV-19 caption styling including active-word highlight · CNV-29 scene transitions (seven: cut, cross dissolve, fade through black, wipe left and right, slide left and up)
- **Built:** CNV-02 layer types (video, audio, image, text, shape, waveform, progress bar, captions) · CNV-06 position and rotation · CNV-07 layer order · CNV-08 hide, lock, duplicate, delete · CNV-12 text (font, size, colour, alignment, outline, background, shadow) · CNV-16 background colour · CNV-22 waveform layer (bars, line, mirror) · CNV-23 progress bar (bar style) · CNV-24 in and out animations (fade, slide, zoom) · CNV-36 scene properties
- **Partial:** CNV-01 (select, drag, resize on the canvas, tested; no crop or floating toolbar) · CNV-03 layer properties (no border) · CNV-09 (single-layer align; no multi-select) · CNV-10 (snaps to the centre lines only) · CNV-14 shapes (no arrow) · CNV-15 colours (ten swatches; no gradients or eyedropper) · CNV-25 Ken Burns (a slow zoom on images) · CNV-32 and CNV-34 effects (thirteen on/off effects at fixed strength, applied on export only: green-screen key, blur, black and white, brightness, contrast, saturation, warmth, vignette, grain, pixelate, invert, mirror, rounded corners)
- **Not built:** CNV-04 · CNV-05 crop · CNV-11 replace media · CNV-13 dynamic text · CNV-17 custom fonts · CNV-20 per-speaker captions · CNV-21 · CNV-26 keyframes · CNV-27 text animations · CNV-28 kinetic typography · CNV-30 smart transitions · CNV-31 · CNV-33 effects drawer · CNV-35 paste attributes · CNV-37 editor layouts · CNV-38 avatars

The canvas preview shows video, images, text, shapes and captions exactly as export draws them. Visual effects and scene transitions are **not previewed**; they appear only in the exported file.

## Audio

- **Tested:** AUD-07 lower music under speech (ducking) · AUD-16 export loudness (LUFS targets and peak)
- **Built:** AUD-02 volume and mute per layer and for the script track · AUD-03 pan · AUD-05 levelling to −16 LUFS · AUD-13 composition loudness
- **Partial:** AUD-01 Studio Sound (a conventional clean-up chain: noise reduction, high-pass, de-ess, compression. It is not an AI model and does not remove echo) · AUD-04 volume keyframes (played and exported; no UI to draw them) · AUD-09 effects (compressor, limiter, EQ band, shelves, high-pass, low-pass, de-esser, gate, reverb as on/off presets; no graphic EQ; bitcrusher, distortion and flanger exist in the engine with no button) · AUD-10 and AUD-11 (fixed presets, no knobs)
- **Not built:** AUD-06 normalise as a file action · AUD-08 room tone · AUD-12 reorder effects · AUD-14 mono conversion · AUD-15 music and sound libraries · AUD-17

Audio effects are applied on export only; preview plays gain, mute, pan, fades and volume keys.

## AI

- **Tested (against the live API):** AI-01 chat co-editor · AI-07 one undo reverts a whole response · AI-21 remove filler words · AI-22 remove retakes · AI-23 shorten word gaps · AI-24 edit for clarity · AI-25 add chapters · AI-30 create clips · AI-32 summary, show notes, titles, YouTube description, social post, blog post
- **Built:** AI-20 tools panel · AI-33 brainstorm, outline and script drafts · AI-27 chroma key for real green screens (no AI background removal) · AI-50 text to speech with the Mac's system voices
- **Partial:** AI-02 what the assistant can do (script edits, captions, text layers, canvas size, transitions, chapters, clips; no translation, music, slides or web search) · AI-05 model choice (a settings-file field) · AI-08 chat history (kept for the session; not saved with the project) · AI-10 automation (the `mango` command-line tool; no API or MCP server) · AI-51 to AI-54 voices (system voices only, default voice, no picker) · AI-70 translate captions (to an SRT you can copy; not a translated composition)
- **Not built:** AI-03 · AI-04 context attachments · AI-06 skills · AI-09 · AI-26 eye contact · AI-28 centre active speaker · AI-29 quick design · AI-31 highlight reel · AI-34 video maker · AI-35 lighting and skin effects · AI-40 to AI-43 Regenerate · AI-55 voice cloning · AI-56 to AI-59 · AI-60 to AI-66 avatars and generated media · AI-71 to AI-75 dubbing, lip sync, translation options

## Collaboration

- **Partial:** COL-01 comments (add at the playhead or selection, resolve, delete, jump to; they follow their words through edits; no mentions; replies exist as a command only)
- **Not built:** COL-02 to COL-08. This is a single-user local app.

## Export

- **Tested:** EXP-02 video (MP4 H.264, MP4 HEVC, ProRes, WebM, GIF; size and quality) · EXP-05 audio (M4A, WAV, MP3) · EXP-09 transcript (TXT, Markdown, HTML, RTF; no DOCX)
- **Built:** EXP-01 export dialog
- **Partial:** EXP-03 scope (whole composition or the selection) · EXP-04 audio settings (48 kHz stereo, fixed) · EXP-06 metadata (title and chapters; no artwork) · EXP-07 subtitles in video (burned in through captions; no embedded subtitle track) · EXP-08 subtitles (SRT and VTT, tested; line-length and speaker options exist in the engine with no UI) · EXP-10 timelines (EDL and FCPXML, checked for structure and frame accuracy but **not imported into Premiere, Resolve or Final Cut**; no AAF, Premiere project or Audition session)
- **Not built:** EXP-11 · EXP-12 · EXP-13 to EXP-18 web publishing, YouTube and other destinations

## Preferences and shortcuts

- **Built:** PRF-02 light and dark
- **Partial:** PRF-01 settings (a JSON file plus an API-key prompt; no settings window) · PRF-09 action bar ("jump to time" with Cmd+K; no command palette)
- **Not built:** PRF-03 to PRF-08 · PRF-10 · PRF-11 (macOS only; no Windows or web)

Shortcuts follow the inventory's table where the feature exists: Space, Delete, Cmd+Delete (ignore), Shift+Cmd+Delete (restore), Shift+Delete (gap), C, /, #, @, S, arrows with Shift, Cmd+F/G, Shift+J/K/L, scene and marker jumps, Opt+Cmd+T/S, Shift+Cmd+M/H/C, Cmd+D, 0, [ ], Cmd+=/−/0, Cmd+Z. Opt+/ lists them in the app.
