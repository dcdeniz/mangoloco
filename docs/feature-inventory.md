# Descript feature inventory (as of Nov 2026)

## How this was built
- Source of truth was the official help center dump (`help.descript.com/llms.txt` index of ~340 articles and `llms-full.txt`). I read the full text of roughly 200 articles covering editing, visuals, timeline, effects, export, recording, AI speech, generative media, layouts, projects and collaboration.
- Pricing and changelog were read through a summarising fetch, so plan numbers and changelog items are faithful but not verbatim. The changelog was read back to Feb 2026 only.
- **Tags:** `[docs]` = read in an official source during this task. `[index]` = only the help-center index line was read, not the article body. `[recall]` = general knowledge, not verified here.
- **Not read:** the 67 SquadCast articles, the 25 developer/API reference pages, billing articles, and per-host publish articles (Buzzsprout, Castos, etc.).

## Things you asked about that the docs do not show
- **De-esser:** not in the audio effects list. Do not assume it exists.
- **Blend modes:** no mention anywhere I read; only opacity.
- **Layer grouping:** explicitly "not available yet" (except auto-created kinetic-typography groups).
- **Offline editing:** explicitly unsupported; everything is cloud-synced.
- **Accessibility:** there is no dedicated accessibility article. What exists is listed under PRF.
- **"Overdub":** the name no longer appears; the feature is now AI speech / AI Speakers / voice clones / Regenerate. `[recall]` for the rename.
- **"Ripple" and "headings":** the docs never use "ripple"; script edits close gaps by default and you opt out with "replace with gap clip". Markers act as section headers; there is no separate heading style.
- **Doc inconsistencies worth knowing:**
  - Transcription languages: 26 in help, 25 on pricing.
  - Transcription glossary: "all plans" in help, Business-only row on pricing.
  - Slip tool key: `Y` in the shortcut list, `S` in the timeline tools table.
  - Split: `S`, `Shift+S`, and `Cmd/Ctrl+Shift+S` are all cited in different articles.
  - Toggle timeline: `Shift+Cmd+S` versus `Ctrl+Opt+T`.
  - `Cmd+G`: "Find next" versus "adjust word gap at cursor".
  - Share-page resolutions include 1440p; local export lists 480p/720p/1080p/4K.

---

## PRJ — Projects, drive, workspaces, compositions
- **PRJ-01** [docs] Project = top-level container holding compositions, project files and sequences. Docs advise one project per episode/lesson for performance.
- **PRJ-02** [docs] Composition = an editable timeline/script within a project; multiple per project (raw, final, clips). Editing is non-destructive.
- **PRJ-03** [docs] Composition actions menu: rename, duplicate, copy link, open in new window, set to audio only, flatten, copy highlights, delete.
- **PRJ-04** [docs] New project from Drive view ("New project", choose type) or File > New > Project; `Cmd/Ctrl+N`. New composition `Opt+Cmd+N`.
- **PRJ-05** [docs] Video versus audio-only composition. Audio-only hides scenes and the scene editor. Importing audio into a blank composition auto-sets audio-only; adding video prompts a switch. Toggle via File menu; default type set in app settings.
- **PRJ-06** [docs] Aspect ratio menu: Landscape 16:9, Portrait 9:16, Square 1:1, Other (custom W×H px). Choose "Fit layers inside frame" or "Fill the frame" before changing. Applies to all scenes.
- **PRJ-07** [docs] Composition resolution presets 720p/1080p/1440p/4K, and a frame-rate dropdown. Export resolution cannot exceed composition resolution.
- **PRJ-08** [docs] Drive view sidebar: Home, Recents, Shared with me; workspaces list; tools (Brand Studio, Media library, Speakers, Layout packs, Learn); Trash.
- **PRJ-09** [docs] Workspaces: Personal (private, default for new projects), General (shared), Custom (Enterprise; open or private). Sharing a personal project moves it to a shared workspace.
- **PRJ-10** [docs] Project row actions: Rename, Duplicate, Move to…, Copy project link, Project access, Delete.
- **PRJ-11** [docs] Folders group projects inside a workspace; projects can move between workspaces, folders or drives.
- **PRJ-12** [docs] Trash: deleted projects kept 30 days, restorable, multi-delete.
- **PRJ-13** [docs] Drive-wide search: matches project names, file names, transcript content (snippet, click jumps to the moment), folders, layout packs. Full results page with type filter, sort by relevance/date, right-click actions.
- **PRJ-14** [docs] Visual search (Enterprise): describe on-screen content ("a dog at the beach"), get matched moments, "Add as new layer".
- **PRJ-15** [docs] Multiple drives per account with separate billing; drive selector to switch.
- **PRJ-16** [docs] Autosave to cloud, no Save button; sync spinner top-left with hover status.
- **PRJ-17** [docs] Version history (File > Version history): condensed or "Show all revisions", author and date per row, preview a version, Restore. Restore affects the whole project.
- **PRJ-18** [docs] Collaboration conflict handling: if local and cloud revisions cannot merge, a new revision with conflicting changes is created.
- **PRJ-19** [docs] Backups (Help & feedback > Debug): view automatic backups, export project backup (`.debackup`), import backup as a new project.
- **PRJ-20** [docs] Duplicate project ("(Copy)"), duplicate composition, "Duplicate to…" a selection into another composition, copy/paste script between projects via a second window.
- **PRJ-21** [docs] Multi-window editing: File > New > Window (or another browser tab).
- **PRJ-22** [docs] Duration and word count of selection (or whole script) shown at top of script editor.
- **PRJ-23** [docs] Project settings: optimize computer storage, automatic leveling on clips (-16 dB), room tone on new gap clips, show wordless media; can be saved as defaults for new projects.
- **PRJ-24** [docs] Proxy workflow: originals uploaded to cloud and used for export; smaller optimized versions streamed for editing.
- **PRJ-25** [docs] Mixed sample rates: in-app playback at 44.1k; timeline export upsamples to the highest rate in the project.
- **PRJ-26** [index] Change project ownership; drive ownership transfer via support.

