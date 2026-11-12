# Glint

Offline instant-replay clipper for Windows. Glint keeps the last few minutes of your screen in a replay buffer (in RAM, in temporary files, or both) and saves a clip only when you press a key. No account, no upload. Your clips never leave your PC; the only thing Glint does online is ask GitHub once a day whether a newer version exists (it can be switched off).

## Install

Download **GlintSetup.exe** from the [latest release](https://github.com/swamie/glint/releases/latest) and run it. It installs Glint for your Windows account (no administrator rights needed) and fetches what Glint needs, all from GitHub releases:

| What | From | Notes |
| --- | --- | --- |
| Glint | github.com/swamie/glint | to `%LOCALAPPDATA%\Programs\Glint` |
| ffmpeg, ffprobe | github.com/GyanD/codexffmpeg | skipped if a suitable ffmpeg is already on PATH |
| Blur v2.23 | github.com/f0e/blur | optional, asks first (about 125 MB) |
| WebView2 runtime | microsoft.com | only if missing; Windows 11 has it built in |

It adds a Start menu shortcut and an entry under Installed apps (uninstalling keeps your clips). The installer is not code-signed, so Windows SmartScreen may show "Windows protected your PC": choose More info → Run anyway.

**Updates.** The small up arrow at the top of Glint's window checks GitHub for a newer release about once a day; click it to check right away. When there is one, the arrow lights up and a click downloads it, swaps it in and restarts Glint. The download's SHA-256 is checked against the release's checksum file. Switch the daily check off under Settings → Updates.

- **Stack:** Rust + Tauri 2 (WebView2 UI, plain HTML/CSS/JS, no Node build step).
- **Capture:** one `ffmpeg` process: Windows Graphics Capture → GPU scaler → hardware H.264 (NVENC / AMF / Quick Sync, x264 fallback) → MPEG-TS on a pipe.
- **Buffer:** encoded video, split at keyframes, plus desktop audio (WASAPI loopback) and the microphone as PCM. By default the newest 30 seconds are in RAM and everything older is in temporary files; it can also be all RAM or all disk (see Replay buffer).
- **Clip:** video is stream-copied (no re-encode); audio is encoded to AAC. Track 1 = mix, track 2 = desktop, track 3 = microphone.

## Requirements

- Windows 10/11 with WebView2 (preinstalled on Windows 11).
- `ffmpeg` 8.0 or newer on `PATH`, or next to `Glint.exe`, or set in Settings. Older builds lack `gfxcapture`; Glint then falls back to Desktop Duplication.
- Optional: [blur](https://github.com/f0e/blur) v2 for the "Render with Blur" button. Glint looks for `blur-cli.exe` in `%LOCALAPPDATA%\Glint\blur`, then in a `blur` folder beside `Glint.exe`, then on `PATH`.

## Use

Run `dist\Glint.exe`. Keep `WebView2Loader.dll` beside it; the exe will not start without that file. Default keys:

| Key | Action |
| --- | --- |
| `F8` | Save the last 30 s |
| `Shift` + `F8` | Save the last 10 min |

Add, remove and rebind under **Keybinds**. One key can carry several bindings; the one with the most matching modifiers wins, an unbound extra modifier (for example `Ctrl` held to crouch) falls back to the plain key, and key presses are never swallowed from the game. The buffer length follows the longest clip binding.

Keys are read by a global keyboard hook. Windows does not feed that hook while Glint's own window has focus, so the window matches the same keybinds itself in that case; a press that arrives both ways is counted once. If Medal is still running it also reacts to `F8`: quit it or rebind one of the two.

Every key press that fires, and every clip saved or failed, is written to `%APPDATA%\Glint\glint.log`. Start Glint with the environment variable `GLINT_DEBUG_KEYS=1` to log every key event the hook sees.

**Still screens.** Windows Graphics Capture sends frames only when the picture changes. Glint never invents duplicate frames, so the file's frame rate is "up to" the setting: a busy game delivers the full rate, a quiet desktop far fewer. A clip of a still screen keeps its full length: the last picture stays up while the audio plays. The status bar shows "screen still" at those times.

**Bitrate.** Under Recording, **Bitrate** is either Auto (quality-targeted, capped by the Quality level) or a fixed rate from 5 to 100 Mbps. A fixed rate is constant bitrate without filler: moving pictures use all of it, a still screen uses less because there is nothing to spend it on. A fixed rate sets the buffer's size: 50 Mbps for 10 minutes is 3.75 GB.

**Replay buffer.** Capture → Replay buffer chooses where those gigabytes live.

| Setting | RAM | Disk | Notes |
| --- | --- | --- | --- |
| RAM + disk (default) | the newest 30 s (15 s to 2 min, your choice) | everything older | Clips up to the RAM length are saved straight from RAM |
| RAM only | the whole buffer | nothing | Fastest; a 10-minute buffer at 50 Mbps is about 4 GB of RAM |
| Disk only | about one second of video, ten of audio | the whole buffer | Lowest RAM use |

The files are temporary: `%LOCALAPPDATA%\Glint\buffer` unless you pick another folder (an SSD is best), written in 64 MB pieces that are deleted as the buffer moves past them, and removed when clipping stops or Glint quits; leftovers from a crash are cleared at the next start. Video is written at the capture bitrate while the picture moves (about 6 MB/s at 50 Mbps) and hardly at all while it is still; silent audio is not written. If the folder cannot be written or its drive has less than 1 GB free, the buffer stays in RAM and the Capture page says so. A buffer barely longer than its RAM part (longest clip under about 40 s) stays in RAM in the default mode. Changing the setting restarts the buffer.

Clips go to `Videos\Glint`. Settings, the clip index and thumbnails live in `%APPDATA%\Glint`.

Closing the window keeps the recorder in the tray and destroys the WebView, so the UI costs no memory while hidden. `Glint.exe --minimized` starts straight to the tray.

## Build

```
cd src-tauri
cargo build --release
```

The UI in `ui/` is embedded at compile time. The outputs are `target\release\glint.exe` and `target\release\WebView2Loader.dll` (ship both together), and `target\release\glint-setup.exe`, the installer.

**Publishing a release.** Raise `version` in `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit and push, then run `.\release.ps1` (needs the GitHub CLI signed in). It builds, packages `Glint-windows-x64.zip` with its checksum and `GlintSetup.exe`, and publishes them as release `v<version>`. Installed copies pick it up within a day.

## Player

Clips open in Glint's own player. `Space` play/pause, `←` `→` 5 s (`Shift` 1 s), `,` `.` one frame, `↑` `↓` volume, `M` mute, `L` loop, `F` fullscreen. In trim mode drag the two handles on the timeline or press `I` / `O` to set the start and end at the playhead. Trims are lossless and snap to keyframes (one per second).

## Clip sound, overlay, microphone

**Clip sound.** Sixteen built-in sounds, all synthesised in code (no audio files), with a volume slider, or your own `.wav` file. Set under Settings. The sound plays the moment the key is pressed.

**Clip overlay.** A small box in a corner of the recorded monitor when a clip is on disk. Pick the corner under Settings. It shows a microphone icon while the microphone is being recorded. The box never takes focus and clicks pass through it. It is an ordinary always-on-top window, so it shows over windowed and borderless games but not over exclusive fullscreen.

**Microphone noise suppression.** RNNoise (the `nnnoiseless` crate, model built in), under Recording. It runs when a clip is saved, on that clip's microphone audio, so buffering costs nothing extra; the mix track and the microphone track both get the cleaned voice. The microphone becomes mono. Measured on synthetic speech with pink noise and hum added: noise-only passages fell from -39 dB to -81 dB, speech level changed by 0.3 dB, no added delay, and 10 minutes of audio took 1.3 s.

The overlay's opacity, size and time on screen are adjustable, and its text ("Captured last 30s of Overwatch") can show the length, the game, both or neither. It follows the colour theme.

**From the command line.** With Glint running, `Glint.exe --clip` saves a clip of the shortest clip keybind's length, and `Glint.exe --show-overlay` shows the overlay. Useful for macro keys and scripts.

## Games

Glint looks at the foreground window every two seconds. It counts as a game if its executable is in a short table of known games, sits in a game store's folder (Steam, Epic, Riot, Xbox, Ubisoft, EA, GOG, a `Games` folder), or its window covers the whole monitor; browsers, launchers, players and tools are excluded by name. Minecraft (Java) is told apart by its window title. The name comes from the table, else the Steam folder, else the executable's product name. A game stays "the game being played" after it loses focus, until its process ends. Clips are tagged with it, or with "Desktop" when no game is running. A wrong guess is fixed per clip with right-click → Game.

Each game seen is remembered in `%APPDATA%\Glint\games.json` with its icon, taken from the game's executable into `%APPDATA%\Glint\icons`. The Library shows one chip per game with that icon; click to filter, right-click to pick your own icon image.

## Stretched resolutions

A stretched resolution (4:3, 5:4, 3:2 or 16:10 shown full-screen on a wider display) reaches any screen capture as a narrow picture, although the player sees it filling the screen; left alone, the clip plays back narrow with black bars. With Capture → Aspect ratio on Auto, Glint compares the current display mode with the display's own shape (its largest mode) and, when the mode is clearly narrower, widens the clip to the display's shape on the GPU: 2160×1440 on a 16:9 display is saved as 2560×1440. Modes as wide as the display, or wider, are left alone, so ultrawide and 16:10 screens are not touched. A fixed ratio or "As captured" can be chosen instead. Glint also notices when a game changes the display mode and restarts the capture at the new size.

## Discord Rich Presence

Settings → Discord. While the buffer is on, your Discord profile shows "Clipping Overwatch" (or "Clipping the desktop") and the buffer length. It talks to the Discord client on this PC through Discord's local pipe; nothing is sent anywhere by Glint. Discord only accepts a presence from a registered application, so you paste in the Application ID of one you created at discord.com/developers/applications (New Application, name it Glint, copy "Application ID"). The name and icon of that application are what Discord shows.

## Themes

Settings → Appearance has twelve colour themes. Custom CSS, applied after the theme, can restyle anything; the colours are CSS variables (`--bg`, `--panel`, `--raised`, `--raised-2`, `--line`, `--line-2`, `--text`, `--muted`, `--dim`, `--accent`, `--accent-2`, `--accent-ink`), so `:root { --accent: #ff4fd8; }` is a one-line recolour.

## Library

Right-click a clip for its menu: open, copy file, favorite, rename, set the game, move to a folder, open in the editor (trim), render with Blur, open file location, select, delete. On a selection of several clips the menu acts on all of them. Right-click a folder to rename or delete it. Folders are also listed in the sidebar; drop dragged clips on one to move them.

Drag across the grid to draw a selection box. `Ctrl` or `Shift` while dragging adds to the selection. `Ctrl`+click or the tick box on a card toggles one clip, `Shift`+click selects a range, `Ctrl`+`A` selects everything in view, `Esc` clears. While clips are selected a plain click selects instead of opening.

The bar that appears moves the selection to a folder, favorites it, copies the files to the clipboard, or sends them to the Recycle Bin (`Delete`). Dragging a selected clip carries the whole selection: drop it on a folder chip to move it there.

**Game folders.** With Settings → Game folders on, each new clip is saved into a subfolder named after its game (`Videos\Glint\Overwatch\…`), which also shows as a library folder. The game is the product name of the app in the foreground when the key is pressed; "Desktop" when that is the shell. "Sort by game" in Settings moves the clips that sit loose in the clips folder into their game's folder and leaves clips you filed yourself alone. Changing a clip's game from the right-click menu moves it to the new game's folder if it was still in its old game's folder.

Folders are real subfolders of the clips folder, one level deep. Moving a clip moves its file; a name clash in the target gets " (2)" added, never an overwrite. Deleting a folder moves its clips back to the clips folder first and deletes no clip. Rescan also picks up videos you put into those subfolders yourself.

## Editor

Open a clip and press **Edit** (or right-click → Open in editor). Under the video is a timeline: a ruler, the video as a film strip, and one row per audio source with its waveform (desktop audio, microphone, and any audio files you add).

- **Cut.** Split at the playhead, select a piece, delete it; the rest closes up in the result. `I` / `O` trim the start and end. Cut points can be dragged; double-click one to join the pieces again. Cut-out parts stay visible, dimmed.
- **Audio per piece.** Each piece of each audio row can be muted or set from 0 to 200 %. Each row also has its own mute and volume.
- **Add audio.** Drop audio files on the window, or press Add audio: each becomes a new row that can be moved and trimmed.
- **Speed** 0.25x to 4x (pitch kept) and **Colour** (presets, four sliders, your own presets) in the side column.
- **Zoom** the timeline with Ctrl + wheel, `+` / `-`, or Fit.
- **Export**, or **Export and render with Blur**. The preview plays what the export will produce: cuts are skipped and levels applied.

One kept range with nothing else changed is cut losslessly in an instant. Anything else re-encodes on the hardware encoder (falling back to x264). Changing audio produces one mixed track; otherwise all tracks are kept. The edit model is `ui/editmodel.js`; the export receives exactly what `emToOpts` builds.

Shortcuts in edit mode (also under the Shortcuts button): Space play/pause; S split; Delete cut out or restore the selected piece; M mute it; Up / Down its level; I / O trim; [ / ] previous / next cut; Left / Right 5 s (Shift 1 s); , / . one frame; + / - zoom; Shift+F fit; Ctrl+Z undo; Ctrl+Y redo; Ctrl+E export; Esc leave.

## Renders

A Blur render is a job. Jobs run in the background whether or not the clip is open: a button appears in the top bar while there are any, showing overall progress; its list shows each render with progress, Cancel, and Open for finished ones. "At once" (1 to 4) sets how many run together; the rest wait in order. Quitting Glint stops running renders.

## Import

Library → Import brings in clips recorded by NVIDIA ShadowPlay / NVIDIA app, Medal, Outplayed and SteelSeries GG Moments, or from any folder. Glint finds each program's clips folder, lists what is there grouped by game (from the folder layout, the file name, or Medal's own index), and adds what you tick. By default files stay where they are and are only listed; "Copy them into Glint's folder" copies them (into game folders if that setting is on). The originals are never moved, changed or deleted.

## Recommended settings, interface size, language

- Capture settings that suit this PC carry a blue outline and a "Recommended" tag; others offer "Use recommended". The graphics card is read from the registry. Choosing an encoder (or Blur GPU type) made for another brand of card, or the CPU encoder, asks for confirmation first.
- Settings → Appearance → Interface size scales the whole window (70 % to 150 %).
- The globe button at the top right of Settings switches language (English, Korean, Greek). Texts are looked up by their English wording in `ui/lang/<code>-*.js`; a text without a translation shows in English. `node tools/i18n-check.js ui ko` (or `el`) lists texts that still lack a translation.
- Custom theme (CSS) is a fold-out box in Appearance. "Copy AI prompt" copies a prompt that explains Glint's theme variables to an AI, so it can write a theme from a one-line description.

## Blur

The Blur page starts with a live preview: the same moving shapes drawn as the original and as Blur would render them with the current settings, following Blur's own maths (frames blended per output frame, weighting, gamma, interpolation, filters). It updates while a control is dragged, and a line under it says in plain numbers what will happen, including when a combination would do nothing. Quick presets set the common fields; less-used settings are in collapsible groups.

The Blur page has every setting of blur v2's config file, with blur's own names and defaults. Two are fixed: `preview` is off (Glint has no preview window) and `detailed filenames` is off (Glint names the output).

"Render with Blur" hard-links the clip into a private job folder with a generated `.blur-config.cfg` and runs `blur-cli` there; blur reads a config beside the input before its global one. The `-c` flag is not used: 1.92 hangs on it and 2.23 did not pick the file up. A render that prints nothing for 3 minutes is cancelled.

Blur must sit on a short path. Its bundled VapourSynth fails with "Failed to initialize VSScript" when the full path of its files nears Windows' 260-character limit.

## Self-tests

All run without any window.

```
glint.exe --hotkey-test report.json
glint.exe --bench report.json --height 1080 --fps 120 --secs 20 --clip 15
glint.exe --folders-test report.txt
glint.exe --buffer-test report.txt
glint.exe --blur-test in.mp4 out.mp4 report.txt
```

`--hotkey-test` injects `F13` combinations and checks binding resolution. `--bench` runs the real recorder, reports CPU, RAM, GPU encoder load and delivered frame rate, then saves a clip and verifies it with `ffprobe`; `--bitrate 50` sets a fixed rate and `--buffer ram|mixed|disk` (with `--ram-secs N`) chooses where the buffer lives. `--buffer-test` feeds the video and audio rings made-up data on a made-up clock and checks that clips cut across RAM and disk are byte-exact, that files are deleted as the buffer moves on, and that an unwritable folder falls back to RAM. `--folders-test` moves, renames and deletes library folders on scratch files and refuses to run unless `GLINT_DATA_DIR` is set. `--blur-test` renders one file through the Blur wrapper. Set `GLINT_DATA_DIR` to keep test output out of your real library.

## Measured (RTX 5060 Ti, i5-11600, 2160×1440 @ 280 Hz)

Recorder cost while buffering, video + desktop audio + mic. CPU is the share of the whole 12-thread CPU.

| Output | Delivered fps | CPU | ffmpeg RAM | Glint RAM | NVENC load | 10 s clip save |
| --- | --- | --- | --- | --- | --- | --- |
| 1080×720 @ 60 | 60.0 | 0.95 % | 95 MB | 17 MB | 12 % | 0.86 s |
| 1620×1080 @ 60 | 59.9 | 0.81 % | 110 MB | 17 MB | 18 % | 0.92 s |
| 2160×1440 @ 60 | 59.7 | 0.71 % | 130 MB | 17 MB | 22 % | 0.67 s |
| 2160×1440 @ 120 | 119.9 | 1.21 % | 131 MB | 17 MB | 30 % | 0.61 s |
| 2160×1440 @ 240 | 239.9 | 1.69 % | 131 MB | 17 MB | 42 % | 0.85 s |
| 2160×1440 @ 280 | 280.1 | 1.77 % | 131 MB | 17 MB | 41 % | 1.08 s |
| 1620×1080 @ 280 | 280.0 | 2.03 % | 111 MB | 17 MB | 31 % | 1.11 s |

"Glint RAM" excludes the replay buffer itself, which is shown live in the status bar.

These rows were measured with an earlier capture chain that padded the stream to a constant rate, so every row encoded the full frame count. The current chain drops duplicates instead, so its load is the same or lower; it was re-run once at 2160×1440 @ 60 on a quiet desktop (0.49 % CPU, clip valid) but the table was not repeated.

10-minute run at 2160×1440 @ 60 (mostly static desktop): 60.00 fps delivered over 616 s, 0.70 % CPU, no encoder restarts. Buffer held 154 MB of video plus 234 MB of audio. The 10-minute clip (36 052 frames, 3 audio tracks, 173 MB) took 22 s to write, mostly AAC encoding; the encoder setting was made about twice as fast afterwards and that change has not been re-timed on a 10-minute clip. Busy game footage fills the buffer faster, up to the quality ceiling shown under Recording.

Fixed bitrate was checked with a synthetic noise source through the same thinning filter and encoder settings (140 fps thinned to 60, 50 Mbps asked): the stream came out at 54 Mbps including MPEG-TS overhead. The same test showed the earlier builds delivered only about 43 % of their stated ceiling on this 280 Hz monitor, because the encoder budgets bits at the capture rate, not the thinned rate; rates are now scaled to compensate, so Auto mode's real peak bitrate is higher than when the table above was measured. A live capture at a fixed rate ran clean, but the screen was still, so the rate was not observed on real footage.

Not measured: load while a GPU-bound game is running, and audio/video sync to the frame.

## Layout

```
src-tauri/src/
  recorder.rs   ffmpeg supervisor, MPEG-TS parser, replay ring (RAM and disk), snapshots
  spill.rs      the buffer's temporary files
  audio.rs      loopback + mic capture into wall-clock-anchored rings
  clip.rs       mux a snapshot to MP4; lossless trim
  hotkeys.rs    low-level keyboard hook and binding resolution
  library.rs    clip index, ffprobe, thumbnails
  blur.rs       blur-cli wrapper
  sound.rs      synthesised clip sounds
  denoise.rs    microphone noise suppression (RNNoise)
  games.rs      game detection, game list, icons
  discord.rs    Discord Rich Presence over the local pipe
  editor.rs     per-track audio, waveforms and film strip for the editor; export
  jobs.rs       background render queue
  import.rs     import from other clipping software
  lang.rs       the backend's own texts (overlay, tray) in other languages
  update.rs     update check, download and swap from GitHub releases
  setup.rs      GlintSetup.exe, the one-time installer (a second program in the same crate)
  core.rs       shared state, settings application, status
  app.rs        Tauri window, tray, toast, UI commands
  bench.rs      headless benchmark     keys.rs  hotkey self-test
tools/          i18n-check.js (translation coverage), serve.js (serve ui/ to a browser; mock.js stands in for the backend)
ui/             index.html, style.css, app.js (library, settings), editor.js/.css (viewer, player, editor),
                editmodel.js (the edit model), timeline.js/.css, jobs.js/.css (renders button), importer.js/.css,
                blurpage.js/.css (Blur settings and live preview), themes.js, i18n.js and lang/ (translations),
                toast.html (clip overlay), mock.js (browser preview only)
```