## MED — File/media management and import
- **MED-01** [docs] Import methods: drag into script (upload + transcribe), Upload button in Project panel (no transcription), import by link, from other projects, Media library, Zoom cloud recordings, Google Drive (web only), PDF slides, partner "Edit in Descript", Zapier/API/MCP.
- **MED-02** [docs] In-app audio containers: WAV, MP3, AIFF, M4A, FLAC, OPUS, AAC. Unsupported: OGG, WMA. Codecs AAC, PCM 8/16/24/32-bit, Opus; ALAC unsupported.
- **MED-03** [docs] In-app video containers: MP4, M4V, MOV, WEBM, MKV. Codecs H.264, HEVC, VP8, VP9; ProRes and QTRLE on Creator+ only. MTS/AVI/WMV/MXF only via API. AV1 only inside MKV.
- **MED-04** [docs] Images: BMP, JPEG, PNG, GIF, WEBP, HEIC (AVIF added per May 2026 changelog). PDF for slides. DOCX/TXT/RTF cannot be imported.
- **MED-05** [docs] Files with 3+ audio channels are downmixed to stereo on import. HDR video supported in beta.
- **MED-06** [docs] PDF slide deck import: each page becomes a JPG in project files.
- **MED-07** [docs] Activity panel shows per-file upload progress; cancel one file or all. Resumable uploads (changelog).
- **MED-08** [docs] Project panel: Compositions and Files sections, folders and subfolders, auto folders "AI Assets" and "Saved keyframes", name search, filter by type, sort by name/date added/date modified.
- **MED-09** [docs] File actions: Rename, New composition from file, Duplicate, Create sequence, Detect speakers, Detect filler words, Convert to mono, Separate into mono files, Normalize audio, Detach audio, Replace (file or transcript), Set A/V sync offset, Fix file issue, Import transcript, Download, Delete, Transcribe / Re-transcribe.
- **MED-10** [docs] Replace file: swaps the source everywhere it is used, keeping edits; must be same duration/content; transcript not regenerated.
- **MED-11** [docs] Fix file issues: automatic analysis badges files (audio drift, unsupported codec); "Repair file" adds a fixed copy. Rate limited (10 per user per 10 min, 30 per project per hour).
- **MED-12** [docs] Repair audio drift for variable-frame-rate/screen-capture files.
- **MED-13** [docs] A/V sync offset slider per file (negative = audio earlier, positive = later).
- **MED-14** [docs] Flatten tracks: merge clips into one new file; "Script track only" or "All tracks".
- **MED-15** [docs] Detach audio: creates a sequence with separate video and audio tracks.
- **MED-16** [docs] Media library (drive-level): upload, add from project, import from Zoom/Google Drive, folders, type filters, remove. Reusing files costs no extra media minutes; transcribed automatically.
- **MED-17** [docs] Media panel "My Media" tab: browse drive, Media library, Brand Studio, workspace projects; search matches file names and spoken words; multi-select; "Add to project files" / "Add at playhead".
- **MED-18** [docs] Media panel "Stock" tab: stock videos, images, GIFs, stickers, backgrounds, music, sound effects (GIPHY and Storyblocks). GIPHY content is non-commercial.
- **MED-19** [docs] Stock search results capped by plan (first 5 Free, 12 Hobbyist, unlimited Creator+).
- **MED-20** [docs] Visual roles per file/layer: Talking Head, Screen Recording, Graphics, B-Roll/Other. Auto-assigned on import, editable; used by layouts.
- **MED-21** [docs] Save current frame as PNG to project files.
- **MED-22** [index] Imports from Captivate, Restream (beta), Ecamm, HubSpot, SquadCast via "Edit in Descript".

## LAY — Layouts and templates
- **LAY-01** [docs] Layout = scene-level design (speaker framing, fonts, colours, media, audio). Layout pack = themed collection grouped by type (Camera, Media, Chapter, Intro, Outro, Captions, Text, Other).
- **LAY-02** [docs] Apply from scene thumbnail > Layout, timeline/storyboard right-click "Change layout", scene toolbar, Properties panel, or selection toolbar "Change layout" (creates a scene from the selection).
- **LAY-03** [docs] Layout pack browser tabs: Gallery, Drive, Private; search; "Change layout pack" does not restyle already-applied scenes.
- **LAY-04** [docs] Matching rules: placeholders matched by visual role; camera and media slots interchangeable; matching by layer name; topmost layer wins a contested slot; unedited layers replaced, edited ones preserved.
- **LAY-05** [docs] Non-fitting layers are removed with a notification offering restore individual / Restore All.
- **LAY-06** [docs] Locked layers bypass layout matching entirely.
- **LAY-07** [docs] Smart Fill: AI fills Title/Subtitle text placeholders from script context; toggle per layout; dynamic text types unaffected.
- **LAY-08** [docs] Save scene as layout: per layer choose Keep as is / Don't include / Turn into placeholder; name and type.
- **LAY-09** [docs] Build a pack as a project (one scene per layout), "Publish layout pack"; visibility Private / Share with Drive / Share publicly.
- **LAY-10** [docs] Remix a gallery pack: pick base, primary/accent/background colours, Title/Subtitle/Text fonts, name and share.
- **LAY-11** [docs] Generate layout pack with AI from brand PDFs/images/fonts/logos and a prompt; four design directions, refine by prompt; costs 50 credits per directions run and 70 per pack.
- **LAY-12** [docs] Default layout pack per drive (Business+, via Brand Studio).
- **LAY-13** [docs] Placeholder layers (Media, Camera, Screen) with notes; replace via Add File / Record / Add Stock Media / Generate Media or drag-drop.
- **LAY-14** [docs] Multicam layouts: placeholder layers inserted when the layout has more tracks than the scene.
- **LAY-15** [docs] Prompt templates are being retired in favour of Underlord skills (cut-off 30 Nov 2026).

## REC — Recording
- **REC-01** [docs] Three recorders: Editor recorder (web + desktop), Screen Recorder (desktop only), Rooms (browser-based remote).
- **REC-02** [docs] Editor recorder inputs: Camera, Screen (full screen or window), Audio; computer audio system-wide on desktop, tab-only on web.
- **REC-03** [docs] Record targets: into script at playhead, replace selection (re-record over highlighted text), or record as new layer in a scene.
- **REC-04** [docs] Recorder controls: cancel/delete, restart, pause/resume, stop and save, settings. 4-hour limit per session. Background cloud upload with progress.
- **REC-05** [docs] Framing aids: "Center Me" silhouette overlay and Grid.
- **REC-06** [docs] Single system audio input only; multi-mic needs an aggregate device.
- **REC-07** [docs] Screen Recorder runs from menu bar/system tray; can be installed as standalone launcher; global shortcut `Cmd/Ctrl+Shift+2` (customisable).
- **REC-08** [docs] Screen Recorder inputs: display, application window, or screen region (drag, numeric W/H, aspect presets), camera, microphone, computer audio.
- **REC-09** [docs] Quick Share on: publish instantly to a share page as one combined track. Off: opens as a project with each input on its own track.
- **REC-10** [docs] Screen Recorder settings: 3-second countdown, camera resolution, transcribe microphone, Studio Sound, destination drive, default publish privacy, after-recording action, show author, enable comments, camera shape slider (square to circle), run in background, launch at login.
- **REC-11** [docs] Dock window: movable/resizable camera preview, pause/resume (not on Windows), stop + timer, restart, minimise, cancel. No 4K screen recording on Windows.
- **REC-12** [docs] Teleprompter: paste, type or import from script; font size, scroll speed (Variable or Constant), restart, play, flip/mirror, light/dark theme, auto-scroll on record, focus mode. Time-based, not voice-tracked. Never captured on desktop recordings.
- **REC-13** [docs] Recordings from the Editor recorder automatically create sequences.
- **REC-14** [docs] Studio Sound can be enabled while recording.
- **REC-15** [docs] Recording failure recovery flows exist (recover lost recording, troubleshooting). [index]

### Rooms (remote recording)
- **REC-20** [docs] Create a Room from Drive view (new project) or from inside a project (saves there); each Editor has their own Room.
- **REC-21** [docs] Invite links with pre-assigned role (Guest, Co-host, Producer, Viewer); persistent, do not expire.
- **REC-22** [docs] Green room: guests check devices then "Request to join"; host approves/denies and can change role; optional auto-approval.
- **REC-23** [docs] Roles: Host/Co-host (recorded, full control), Guest (recorded, no controls), Producer (off-stage, not recorded, push-to-talk), Viewer (watch + text chat only). Roles locked during a recording.
- **REC-24** [docs] Control Room (paid): 3 producer/viewer slots on Hobbyist/Creator, 10 on Business/Enterprise; push-to-talk to all or individuals; chat and DMs.
- **REC-25** [docs] Recording: countdown, timer, audio-only mode, 4-hour limit per take, participant notification.
- **REC-26** [docs] Local recording per participant with progressive upload plus cloud backup per participant (WebM VP8/Opus); phone/tablet is cloud-only.
- **REC-27** [docs] Specs: audio MP4/AAC 48 kHz (WebM/Opus in Firefox), optional lossless WAV per participant (Chromium only); video up to 4K, variable frame rate up to 30 fps, H.264/MP4.
- **REC-28** [docs] Screen share: tab/window/entire screen, tab audio; up to two simultaneous shares, both recorded.
- **REC-29** [docs] Room audio settings: echo cancellation, background noise reduction (off by default), auto gain control, recording quality.
- **REC-30** [docs] Room video settings: aspect 1:1 or 16:9, grid/silhouette overlay, recording quality, call quality Auto or Bandwidth saver.
- **REC-31** [docs] General room settings: theme, chat notifications, producer chime, emoji skin tone.
- **REC-32** [docs] Live captions in Rooms; images in chat up to 25 MB; warnings for black camera/screen (10 s) or near-silent mic (30 s); CPU overload warnings.
- **REC-33** [docs] Editor comments during a live session land in the project.
- **REC-34** [docs] Scheduling: title, date/time/timezone, save to new or existing project, who can start recording (Host only / drive members / project access / anyone with link), Google Calendar or Outlook event, Upcoming/Past lists.
- **REC-35** [docs] Async recording requests (Enterprise per changelog): link with title, optional teleprompter script, expiry; guest records one take, reviews, submits or re-records; isolated per guest.
- **REC-36** [docs] After session: auto-creates a sequence in a composition, transcribes, labels speakers with room names, builds scenes with multicam layouts; Backups folder.
- **REC-37** [docs] Up to 10 room participants on all plans; recording session hours per drive are plan-limited (2 h Free, 5/15/25 h per month).
- **REC-38** [index] SquadCast still documented as a separate product (67 articles); specs table lists WAV/MP3 audio and conference cloud backups. Not examined further.

## TRN — Transcription
- **TRN-01** [docs] Automatic transcription when a file is added to the script, on demand from the Project panel, or when imported into a sequence. Not metered on current plans.
- **TRN-02** [docs] 26 Latin-alphabet languages: Brazilian Portuguese, Catalan, Croatian, Czech, Danish, Dutch, English US/UK, Finnish, French, German, Greek (beta), Hindi (beta), Hungarian, Italian, Latvian, Lithuanian, Malay, Norwegian, Polish, Romanian, Slovak, Slovenian, Spanish, Swedish, Turkish, Welsh (beta). No CJK or Cyrillic.
- **TRN-03** [docs] Auto language detection (default) or fixed default language; "Always ask before transcribing"; UK English and Welsh must be chosen manually.
- **TRN-04** [docs] Re-transcribe a file in a different language. One language per file.
- **TRN-05** [docs] Selectable transcription model in settings (Scribe v2 default per changelog).
- **TRN-06** [docs] Limits: files over 15 hours fail; music/lyrics not transcribed.
- **TRN-07** [docs] Inverse text normalisation (numbers/dates) for English, French, Spanish.
- **TRN-08** [docs] Automatic speaker detection (files 10 s to 5 h): detect, then "Identify speakers" modal plays a sample per voice. Manual mode asks for the number of speakers first. Pricing lists "Detect 8+ speakers".
- **TRN-09** [docs] Speaker labels: add with `@` or "Add speaker"; applies to following paragraphs until next change (Shift+Enter = current paragraph only); label a selected range; move by dragging; rename; colour; replace throughout project; delete.
- **TRN-10** [docs] Speaker model: label only, label + voice, label + avatar. Project speakers versus drive speakers (voice clones, shareable).
- **TRN-11** [docs] Correct single word: select, `C`, type; "Correct" or "Correct All" (every instance in project). Alignment re-analysed.
- **TRN-12** [docs] Correct mode (`Opt/Alt+C`): type freely to fix text without changing media.
- **TRN-13** [docs] Hold `Z` + click toggles capitalisation; hold `X` + click cycles punctuation (period, question mark, comma, none).
- **TRN-14** [docs] "Remove from transcript": keeps media, hides the text from script, captions and exports.
- **TRN-15** [docs] Transcription glossary: up to 30 terms per drive, one per line, English only; auto-adds words corrected 3+ times; optional notification.
- **TRN-16** [docs] Import / Replace transcript: paste your own text, "Sync" aligns it; `Speaker Name:` prefixes become labels; English only.
- **TRN-17** [docs] Wordbar: per-word boundaries above the waveform; `Cmd/Ctrl`-drag a boundary to fix alignment; drag a word to change timing.
- **TRN-18** [docs] "Realign words in range" on a selected range.
- **TRN-19** [docs] Multitrack transcription: each sequence track transcribed and merged into one script; include/exclude tracks from script.
- **TRN-20** [docs] Mic bleed fix: automatic prompt "Fix duplicate transcripts", manual run, and revert. Removes text only.
- **TRN-21** [docs] Alignment error indicators: red underlines, "Underline word errors" view toggle, Help > Debug > Force Fix Corrupted Alignment.
- **TRN-22** [docs] Separate stereo file into mono files to fix blank transcripts from phase issues.

## TXT — Script / text-based editing
- **TXT-01** [docs] Delete text = removes the matching media and closes the gap; non-destructive.
- **TXT-02** [docs] Cut/copy/paste text moves or duplicates the media with it, across compositions and projects.
- **TXT-03** [docs] Ignore (`Cmd+Delete` / `Ctrl+Backspace`): strikethrough, skipped in playback; hover to Restore or Delete. Ignored text can be included in transcript export but not subtitles.
- **TXT-04** [docs] Restore removed media: select across an edit boundary, right-click (or `Shift+Cmd+Delete`); batch restores everything in the selection.
- **TXT-05** [docs] Edit boundaries: vertical markers where clips meet; toggle in View > Script; click to "Smooth" or restore.
- **TXT-06** [docs] "Remove ignored text" command deletes all strikethrough content.
- **TXT-07** [docs] Replace selection with gap clip (`Shift+Delete`) keeps timing.
- **TXT-08** [docs] Write mode (`Cmd/Ctrl+E`, or type on a blank line): draft text before audio exists; blue border and "Done writing"; single-letter shortcuts disabled.
- **TXT-09** [docs] Inline notes in parentheses, e.g. `(laughter)`: annotate without affecting alignment; exported in transcripts, not in wordbar.
- **TXT-10** [docs] Word gap editing: right-click a gap > Edit word gap > presets, "Set manually…", or "Shorten all word gaps"; shortcut to reuse last value.
- **TXT-11** [docs] Wordless media (beta, off by default): silences appear as editable tokens; range labels (e.g. "cough") via "Add label for range" feed captions.
- **TXT-12** [docs] Find in script (`Cmd/Ctrl+F`) and find in project (`Shift+Cmd+F`); options: auto-advance, preview results, whole words, match case, wrap around, current composition only.
- **TXT-13** [docs] "Find and…" modes: Find, Remove filler words, Shorten word gaps, Correct transcript, Remove media, Copy; jump cuts as a search type (changelog).
- **TXT-14** [docs] Bulk actions on results: Delete, Delete and replace with gap, Ignore, Remove from transcript, Duplicate to new/existing composition or folder, Copy. Find mentions "replace" as an action; the exact replace UI was not described.
- **TXT-15** [docs] Highlights: multiple colours from the selection toolbar; filter by colour; "Copy highlights" (all or by colour) to paste into another composition; clear highlight.
- **TXT-16** [docs] Bold and italic on script text.
- **TXT-17** [docs] Markers: type `#`, or "Add marker"; shown as section headers in script and purple bookmarks in timeline; drag to move, rename, delete; marker list by the transport; "Copy chapters for YouTube".
- **TXT-18** [docs] Scene boundary: type `/` in script (splits the scene); `/` on an empty line or `//` creates a new empty scene; `Cmd/Ctrl+Shift+Enter` new scene; right-click > Add scene.
- **TXT-19** [docs] Scene thumbnails in the script margin; click to select; drag boundary; delete boundary merges into previous scene; double-click + Delete removes scene content.
- **TXT-20** [docs] Selection toolbar: Ask Underlord, Change layout, Add layer, Add scene, Correct, Regenerate, edit options (ignore/delete/gap/correct), bold/italic, Highlight, Duplicate to…, Comment, more.
- **TXT-21** [docs] Replace script track: copy new media, "Paste special > Replace script track" over a selection; re-aligns other layers to the new timing.
- **TXT-22** [docs] Filler words underlined in script (toggle in View menu).
- **TXT-23** [docs] Playhead and text cursor are independent during playback; current word highlighted.
- **TXT-24** [docs] Script text size increase/decrease.
- **TXT-25** [docs] Add layer from a script selection (file, stock, text, captions, shape, recording); adding text from a selection creates a scene.
- **TXT-26** [docs] Only media on the script track is transcribed; layers are not.

## SEQ — Sequences, multitrack, multicam
- **SEQ-01** [docs] Sequence = bundle of synced tracks edited as one item from the script; edits apply to all tracks.
- **SEQ-02** [docs] Create from multi-selected files ("Create sequence"), from an empty composition ("Multicam & multitrack"), or empty via the `+` menu. Must be inserted into the script to edit.
- **SEQ-03** [docs] Sequence Editor (`Shift+Cmd/Ctrl+O`, right-click > Edit sequence; `Esc`/Done to close). Changes apply everywhere the sequence is used.
- **SEQ-04** [docs] Up to 14 tracks per sequence; add media as new track.
- **SEQ-05** [docs] Multi-Track Autosync aligns two or more audio-bearing tracks automatically.
- **SEQ-06** [docs] Nudge clips: arrows 1 frame, `Cmd/Ctrl` ½ frame, `Shift` 5 frames, `Opt/Alt` 1 second.
- **SEQ-07** [docs] Per-track mute/solo, volume, pan, volume automation, effects, auto-level; Blade/Slip/Range tools inside the sequence.
- **SEQ-08** [docs] Include in script / Remove from script per track.
- **SEQ-09** [docs] Automatic Multicam: style ("Show only active speaker" plus multi-speaker styles), cutaway frequency None / Occasional (30 s) / Frequent (10 s), camera setup mapping audio tracks to cameras, optional layout pack. Overwrites existing scenes; track order sets panel positions.
- **SEQ-10** [docs] Manual multicam: per scene, "Change source" shows/hides stacked camera layers.
- **SEQ-11** [docs] Export soloed track as mono file.
- **SEQ-12** [docs] Limitations: TTS and Regenerate do not work over sequences; lip sync and jump-cut video smoothing unsupported on multitrack.

## TML — Timeline
- **TML-01** [docs] Collapsed by default; drag handle or shortcut to expand. Contains transport, split button, time ruler, layer lanes, wordbar, script track.
- **TML-02** [docs] Timeline view and Storyboard view (grid of scene tiles; replaced the old Scene Rail).
- **TML-03** [docs] Zoom in/out, 100%, Fit in view, Fit current scene, zoom to playhead, fit selection; layer size compact/default/large.
- **TML-04** [docs] Tools: Select (`A`), Range (`R`), Blade (`B`), Slip, Hand (`H`); hold key for temporary tool. "Advanced timeline tools" toggle in View menu.
- **TML-05** [docs] Select tool: move, trim, combine clips; drag media between script and layer lanes; Shift-click multi-select adjacent script clips.
- **TML-06** [docs] Range tool: select a span on the script track to delete, replace with gap, drop visuals into (auto-creates scene boundaries), realign, or label.
- **TML-07** [docs] Blade: split at a point; draggable edit point expands/collapses non-destructively.
- **TML-08** [docs] Slip: change which portion of source plays within a fixed-length clip.
- **TML-09** [docs] Trim by dragging clip/layer edges.
- **TML-10** [docs] Split button: no layer selected = split scene at playhead; layer selected = split that layer; Shift splits a media layer into clips. Splitting non-media layers duplicates the layer.
- **TML-11** [docs] Gap clips: silent placeholders; created by extending word gaps, wordbar drag, or replacing a range. Black frame in video.
- **TML-12** [docs] Freeze frame: right-click video > Add freeze frame; adjust duration by trim or Properties. Extending a word gap on video inserts one automatically.
- **TML-13** [docs] Automatic microfades (~5 samples) at every cut; invisible; not included in timeline exports.
- **TML-14** [docs] Clip transitions/fades: drag the white handle at a clip edge for a fade, or click for a menu with duration, loops, fade direction.
- **TML-15** [docs] Crossfade requires both clips on the same layer with source handles beyond the cut.
- **TML-16** [docs] Clip speed control in Properties (affects export; duration updates). Speed is not available on AI speech until converted.
- **TML-17** [docs] Duration modes for non-script layers: Once, Loop, Freeze (hold last frame). Stock music can loop across the composition.
- **TML-18** [docs] Attach/snap a layer to a scene boundary (red/dashed line); extend a layer across scenes by dragging its edge.
- **TML-19** [docs] Layer order via right-click > Layer order; mute audio layer / hide visual layer with `0`.
- **TML-20** [docs] Markers, comments and audio pins shown on the ruler (toggle).
- **TML-21** [docs] VU meter (-30 to 0 dB, yellow/red near clipping).
- **TML-22** [docs] Playhead autoscroll toggle (`Esc`).
- **TML-23** [docs] Transport: play/pause, prev/next scene, playback speed up to 3x (preview only), markers list.
- **TML-24** [docs] Frame stepping with arrow keys when the timeline is focused.
- **TML-25** [docs] Reorder scenes by dragging thumbnails.
- **TML-26** [docs] Copy/paste of layers keeps offset from scene start.
- **TML-27** [docs] "New timeline" is a Labs toggle. Which timeline your users see may vary.
- **TML-28** [recall] Explicit ripple versus non-ripple mode toggle: not documented; behaviour is implied by delete versus replace-with-gap.

## CNV — Canvas / visual
- **CNV-01** [docs] Scene Editor: click to select (purple border = script track visual, blue = other), drag to move/resize, floating layer toolbar (Crop, Transform, Replace, Effects/Animation), scene toolbar (Layout, Background, Layers).
- **CNV-02** [docs] Layer types: video, audio, image, shape, waveform, progress bar, text, emoji, captions, avatar, placeholder.
- **CNV-03** [docs] Layer properties: duration, audio (volume/pan/mute/solo), opacity, size and position (aspect lock), rotation, corner rounding, border, animation, effects.
- **CNV-04** [docs] "Current scene / All scenes" toggle scopes an edit on a multi-scene layer.
- **CNV-05** [docs] Crop mode: handles, drag content inside frame, zoom; double-click to enter; reset crop.
- **CNV-06** [docs] Transform: position, rotate, flip horizontal/vertical.
- **CNV-07** [docs] Layer order: bring to front `]`, forward `Shift+]`, backward `Shift+[`, send to back `[`; drag in the layer list.
- **CNV-08** [docs] Hide/show (`Opt/Alt+H`), lock (`Opt/Alt+L`), rename, delete, duplicate (`Cmd/Ctrl+D` or Alt-drag).
- **CNV-09** [docs] Multi-select (Shift-click or marquee, same scene only): move/resize/delete/copy together; "Mixed" values; align left/right/center/top/middle/bottom; distribute (3+ layers).
- **CNV-10** [docs] Snap-to guides toggle.
- **CNV-11** [docs] Replace media (keeps edits) from toolbar, right-click, or Properties.
- **CNV-12** [docs] Text layers: Text, Subtitle, Title presets; font, size, alignment, colour, border, background (bounding box, radius, colour, "Hug lines" or "Box"), fixed or auto sizing; bullet points; emoji.
- **CNV-13** [docs] Dynamic text types: Freeform, Captions, Speaker (active speaker name), Composition name, Marker (current chapter), Timer (count up/down from start, end or last marker).
- **CNV-14** [docs] Shapes: rectangle, ellipse, line, arrow.
- **CNV-15** [docs] Fill and stroke: solid colours, eyedropper, gradients (Vertical, Horizontal, Center, Corner; up to 64 stops; 21 presets). No gradients on waveforms, strokes or text borders.
- **CNV-16** [docs] Scene background colour or gradient; background image via a layer.
- **CNV-17** [docs] Custom fonts: Google Fonts search or upload `.ttf`/`.otf`/`.woff` (Creator+), saved per drive; cannot be removed.
- **CNV-18** [docs] Captions layer: pick a preset from the Captions panel (search, filter, sort; 18 new presets in Sept 2026; Legacy group); built from the script and auto-synced.
- **CNV-19** [docs] Caption styling: font, size, alignment, text and background colour (gradients), stroke; "Active word" fill/stroke/background (karaoke-style); "Future words" styling; one-word-at-a-time presets.
- **CNV-20** [docs] Per-speaker caption layers (assign a speaker to each layer).
- **CNV-21** [docs] Convert any text layer to captions via Type dropdown.
- **CNV-22** [docs] Waveform layer (audiogram): styles Lines, Circle, Ring, Wave; speaker assignment; fill, size, fidelity, magnitude, mirror.
- **CNV-23** [docs] Progress bar: styles Bar, Ring, Pie; colour/opacity; start and end percentage.
- **CNV-24** [docs] Layer animations (in/out presets): Blur, Fade, Scale, Slide, Spin, Wipe, Zoom and pan, Ken Burns.
- **CNV-25** [docs] Ken Burns options: In / Out / None, eight directions, centre zoom, speed Slow/Moderate/Fast. Not on the script layer.
- **CNV-26** [docs] Custom keyframe animation: two keyframes (start/end) for position, size, rotation, crop, opacity; drag diamonds in timeline; easing curves with preview. Script-track video offers "Zoom and pan" and Custom.
- **CNV-27** [docs] Text animations: Appear, Reveal, Pop, Bounce, Pulse, Glitch, Flicker, Wave, Wiggle (plus Block reveal, Feathered slide, Glow slide, Stretch where enabled); animate by character/word/line/paragraph; duration and easing.
- **CNV-28** [docs] Animate text (kinetic typography): select up to 50 words, choose preset, Generate; AI picks emphasis words and builds grouped layers; Base and Emphasis styles; not synced to later script edits.
- **CNV-29** [docs] Scene transitions (16): None, Blur, Clock Wipe, Color Dip, Crossfade, Fade, Cross-zoom, Double Wipe, Flip, Move Out, Push-Pull, Scale, Shake, Shape Wipe, Split, Wipe; duration, direction, feather, easing; "Apply to all".
- **CNV-30** [docs] Smart transitions: auto-applied between new scenes; duration, easing, Add Fades toggle; global on/off.
- **CNV-31** [docs] Transitions fail across ignored text; very short scenes may not show them.
- **CNV-32** [docs] Visual effects: Ascii, Blur, Chroma key, Color tone, Film grain, Glow, Invert, Pixelate, Portrait Lighting, Selective color, Shadows, Threshold, Uplighting, VHS, Vignette, Zoom blur, Glass blur, Skin smoothing, Blur speaker background (beta), Green Screen, Eye Contact.
- **CNV-33** [docs] Effects drawer with hover preview; effects at layer or scene level; reorder by dragging; toggle or remove.
- **CNV-34** [docs] Colour adjustments: filter presets (Neutral, Warm, Cool, Black and white, Pop, Invert); white balance eyedropper; sliders Exposure, Contrast, Highlights, Shadows, Temperature, Tint, Saturation; double-click resets.
- **CNV-35** [docs] Copy/paste attributes (`Shift+Cmd/Ctrl+V`): gain + speed; size/position/fill/border/stroke/opacity; text styles; zoom level + colour adjustments.
- **CNV-36** [docs] Scene properties: layout, duration, background, effects, transitions, layer list.
- **CNV-37** [docs] Editor layout: Full-screen video, Stacked, Side-by-side; collapsible panels; floating movable panels; resizable sidebar.
- **CNV-38** [docs] Avatar layers behave as Talking Head and can be resized/cropped and combined with Green Screen.

## AUD — Audio
- **AUD-01** [docs] Studio Sound: AI speech enhancement (noise, echo); applied per source file, affecting all uses; Intensity slider; needs internet; not on TTS until converted; credit ceiling 30 per application.
- **AUD-02** [docs] Volume slider and mute per script selection, clip, layer, sequence track, or whole composition; optional dB display.
- **AUD-03** [docs] Pan left/right in advanced audio settings.
- **AUD-04** [docs] Volume keyframes: right-click > Add volume keyframe or `Opt/Alt`-click; drag for level and position.
- **AUD-05** [docs] Auto-leveling to about -16 dB: global default, per project, per clip; fails around large spikes.
- **AUD-06** [docs] Normalize audio as a file action.
- **AUD-07** [docs] "Lower audio of other layers" (ducking): based on clip presence, not signal; adjustable reduction percentage; layer-level only; not in Sequence Editor.
- **AUD-08** [docs] Room tone: AI noise profile from the first 30 s of each file fills gap clips; per-clip add/delete/mute/volume; disabled when Studio Sound is on.
- **AUD-09** [docs] Audio effects: Compressor, Limiter, Equalizer (5-band parametric), Graphic EQ (10 bands, 31.25 Hz to 16 kHz), High shelf EQ, Low shelf EQ, High-pass filter, Low-pass filter, Reverb, Bitcrusher, Distortion, Flanger. Most have presets.
- **AUD-10** [docs] Compressor: threshold, ratio, attack, release, knee, input and reduction meters. Limiter: ceiling, threshold, release, meters.
- **AUD-11** [docs] Reverb: mix, width, damping, room size, pre-delay, low cut.
- **AUD-12** [docs] Effect chain order is top-to-bottom and draggable; audio effects apply to layers or sequence tracks, not scenes.
- **AUD-13** [docs] Composition-level audio properties (click composition title).
- **AUD-14** [docs] Convert to mono / separate into mono files.
- **AUD-15** [docs] Stock music and sound effects library; AI-generated music (duration, tempo, optional lyrics) and sound effects (duration, loop).
- **AUD-16** [docs] Export loudness normalisation: Off, Peak, -14/-16/-18/-23/-24 LUFS.
- **AUD-17** [docs] Not documented: de-esser, noise gate, multitrack mixer view. Do not assume.

## AI — AI features
### Underlord (chat co-editor)
- **AI-01** [docs] Chat panel in the sidebar; plain-language requests that edit the project; also "Ask Underlord" from the selection toolbar.
- **AI-02** [docs] Documented action groups: Captions; Clips (find/split/reformat vertical); Animations and Transitions (pans, zooms, zoom cuts, fades, animated callouts); Translate; Sound effects and music / audio effects; Slides to video (PDF + TTS + avatar); Web search.
- **AI-03** [docs] Also runs: center active speaker, kinetic typography (up to 5,000 words), media generation, YouTube thumbnails, create clips, translate/dub/lip sync, finding files and projects.
- **AI-04** [docs] Context attach via `@`, `+`, or right-click "Add to Underlord": scenes, script selections, layers, project files, speakers, timestamps/ranges.
- **AI-05** [docs] Model picker (Auto plus many third-party models; reasoning variants); premium models on paid plans.
- **AI-06** [docs] Skills: reusable Markdown instructions invoked with `/`; create manually or with Underlord; private to the user.
- **AI-07** [docs] Revert per response (session checkpoints), plus undo and version history.
- **AI-08** [docs] Persistent chat history per user and project; automatic edit review; tables in chat; media previews before applying; up to 5 concurrent generations on paid plans (changelog).
- **AI-09** [docs] Credits: every message costs "agent brain" credits plus any tool cost; non-deterministic.
- **AI-10** [docs] Runs outside the editor via API, MCP (Claude, ChatGPT, Cursor), Zapier.

### AI Tools panel
- **AI-20** [docs] Five groups: Sound good, Look good, Repurpose, Publish, Write; disabled tools show a tooltip.
- **AI-21** [docs] Remove filler words: review list with timestamps and audition; per item Delete / Delete and replace with gap / Ignore / Remove from transcript; "Avoid harsh cuts". English, plus Spanish/German/French/Portuguese/Italian per Aug 2026 changelog.
- **AI-22** [docs] Remove retakes: detects repeated lines/false starts and marks earlier takes as ignored text.
- **AI-23** [docs] Shorten word gaps: threshold "more than" / "between" durations, target length (e.g. 200 ms), Shorten / Shorten all.
- **AI-24** [docs] Edit for Clarity: intensity Low/Medium/Heavy; "Reword for flow" (AI speech); results as ignored text and blue AI text; copy edits to new composition / remove ignored / discard all; optional smooth jump cuts.
- **AI-25** [docs] Add Chapters: generates markers and titles.
- **AI-26** [docs] Eye Contact: single face, constant frame rate, needs internet.
- **AI-27** [docs] Green Screen (AI background removal) and Chroma key for real green screens.
- **AI-28** [docs] Center active speaker (beta): choose 16:9, 1:1 or 9:16; creates scenes and fixed crops per speaker.
- **AI-29** [docs] Quick Design: scenes + layouts + optional B-roll (stock or AI) for single-speaker, single-scene compositions.
- **AI-30** [docs] Create clips: 1–20 clips, 10 s to 5 min, optional topic and layout; output as new compositions in a "Clips from…" folder.
- **AI-31** [docs] Create highlight reel and Find highlights.
- **AI-32** [docs] Publish tools: Draft a title, Summarize, Show notes, YouTube description, Draft a social post (choose platform), Draft a blog post; work on selection or whole composition.
- **AI-33** [docs] Write tools: Brainstorm, Write a script, Write an outline, Rewrite.
- **AI-34** [docs] AI Video Maker: prompt, script or footage → script, voiceover, visuals, optional avatar; aspect ratio; five B-roll styles or stock.
- **AI-35** [docs] Portrait Lighting, Uplighting, Skin smoothing (strength + regions), Blur speaker background.

### Regenerate
- **AI-40** [docs] Smooth an edit: click an edit boundary > Smooth, or select text > Regenerate (`D`); choose Audio & video or Audio only; Play / Retry / Keep / Revert. English only.
- **AI-41** [docs] Change what was said: retype up to 250 characters; requires an authorised custom voice clone; video retimed best-effort.
- **AI-42** [docs] Smooth jump cuts (bulk): "Find first" or "Smooth all" (batches of 5, cancellable queue); video consent prompt on first use.
- **AI-43** [docs] Not usable across multiple edit points, scene boundaries, ignored text, or already regenerated clips.

### AI speech and voices
- **AI-50** [docs] Text-to-speech: write in Write mode, assign a speaker with a voice, "Done writing" generates audio; multi-speaker scripts.
- **AI-51** [docs] TTS languages (19): English, Croatian, Czech, Danish, Dutch, Finnish, French, German, Hungarian, Italian, Malay, Polish, Portuguese (BR and PT), Romanian, Slovak, Spanish, Swedish, Turkish.
- **AI-52** [docs] Voice model choice: Multilingual v2, ElevenLabs v3, v4.
- **AI-53** [docs] Tone tags (v3/v4): parenthesised directions via "Tone" button; presets or custom up to 150 characters; not in captions or share-page transcript.
- **AI-54** [docs] Stock voices: recommended library plus ElevenLabs community voices; 25+ on Hobbyist/Creator, 60+ on Business.
- **AI-55** [docs] Voice clone: record or upload a fixed English consent statement; refused for deceased or non-consenting people or AI audio; cannot add training data later; shareable with drive members.
- **AI-56** [docs] Convert recorded audio to AI speech (voice replacement); video is not retimed.
- **AI-57** [docs] Convert AI speech to a standard audio layer (permanent) to enable trims, fades, speed, word boundaries.
- **AI-58** [docs] Text colour states for TTS (unassigned, generating, generated, failed, regenerating).
- **AI-59** [docs] Pronunciation fixes via phonetic spelling then transcript correction.

### Avatars and generative media
- **AI-60** [docs] Avatars: assign to a speaker; stock gallery, text prompt (three candidates), or uploaded image (JPEG/PNG/WEBP up to 10 MB); "Generate avatar" lip-syncs to the speech; attitude presets Friendly/Professional/Calm/Excited or custom; model picker.
- **AI-61** [docs] Generate image/video pane: prompt, attach reference, style, aspect ratio, number/duration, resolution, audio generation on some models; separate history per type.
- **AI-62** [docs] Style library (20 named styles, e.g. 3D Animation, Anime, Cinematic, Watercolor).
- **AI-63** [docs] Edit image with AI: draw with pencil (add/change, optional reference) or eraser (remove).
- **AI-64** [docs] Expand with AI (outpaint image/video to a new frame); Extend video (continue from last frame); First/last-frame morph video.
- **AI-65** [docs] YouTube thumbnail generator: four options; set as share-page thumbnail, download, add as layer, reuse prompt.
- **AI-66** [docs] Generated assets get actions: Edit image, Turn into video, Reuse prompt.

### Translation
- **AI-70** [docs] Translate captions into about 67 languages; each translation is its own composition; language dropdown; re-translate overwrites manual edits.
- **AI-71** [docs] Dub speech into about 28 languages with stock voices per speaker; "Recommended" dubbing voices on Business+; Change Speakers; regenerate sections.
- **AI-72** [docs] Lip sync for dubbed video (Creator+): single speaker only, not with avatars or sequences.
- **AI-73** [docs] Translation style: Auto, Match timing, Direct translation.
- **AI-74** [docs] Translation proofread (Business+) and "Do not translate" list (Business+).
- **AI-75** [docs] Batch export of multiple translations; subtitles/transcripts per language.

## COL — Collaboration
- **COL-01** [docs] Comments: select words > comment (`Shift+Cmd/Ctrl+M`); edit, `@`mention (email notification), resolve/unresolve, sidebar sort, "Copy all comments". Comments do not survive copy/paste or duplication.
- **COL-02** [docs] Drive roles: Editor (paid seat), Viewer (free; view + comment); Owner; Admin (Enterprise).
- **COL-03** [docs] Project collaborators without drive membership: Project editor (edit + export; no AI, recording or upload) and Project commenter.
- **COL-04** [docs] Project link access: Project access required / Anyone with link can view / can comment; per-project access level for all drive members.
- **COL-05** [docs] Live collaboration with presence in version history; cursor presence (Labs; Business/Enterprise).
- **COL-06** [docs] Share-page comments are separate from project comments.
- **COL-07** [docs] Brand Studio (Business+): fonts, colours, gradients, logo, layout packs, up to 500 media assets, glossary; asset permissions (Enterprise).
- **COL-08** [docs] Enterprise: SSO, SCIM, audit logs, custom workspaces, hide General workspace, member CSV export, AI/data controls.

## EXP — Export and publish
- **EXP-01** [docs] Export button (top right) with tabs Video, Audio, Transcript, Subtitles, Timeline, and a Destination dropdown.
- **EXP-02** [docs] Local video: MP4 or GIF; resolution 480p/720p/1080p/4K (plan-capped); quality Low/Medium/High only (no bitrate control); no upscaling benefit.
- **EXP-03** [docs] Export scope: current composition, current selection, current scene, each scene, each marker, each line-break section, all compositions (batch; Creator+ per pricing).
- **EXP-04** [docs] Audio settings on video export: channels mono/stereo, 44.1/48 kHz, bitrate 32–256 kbps, normalise.
- **EXP-05** [docs] Audio export: M4A, WAV, MP3; bitrate options for M4A/MP3 only.
- **EXP-06** [docs] Metadata: show title, episode title, description, artwork, markers as chapters (MP4, MP3, AAC; not WAV).
- **EXP-07** [docs] Subtitle tracks are embedded (not burned in) in local video exports; optional per Sept 2026 changelog. Burned-in text requires a captions layer.
- **EXP-08** [docs] Subtitles export: SRT or VTT; show speakers; max characters per line; max lines per card.
- **EXP-09** [docs] Transcript export: HTML, MD, DOCX, TXT, RTF; copy to clipboard (not DOCX); include composition name, markers, ignored text, speaker labels; timecode offset, interval, at paragraph breaks, with speakers, for markers. Excludes wordless media and scene boundaries.
- **EXP-10** [docs] Timeline export (Creator+): Premiere XML, Premiere .prproj (beta, multicam), DaVinci Resolve XML, Final Cut Pro FCPXML, Reaper EDL, Audition SESX, Pro Tools/Logic AAF.
- **EXP-11** [docs] Timeline options: include media files, comments as markers, track per file, Logic compatibility, frame-rate snapping.
- **EXP-12** [docs] What survives XML: clips, trims, markers, ignored script, Studio Sound files; captions to FCP and Resolve. Not: effects, keyframes, speed, images, titles, transitions, scenes, sequences, microfades.
- **EXP-13** [docs] Publish to web link (share.descript.com): Update keeps the URL; resolution up to 4K incl. 1440p; show transcript, allow comments, show author, allow sharing from embeds, optimise file size, custom branding.
- **EXP-14** [docs] Share page: access levels, copy link with start time, embed code, download, page view count, comments, Edit button, Unpublish; markers appear as chapters.
- **EXP-15** [docs] Thumbnails: choose a frame or upload an image.
- **EXP-16** [docs] YouTube publish (web app only): privacy, title, description, category, tags; markers become chapters; no Shorts or brand channels.
- **EXP-17** [docs] Other destinations: Google Drive, Blubrry, Buzzsprout, Captivate, Castos, Convia, eWebinar, Hello Audio, Headliner, Pinecast, Podbean, Podcast.co, Restream, Transistor, VideoAsk, Wistia; HubSpot and LinkedIn publishing [index/changelog].
- **EXP-18** [docs] API/MCP/Zapier: import, Underlord edits, publish, transcript export; no rendered-file download without publishing.

## PRF — Preferences, shortcuts, accessibility
- **PRF-01** [docs] Settings (`Cmd/Ctrl+,`) groups: General, Account, Drive.
- **PRF-02** [docs] General: default project type, default AI speaker, appearance Light/Dark/System (plus "Classic" colour theme per changelog).
- **PRF-03** [docs] Transcription: default language, always ask, auto-detect speakers, glossary auto-add and notifications.
- **PRF-04** [docs] Playback: force software video decoding; limit preview to 1080p.
- **PRF-05** [docs] Editing: automatic volume levels, room tone on gap clips, apply smart transitions, default translation style.
- **PRF-06** [docs] Advanced: volume in decibels, show wordless media, always show usage info; Labs toggles.
- **PRF-07** [docs] Account: default AI models, connected apps (LinkedIn, Zoom), MCP, API tokens, Skills tab.
- **PRF-08** [docs] Drive: logo, name, default publish privacy, members, workspaces, plan, usage (12-month CSV export), invoices.
- **PRF-09** [docs] Action bar (`Cmd/Ctrl+K`): command palette; "Jump to time" accepts several formats; jump to scene/marker/composition.
- **PRF-10** [docs] Accessibility-relevant features only: light/dark themes with contrast improvements, script text size, shortcut reference, live captions in Rooms, wordless-media labels for captions. No screen-reader or keyboard-remapping documentation found (only the Screen Recorder shortcut is stated as customisable).
- **PRF-11** [docs] Desktop app for macOS 14+ and Windows 11; web app on Chromium browsers and Firefox (no recording in Firefox); mobile is view-only.

### Default shortcuts (Mac / Windows) [docs]
| Action | Mac | Windows |
|---|---|---|
| Play/pause | Space, or Cmd+S | Space, or Ctrl+S |
| Play from cursor | Shift+Space | Shift+Space |
| Spot audition | Shift+Cmd+Space | Ctrl+Shift+Space |
| Playback speed down / reset / up | Shift+J / K / L | same |
| Step one frame | Left / Right | same |
| Next / previous marker | Opt+Cmd+. / , | Ctrl+Alt+. / , |
| Next / previous scene | Shift+Cmd+. / , | Ctrl+Shift+. / , |
| Next / previous clip | Opt+Shift+Cmd+. / , | Ctrl+Alt+Shift+. / , |
| Tools: Select / Range / Blade / Hand / Slip | A / R / B / H / Y | same |
| Split clip | S | S |
| Replace with gap clip | Shift+Delete | Shift+Backspace |
| Mute/hide selected layer | 0 | 0 |
| Timeline zoom in / out | Cmd+= / Cmd+- | Ctrl+= / Ctrl+- |
| Toggle timeline | Shift+Cmd+S | Ctrl+Shift+S |
| Ignore | Cmd+Delete | Ctrl+Backspace |
| Restore removed media | Shift+Cmd+Delete | Ctrl+Shift+Backspace |
| Correct selection / Correct mode | C / Opt+C | C / Alt+C |
| Capitalisation / punctuation | hold Z or X + click | same |
| Write mode | Cmd+E | Ctrl+E |
| Speaker label / Marker / Scene | @ / # / / | same |
| Regenerate | D | D |
| Insert text / shape | Opt+Cmd+T / Opt+Cmd+S | Ctrl+Alt+T / Ctrl+Alt+S |
| Highlight | Shift+Cmd+H | Ctrl+Shift+H |
| Comment | Shift+Cmd+M | Ctrl+Shift+M |
| Duplicate to / quick duplicate | Shift+Cmd+C / Opt+Shift+Cmd+C | Ctrl+Shift+C / Ctrl+Alt+Shift+C |
| Paste attributes | Shift+Cmd+V | Ctrl+Shift+V |
| Edit sequence | Shift+Cmd+O | Ctrl+Shift+O |
| Auto-level clip | Opt+Shift+Cmd+L | Ctrl+Alt+Shift+L |
| Layer order | ] , Shift+] , Shift+[ , [ | same |
| Duplicate / hide / lock layer | Cmd+D / Opt+H / Opt+L | Ctrl+D / Alt+H / Alt+L |
| Snap to guides | Shift+Cmd+' | Ctrl+Shift+' |
| Full-screen scene editor | Cmd+. | Ctrl+. |
| Find / find in project / next / previous | Cmd+F / Shift+Cmd+F / Cmd+G / Shift+Cmd+G | Ctrl equivalents |
| Action bar | Cmd+K | Ctrl+K |
| Undo / redo | Cmd+Z / Shift+Cmd+Z | Ctrl+Z / Ctrl+Shift+Z |
| New project / composition | Cmd+N / Opt+Cmd+N | Ctrl+N / Ctrl+Alt+N |
| Project access / copy link | Shift+Cmd+A / Cmd+L | Ctrl+Shift+A / Ctrl+L |
| Publish / local export | Shift+Cmd+P / Shift+Cmd+E | Ctrl+Shift+P / Ctrl+Shift+E |
| Panels: project / media / layouts / AI tools | Opt+Cmd+C / L / P / Y | Ctrl+Alt+C / L / P / Y |
| VU meter | Cmd+Opt+U | Ctrl+Alt+U |
| Shortcut list | Opt+/ | Ctrl+/ |
| Screen Recorder | Cmd+Shift+2 | Ctrl+Shift+2 |
| Settings | Cmd+, | Ctrl+, |

Single-letter shortcuts only work in the script editor outside Write and Correct modes.

## PLN — Plan limits that change behaviour [docs, summarised from pricing]
| Limit | Free | Hobbyist | Creator | Business |
|---|---|---|---|---|
| Price per editor/month (annual / monthly, unlabelled on page) | $0 | $16 / $24 | $24 / $35 | $50 / $65 |
| Media minutes per month | 60 | 600 | 1,800 | 2,400 |
| AI credits | 100 one-time | 400/mo | 800/mo | 1,500/mo |
| Seats | 1 | 1 | up to 3 | up to 5 |
| Export resolution (local and web link) | 720p | 1080p | 4K | 4K |
| Watermark | Yes | No | No | No |
| Web link / YouTube duration | 1 h | 1 h | 3 h | 3 h |
| Max upload size | 1 GB | 10 GB | 20 GB | 50 GB |
| Max source resolution / bitrate | 4K / 150 Mbps | 4K / 150 Mbps | 5K / 1 Gbps | 5K / 1 Gbps |
| Cloud storage | 5 GB | 100 GB | 1 TB | 2 TB |
| Rooms recording hours | 2 h | 5 h/mo | 15 h/mo | 25 h/mo |
| Timeline + batch export, ProRes, custom fonts, top-ups | No | No | Yes | Yes |
| Custom/remixed layout packs | No | Yes | Yes | Yes |
| Translation proofread, do-not-translate, Brand Studio, default team pack | No | No | No | Yes |

- **PLN-01** [docs] Web-link exports over the plan's duration cap are converted to audio-only.
- **PLN-02** [docs] Unused minutes and credits do not roll over. Still images count as 1 second of media. Rooms sessions are charged for session length, not per participant.
- **PLN-03** [docs] AI tools on Free are "Limited" (exact limits not stated on the page as summarised).
- **PLN-04** [docs] Project collaborators and Viewers cannot use anything that consumes minutes or credits.
- **PLN-05** [docs] API requests return 402 when minutes or credits run out.

## Sources
- https://help.descript.com/llms.txt and https://help.descript.com/llms-full.txt (index and full text of the help center)
- https://help.descript.com/descript-tour/keyboard-shortcuts
- https://help.descript.com/add-and-manage-media/supported-file-types
- https://help.descript.com/account-and-app-settings/app-settings
- https://help.descript.com/working-in-projects/project-settings
- https://help.descript.com/script-editing/ (all articles in this section: automatic-transcription, supported-transcription-languages, correct-your-transcript, delete-vs-ignore, edit-boundaries, filler-words, word-gaps, shorten-word-gaps, edit-for-clarity, highlights, markers-chapters, search-in-script-tool, wordless-media, import-transcript, transcription-glossary, the-wordbar, realign-range, regenerate-overview, fix-take, change-meaning, smooth-jump-cuts, speakers, speaker-labels, speaker-detection, multi-track-sequences-overview, create-a-sequence, sequence-editor, detach-audio, replace-script-track, restore-removed-media, fix-duplicate-transcripts-or-mic-bleed)
- https://help.descript.com/visuals/ (all articles, incl. captions, text-layers, shapes, waveforms, progress-bars, automatic-multicam, center-active-speaker, boundaries, scenes-layers, multi-select, color-stroke, custom-fonts, visual-roles, placeholder-layers)
- https://help.descript.com/timeline/ (timeline-overview, timeline-tools, split-button, automatic-microfades, freeze-frames, using-gap-clips)
- https://help.descript.com/effects-animations-transitions/ (all articles, incl. overview, studio-sound, eq, lower-other-audio, auto-leveling, room-tone, volume-automation-and-keyframes, transitions, smart-transitions, animate-visual-layers, text-animations, kinetic-typography, color-adjustments, green-screen, eye-contact)
- https://help.descript.com/export-and-share/ (export-and-publishing-overview, video-gif, audio, subtitles, transcript, timeline-exports, share-page, share-page-settings, youtube, others, google-drive, sequence-tracks, set-and-customize-thumbnails, remove-embedded-subtitles, how-descript-controls-export-quality-and-file-size)
- https://help.descript.com/record/ (editor-recorder, screen-recorder, record-with-descript-s-built-in-teleprompter, rooms-overview, host-guide, record-in-a-descript-room, roles, room-settings, control-room, quality, schedule, async-recordings, edit, presentation)
- https://help.descript.com/ai-speech/ (tts, write-mode, stock-voices, custom-speaker, convert-to-ai-speech, convert-to-audio, tts-colors, pronunciation, sharing)
- https://help.descript.com/generative-media/ (generate-media, generate-audio, edit-with-ai, expand-with-ai, extend-video, last-frame, yt-thumbnail, style-library, models, overview, create-your-avatar)
- https://help.descript.com/repurpose/ (create-clips-from-your-content, translate-overview, translate-captions, dubbing, lip-sync, do-not-translate)
- https://help.descript.com/ai-assistant/ (ai-video-maker, quick-design, prompting, skills, revert)
- https://help.descript.com/getting-started/ (underlord-beta-your-ai-co-editor-in-descript, edit-like-a-doc, scenes, import-overview, add-a-file-to-the-script, create-a-new-project, descript-for-web, descript-system-requirements)
- https://help.descript.com/descript-tour/ (the-editor-interface, playback-and-navigation, customize-the-editor, project-panel, properties-panel, media-panel, view-menu, action-bar, selection-toolbar, copy-paste-attributes, ai-tools-overview, sound-good-tools, look-good-tools, repurpose-tools, publish-tools, write-tools, drive-view, multi-window-editing)
- https://help.descript.com/templates-and-presets/ (overview, layouts-save, layouts-remix, layouts-generate, layouts-apply, layouts-behavior)
- https://help.descript.com/add-and-manage-media/ (media-library-overview, project-file-actions, replace-file, repair-audio-drift, av-sync, flatten-tracks, slides, search-your-drive, local, editInDescript)
- https://help.descript.com/working-in-projects/ (overview, version-history, backups, aspect-ratio, audio, duplicate, access, duration-word-count, project-file-management, save)
- https://help.descript.com/collaboration/ (project-comments, project-links, cursor-presence)
- https://help.descript.com/team-management/drive-roles
- https://help.descript.com/branding/brand-studio
- https://help.descript.com/account-and-app-settings/labs
- https://help.descript.com/billing-payments-plans/track-and-understand-your-media-minutes-and-ai-credits
- https://help.descript.com/api-and-mcp/mcp and https://help.descript.com/api-and-mcp/api
- https://www.descript.com/pricing (via summarising fetch)
- https://feedback.descript.com/changelog (via summarising fetch; Feb–Nov 2026 entries)