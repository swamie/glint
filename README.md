# Glint

Offline instant-replay clipper for Windows. Glint keeps the last few minutes of your screen in a replay buffer (in RAM, in temporary files, or both) and saves a clip only when you press a key. No account, no upload. Your clips never leave your PC; the only things Glint does online are asking GitHub once a day whether a newer version exists (it can be switched off) and, only when you press Send in the bug-report dialog, sending that report to the developers.

## Install

Download **GlintSetup.exe** from the [latest release](https://github.com/swamie/glint/releases/latest) and run it. It installs Glint for your Windows account (no administrator rights needed) and fetches what Glint needs, all from GitHub releases:

| What | From | Notes |
| --- | --- | --- |
| Glint | github.com/swamie/glint | to `%LOCALAPPDATA%\Programs\Glint` |
| ffmpeg, ffprobe | github.com/GyanD/codexffmpeg | skipped if a suitable ffmpeg is already on PATH |
| Blur v2.23 | github.com/f0e/blur | optional, asks first (about 125 MB) |
| WebView2 runtime | microsoft.com | only if missing; Windows 11 has it built in |

The installer is a normal setup window: pick the folder, tick what to install (ffmpeg, Blur, Start menu and desktop shortcuts), watch the progress, and cancel at any time; a cancelled or failed install leaves nothing half-done. It adds an entry under Installed apps; removing Glint from there opens a small window that asks whether to remove your settings too (your clips always stay). For scripts, `GlintSetup.exe --yes` installs with the defaults and no window, and writes what it did to `%TEMP%\GlintSetup.log` (other flags: `--dir <folder>`, `--no-ffmpeg`, `--force-ffmpeg`, `--no-blur`, `--no-shortcuts`, `--no-launch`, `--uninstall`). The installer is not code-signed, so Windows SmartScreen may show "Windows protected your PC": choose More info → Run anyway.

**Updates.** The update button in the top bar (an arrow down into a tray) checks GitHub for a newer release about once a day; click it to check right away. When there is one, the button fills with the theme color and reads "Update x.y.z"; a click downloads it, swaps it in and restarts Glint. The download's SHA-256 is checked against the release's checksum file. Switch the daily check off under Settings → Updates.

**Automatic install.** With Settings → Updates → "Install updates automatically" on (the default), Glint does that click itself: it downloads the new version, swaps it in and restarts in the tray, with clipping on or off as it was. It only does so after three minutes in a row in which its window is closed, no render, compression, export, import or component install is running, and no game is running. Never while a game is running: Glint looks at the game being played and at every running program (games it knows, games it has seen you play, programs inside a game store's folder). If a game starts during the download, the new files are in place but Glint keeps running and restarts once the game is gone. With the window open, the update button waits for your click as before.

- **Stack:** Rust + Tauri 2 (WebView2 UI, plain HTML/CSS/JS, no Node build step).
- **Capture:** one `ffmpeg` process: Windows Graphics Capture → GPU scaler → hardware H.264 (NVENC / AMF / Quick Sync, x264 fallback) → MPEG-TS on a pipe.
- **Buffer:** encoded video, split at keyframes, plus desktop audio (WASAPI loopback) and the microphone as PCM. By default the newest 30 seconds are in RAM and everything older is in temporary files; it can also be all RAM or all disk (see Replay buffer).
- **Clip:** video is stream-copied (no re-encode); audio is encoded to AAC. Track 1 = mix, track 2 = desktop, track 3 = microphone.

## Requirements

- Windows 10/11 with WebView2 (preinstalled on Windows 11).
- `ffmpeg` 8.0 or newer on `PATH`, or next to `Glint.exe`, or set in Settings. Older builds lack `gfxcapture`; Glint then falls back to Desktop Duplication. If the graphics card's encoder accepts no frames from the capture (seen with AMD's AMF on integrated Radeon graphics), Glint feeds it through system memory instead, and if that fails too it encodes with x264 on the CPU; the log says which.
- Optional: [blur](https://github.com/f0e/blur) v2 for the "Render with Blur" button. Glint looks for `blur-cli.exe` in `%LOCALAPPDATA%\Glint\blur`, then in a `blur` folder beside `Glint.exe`, then on `PATH`.

## Use

Run `dist\Glint.exe`. Keep `WebView2Loader.dll` beside it; the exe will not start without that file. Default keys:

| Key | Action |
| --- | --- |
| `F8` | Save the last 30 s |
| `Shift` + `F8` | Save the last 10 min |

Add, remove and rebind under **Keybinds**. One key can carry several bindings; the one with the most matching modifiers wins, an unbound extra modifier (for example `Ctrl` held to crouch) falls back to the plain key, and key presses are never swallowed from the game. The buffer length follows the longest clip binding.

**Two monitors.** On a PC with more than one monitor, each clip keybind has a monitor choice: the capture monitor (the one picked under Capture, the default) or a particular monitor. Glint keeps a replay buffer for every monitor a keybind uses, all at once, so one key can save the left screen and another the right. Each buffered monitor is its own capture (its own ffmpeg, its own share of the graphics card's encoder, memory and disk), sized to the longest keybind that points at it; the status bar shows "+1 monitor". The saved-clip overlay appears on the monitor that was clipped. Measured with a second monitor buffered: about 120 MB more memory, CPU under 1 %.

**Picking a monitor, and what GAME mode records.** Wherever a monitor is picked, Glint draws your monitors in their real arrangement with a live picture of each. In GAME mode Glint decides what the game is from layered evidence: your own pick and your "never" list first, then the installed libraries it reads from disk (Steam, Epic, Riot, Xbox, GOG, Ubisoft, Battle.net, EA), games played before, and the window itself (covering its monitor, no title bar, in front for a while); launchers, browsers, chat apps and overlays are never games. A game you pick stays picked. The switch's menu lists what Glint sees and why, and the status line says what is recorded: the game's window; its monitor by Desktop Duplication when the game runs in exclusive fullscreen; its monitor only while the game is in front when the window cannot be captured; nothing while it is minimized; or that Glint is waiting for the game. It never falls back to the desktop without saying so. `glint.exe --live-check <folder>` writes real monitor pictures and what detection sees on this PC into a folder, for diagnosing.

**Edit the last clip.** A keybind with the action "Edit the last clip" brings Glint's window up with your newest clip open in the editor.

**Switching the capture monitor.** A keybind can have the action "Switch the capture monitor" instead of saving a clip. Set to "Next monitor" it steps through the monitors in order; set to a particular monitor it goes straight to that one. It changes the same setting as the monitor choice under Capture, so the clip keys set to "capture monitor", the tray's "Save clip now" and the Clip button all follow it, and a small box on the new monitor names it. A monitor that was not being buffered before the switch starts with an empty buffer.

**Components.** Settings → Components lists what Glint needs (ffmpeg, ffprobe, and Blur, which is optional), says which are installed, missing or too old, and installs them with one button: ffmpeg from the GyanD builds on GitHub, Blur 2.23 from its own releases. ffmpeg goes beside Glint.exe, or into `%LOCALAPPDATA%\Glint\bin` when that folder cannot be written. If ffmpeg is missing, the status bar's message has an "Install ffmpeg" button; capture starts as soon as it is in place.

**Game audio.** Capture → "Separate game audio track" (off by default) records the current game's sound on its own, using Windows' per-process loopback (Windows 10 version 2004 or newer). A clip then has a last track named Game: Mix, Desktop, Microphone, Game. Desktop still holds everything the PC played, the game included. In the editor, the Game track is heard only where Desktop is muted, so muting Desktop and keeping Game drops voice chat and music.

**Edit together.** Select two or more clips, right-click → Edit together. The clips open in the editor as separate clips, in the order you selected them: drag a clip in the Clips strip to move it, right-click it to move it or cut it out, and trim, split or color each as usual. Desktop, Microphone and Game stay separate audio tracks, matched by name, even when the clips were recorded with different tracks. Nothing is added to the library until you export; the original clips are not changed. A project is only the list of its clips and your edit: it references the original clip files, as a video editor's project file does. Adding clips is instant and no copy is made. The editor plays each piece from its own clip's file. On export Glint reads the same files: when every piece comes from one clip, that clip is exported directly (one trimmed piece is still the lossless cut); otherwise the clips that are used are joined into a temporary file for the export only (clips of the same format without re-encoding; clips that differ are re-encoded to match, and nothing is made smaller), and it is removed afterwards. If a clip of a project is deleted from the library, the project still opens: its pieces are black and say which clip is missing, and the export names it instead of starting. Projects made by 0.1.37 or earlier keep their working file (in `.drafts` inside the clips folder) and work as before.

**Windows 10 and the yellow border.** Windows Graphics Capture draws a yellow border around what it captures, and Windows only lets a program hide it from Windows 11 on, and only while Settings → Privacy & security → Screenshot borders lets desktop apps turn it off. Where it cannot be hidden (Windows 10, or that setting off), capture method Auto uses Desktop Duplication for DESKTOP mode, which has no border. GAME mode still records the game's window by Graphics Capture there, so Windows draws the yellow border around the game on your screen. The border is on the screen only, never in the clip. To avoid it in GAME mode, set the capture method to Desktop Duplication by hand; GAME mode then records the game's whole monitor instead of only its window.

**Taskbar icon.** Settings → "Match the taskbar icon to the theme" colors Glint's taskbar and tray icon with the theme's accent (`--accent`, `--accent-2`), a custom theme's included.

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

**Editor projects.** Tools → Editor lists your editing sessions. Each one is a project: the clips in it, your cuts, their order, levels and colors, saved as you work. "Edit together" makes one; New project starts an empty one. Beside the editor a list of your library lets you add more clips: click, Ctrl+click, Shift+click or drag a box to select, then double-click, press Enter, or drag them onto the timeline. New clips go to the end and your edits stay. Close a project at any time and open it again later; after an export it stays in the list, tagged Exported, until you delete it. A clip of another size than the project's first clip is shown fitted inside the project's picture. The same clip may be in a project twice. **Replace instead of a new copy.** The Export box has a tick, "Replace the original clip" (remembered). With it on, the export takes the clip's place: the original goes to the Recycle Bin and the export gets its name, file name, date and favourite. In a project the tick reads "Replace clips of the project": before the export Glint lists the project's clips and you tick the ones that go to the Recycle Bin; the others stay, and the export is a new clip named after the project. A clip that a render still uses is not replaced. On the Editor page, Ctrl+click, Shift+click, the tick on a card or a box drawn over the cards select several projects, to rename (one name, numbered) or delete them together; F2, Delete, Ctrl+A and Esc work there too.

**Yellow capture border (Windows 11).** Windows draws a yellow border around a captured screen unless the program may turn it off (Settings → Privacy & security → Screenshot borders → "Let desktop apps turn off the screenshot border"). Glint asks for no border. When a game changes the screen mode, Glint waits for the new mode to settle before it restarts the capture, and restarts it once more if the border still shows. If it keeps coming back, pick Desktop Duplication under Capture → Capture method: it has no border.

## Build

```
cd src-tauri
cargo build --release
```

The UI in `ui/` is embedded at compile time. The outputs are `target\release\glint.exe` and `target\release\WebView2Loader.dll` (ship both together), and `target\release\glint-setup.exe`, the installer.

**Publishing a release.** Raise `version` in `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json`, commit and push, then run `.\release.ps1` (needs the GitHub CLI signed in). It builds, packages `Glint-windows-x64.zip` with its checksum and `GlintSetup.exe`, and publishes them as release `v<version>`. Installed copies pick it up within a day.

## Player

Clips open in Glint's own player. `Space` play/pause, `←` `→` 5 s (`Shift` 1 s), `,` `.` one frame, `↑` `↓` volume, `M` mute, `L` loop, `F` fullscreen. In trim mode drag the two handles on the timeline or press `I` / `O` to set the start and end at the playhead. Trims are lossless and snap to keyframes (one per second).

**Previous and next clip.** In the viewer, the arrows just outside the clip's left and right edges (or Page Up and Page Down) open the clip before and after this one, in the order the library shows them. They are hidden at the first and last clip and while editing. Fullscreen has them too: they show with the player's controls while the pointer moves, a little see-through (Settings → Appearance → Clip arrows in fullscreen), and the next clip opens without leaving fullscreen.

**Volume and jumps.** The volume is on the left of the controls, after play, back and forward. Right-click the back or forward button to choose how far it jumps (1, 2, 3, 5, 10, 15 or 30 seconds); the Left and Right keys use the same step, and Shift + Left/Right stays at 1 second.

## Clip sound, overlay, microphone

**Audio mixer.** Settings → Audio starts with a mixer: one strip each for Desktop audio, Microphone and Game audio, with a fader (-40 to +12 dB, double-click for 0 dB), a two-channel level meter and an on / off button. A fader changes how loud that source is in every clip saved from then on (it is applied as the sound is captured); the meters show what is being recorded while the replay buffer is on.

**Clip sound.** Sixteen built-in sounds, all synthesised in code (no audio files), with a volume slider, or your own file: WAV, MP3 or other audio. The volume slider applies to it too (100% is the file's own loudness), and a bar under it shows the file's loudness over time: drag its ends to choose the part that is played (at most 15 seconds), for example to skip silence at the start. Set under Settings. The sound plays the moment the key is pressed.

**Clip overlay.** A small box in a corner of the recorded monitor when a clip is on disk. Pick the corner under Settings. It shows a microphone icon while the microphone is being recorded. The box never takes focus and clicks pass through it. It is an ordinary always-on-top window, so it shows over windowed and borderless games but not over exclusive fullscreen.

**Microphone noise suppression.** RNNoise (the `nnnoiseless` crate, model built in), under Recording. It runs when a clip is saved, on that clip's microphone audio, so buffering costs nothing extra; the mix track and the microphone track both get the cleaned voice. The microphone becomes mono. Measured on synthetic speech with pink noise and hum added: noise-only passages fell from -39 dB to -81 dB, speech level changed by 0.3 dB, no added delay, and 10 minutes of audio took 1.3 s.

The overlay's opacity, size and time on screen are adjustable, and its text ("Captured last 30s of Overwatch") can show the length, the game, both or neither. It follows the color theme. With **Compact** on, a saved clip shows only the two stars in a small square, with no text; a problem (a failed clip, a paused or empty buffer) still shows the full box.

**Audio devices.** Settings → Audio. "Desktop audio device" is the output whose sound is recorded (the headphones or speakers the game plays on), "Microphone device" the microphone; "Default" follows the Windows default device. If a chosen device is unplugged, Glint records the default one and says so, and uses the chosen one again when it is back. A change applies at once without restarting the video capture. The desktop audio, microphone, noise suppression and game audio track switches are in the same section.

**From the command line.** With Glint running, `Glint.exe --clip` saves a clip of the shortest clip keybind's length, and `Glint.exe --show-overlay` shows the overlay. Useful for macro keys and scripts.

## Game or Desktop

The switch at the right of the top bar chooses what is recorded. **DESKTOP** (the default) records the whole monitor and everything the PC plays. **GAME** records only the game: the picture is the game's own window, the sound is the game's own sound plus your microphone when it is on, so nothing else on the desktop (other windows, voice chat, music) is in the clip. A game-mode clip has the tracks Mix (game and microphone), Game and Microphone.

In GAME mode the buffer runs only while a game does: with none, the status reads "Waiting for a game" and recording starts by itself when one appears. When the game closes, what the buffer holds is kept until it ages out, so a clip key still saves the last moments. When another game takes over, the recording moves to it. The clip's size follows the game's window (the Capture height still caps it). If a game's window cannot be captured (the capture method set to Desktop Duplication by hand, an ffmpeg without window capture, or a window that gives no picture), Glint records the monitor the game is on and keeps the game-only sound; the status then says "whole screen", and the switch's menu says why and what to change. A game in exclusive fullscreen is recorded by its monitor too, and goes back to window capture as soon as it is a window again.

Right-click the switch to choose the game: "Detect the game automatically", a list of the running programs that have a window to pick one by hand, and "This is not a game" for a wrong guess. Picks and "not a game" marks are remembered by program name.

With desktop sound on in GAME mode, the first track of a clip always holds everything that was recorded: if the game plays on other headphones or speakers than the ones Glint records (a headset with separate game and chat devices), the game's own sound is mixed into it.

## Games

Glint looks at every visible window every two seconds and scores them: a program you picked or marked, the table of known games, games it has seen before, game store folders and a window that covers the monitor count for it; launchers, helpers, browsers and small windows are left out; the window in front and the game already being played are preferred. A window counts as a game if its executable is in a short table of known games, sits in a game store's folder (Steam, Epic, Riot, Xbox, Ubisoft, EA, GOG, a `Games` folder), or its window covers the whole monitor; browsers, launchers, players and tools are excluded by name. Minecraft (Java) runs inside the Java runtime, so it is recognised by any of: its window title (the game's own, or a client's such as Lunar, Badlion, Feather or LabyMod), the window class its graphics library creates, the folder its Java runtime sits in (a launcher's own), or a Java window that fills the screen; its icon is taken from a Minecraft launcher on the PC, if there is one. The name comes from the table, else the Steam folder, else the executable's product name. A game stays "the game being played" after it loses focus, until its process ends. Clips are tagged with it, or with "Desktop" when no game is running. A wrong guess is fixed per clip with right-click → Game (select several clips first to fix them together).

Every game is filed under one name: "Overwatch 2" (as NVIDIA names it) and "Overwatch" are the same chip, a window title such as "Minecraft 1.8.9" is Minecraft, and names that are not games at all ("Screen Recording", a VPN or a browser that happened to be in front) count as Desktop. This is applied to new clips, to imports, and once at start to the clips already in the library. Clips another clipper saved as plain screen recordings carry no trace of their game, so those stay under Desktop until you set it.

Each game seen is remembered in `%APPDATA%\Glint\games.json` with its icon, taken from the game's executable into `%APPDATA%\Glint\icons`. The Library shows one chip per game with that icon; click to filter, right-click to pick your own icon image.

**Icons.** A game Glint has seen running gets the icon of its program. A game that only came in with imported clips gets its icon when the game is installed on this PC: Glint looks in Steam, Epic, Xbox, Riot, Ubisoft, GOG, Roblox, the installed-programs list and Start menu shortcuts, by name, without going online. A game that is not installed keeps its colored letter. Right-click a game chip → Find icon again looks once more; Change icon takes a picture of your own.

## Monitors

The monitor chosen under Capture (and a keybind's own monitor) is named to ffmpeg by its Windows handle, or by its place among the graphics card's outputs for Desktop Duplication, not by a number: ffmpeg counts monitors in another order than Glint on some PCs, which made clips show the other monitor.

## Stretched resolutions

A stretched resolution (4:3, 5:4, 3:2 or 16:10 shown full-screen on a wider display) reaches any screen capture as a narrow picture, although the player sees it filling the screen; left alone, the clip plays back narrow with black bars. With Capture → Aspect ratio on Auto, Glint compares the current display mode with the display's own shape (its largest mode) and, when the mode is clearly narrower, widens the clip to the display's shape on the GPU: 2160×1440 on a 16:9 display is saved as 2560×1440. Modes as wide as the display, or wider, are left alone, so ultrawide and 16:10 screens are not touched. A fixed ratio or "As captured" can be chosen instead. Glint also notices when a game changes the display mode and restarts the capture at the new size.

## Render rules, GIF and upscaling

**Render rules.** Tools → Render rules. A rule is a name and a row of blocks, built like the clip-name builder: Canvas (a layout you saved in the editor's Canvas panel: the clip becomes that vertical video, its pieces fitted to the clip's picture), Color (one of the editor's looks or your own), Blur (with your Blur settings), Upscale, Compress (to a size in MB, with the Compress page's settings), GIF and Copy (to the clipboard). Drag the blocks to order them, drag one out to remove it, click one for its options. Orders that cannot work are prevented: Canvas goes first with Color beside it (one export does both), after GIF only Copy can follow, and Copy is last. A rule whose layout was deleted says so and does not run. Run a rule on one clip or several from the right-click menu (Render rule), from the viewer's side panel, or from the editor's Export box ("Then run": the rule runs on the clip you just exported, so trim → rule is one click). The steps run in order as one job in the Renders list, which shows the step and its percent; only the final result is kept, as "<clip> (<rule name>)" beside the clip (a GIF beside it, outside the library). A step with nothing to do (the clip is already small enough or already that tall) is skipped.

**Export as GIF.** Right-click a clip → Export as GIF…. Pick the width (the clip's own, 720, 480 or 320 px), the frames a second (10 to 50) and, if wanted, a size limit (10, 25 or 50 MB): a GIF over its limit is made again smaller. The whole clip becomes the GIF, so trim first; a GIF has no sound. It is saved beside the clip as "<title>.gif" and is not added to the library; when it is done a message offers Show in folder and Copy.

**Upscale for YouTube.** Right-click a clip → Upscale for YouTube…. YouTube gives larger uploads more bitrate, so a 1440p or 4K upload looks clearer there; upscaling adds no detail and the file gets much bigger, so do it last, right before uploading. Pick 1080p, 1440p or 4K, Sharp (every pixel becomes a block of pixels, the usual choice for games) or Smooth (Lanczos), and the quality. It uses the hardware encoder when there is one (NVENC, AMF, Quick Sync), else libx264; sound and track names are kept, frame rate and timing are unchanged. A picture wider than 4096 px comes out as HEVC. The result is a new clip, "<title> (4K)".

## Themes

Settings → Appearance has twelve color themes. Custom CSS, applied after the theme, can restyle anything; the colors are CSS variables (`--bg`, `--panel`, `--raised`, `--raised-2`, `--line`, `--line-2`, `--text`, `--muted`, `--dim`, `--accent`, `--accent-2`, `--accent-ink`), so `:root { --accent: #ff4fd8; }` is a one-line recolor.

**Custom colors.** Settings → Appearance → Custom colors changes single colors on top of the picked theme: accent, background, panels, buttons and cards, lines, text, secondary text. Each has a round swatch that opens Glint's color picker: drag in the square and on the hue strip, or type a hex or R G B value. The whole window shows the color while you pick; Enter or a click outside keeps it, Esc cancels. Colors you did not set follow the ones you did (a readable text color on the accent, panels and lines from the background). Custom CSS still wins over them.

**Presets.** The theme tiles under Settings → Appearance are the built-in themes followed by your own presets and "+ New preset", which saves the look on screen under a name. Drag a tile to another place to reorder them; the order is kept. A preset is a built-in theme plus your colors and your custom CSS. The built-in themes never change: the first color you change while one of them is picked makes a preset called "My theme" and goes on there. Right-click a preset to rename, duplicate or delete it; right-click a built-in theme to duplicate it. The colors and the custom CSS sit in one box, "Customise colors", under the tiles.

**Closing the window.** Settings → Startup → "When I close the window (X)": "Keep Glint running in the tray" (the buffer keeps running and the clip keys still work; quit from the tray icon) or "Quit Glint" (everything stops; Glint asks first if a render, compression or export is still running).

## Library

The sidebar folds to a strip of icons with the button at the left of the top bar. Folders sit under one "Folders" row there, closed until you open it (it opens by itself while clips are being dragged); the folder chips above the grid run from the fullest folder to the emptiest. Search is an icon in the top bar that opens when clicked or with Ctrl K.

Double-click a clip's name to rename it in place (Enter saves, Esc cancels). Renaming also renames the file on disk; characters Windows does not allow are dropped and a name already in use gets " (2)". The status bar's right side reads `current/max FPS`, CPU, and the memory of the whole program (Glint, the replay buffer in RAM, ffmpeg and the window's WebView).

Right-click a clip for its menu: open, copy file, favorite, rename, set the game, move to a folder, open in the editor (trim), render with Blur, open file location, select, delete. On a selection of several clips the menu acts on all of them. Right-click a folder to rename or delete it. Folders are also listed in the sidebar; drop dragged clips on one to move them.

Drag across the grid to draw a selection box. `Ctrl` or `Shift` while dragging adds to the selection. `Ctrl`+click or the tick box on a card toggles one clip, `Shift`+click selects a range, `Ctrl`+`A` selects everything in view, `Esc` clears. While clips are selected a plain click selects instead of opening.

The bar that appears moves the selection to a folder, favorites it, copies the files to the clipboard, or sends them to the Recycle Bin (`Delete`). Dragging a selected clip carries the whole selection: drop it on a folder chip to move it there.

**Game folders.** With Settings → Game folders on, each new clip is saved into a subfolder named after its game (`Videos\Glint\Overwatch\…`), which also shows as a library folder. The game is the product name of the app in the foreground when the key is pressed; "Desktop" when that is the shell. "Sort by game" in Settings moves the clips that sit loose in the clips folder into their game's folder and leaves clips you filed yourself alone. Changing a clip's game from the right-click menu moves it to the new game's folder if it was still in its old game's folder.

Folders are real subfolders of the clips folder, one level deep. Moving a clip moves its file; a name clash in the target gets " (2)" added, never an overwrite. Deleting a folder moves its clips back to the clips folder first and deletes no clip. Rescan also picks up videos you put into those subfolders yourself.

**Chip rows.** The game chips and the folder chips each stay on one line; "{n} more" opens the rest and "Show fewer" closes them again. While a folder or game filter is on, a trail beside the title (`All clips › folder › game`) steps back.

**Clip names.** Settings → Storage → Clip names decides how new clips are named. Drag the blocks Game, Date, Time, Length and Text (your own words) into any order, drag one out or press × to remove it, click a block under Add to put it in; the preview shows a name as it will be. With only Game and Time a clip is called `Overwatch 13-35-44`. Two clips that would get the same file name get ` (2)` as before.

**Rename several clips.** Select clips, right-click → Rename {n} clips…. The same blocks build the names, plus Number (in the order you selected the clips; choose the start and the digits) and Current name. The list shows every old and new name before anything changes; afterwards Undo puts the old names back. A clip a job is using is left out, and a clip imported without copying only changes its name inside Glint: the other program's file is never renamed.

**Folder colors.** Right-click a folder in the sidebar or on its chip → Color: eight colors, Custom… (Glint's color picker) or No color. The folder's icon and a marker take the color; renaming the folder keeps it.

## Editor

Open a clip and press **Edit** (or right-click → Open in editor). Under the video is a timeline: a ruler, the video as a film strip, and one row per audio source with its waveform (desktop audio, microphone, and any audio files you add).

- **Cut.** Split at the playhead, select a piece, delete it; the rest closes up in the result. `I` / `O` trim the start and end. Cut points can be dragged; double-click one to join the pieces again. Cut-out parts stay visible, dimmed.
- **Audio per piece.** Each piece of each audio row can be muted or set from 0 to 200 %. Each row also has its own mute and volume.
- **Add audio.** Drop audio files on the window, or press Add audio: each becomes a new row that can be moved and trimmed.
- **Speed** 0.25x to 4x (pitch kept) and **Color** (presets, four sliders, your own presets) in the side column.
- **Trim brackets.** A bracket stands at each end of the part that is kept. Drag one inward to crop the start or the end, back out to give footage back; `I` and `O` do the same to the playhead. Right-click a bracket to reset that side.
- **Select.** Click a piece, Ctrl+click to add or remove one, Shift+click for a block, or drag across the lanes to draw a selection box. The toolbar buttons and shortcuts act on everything selected.
- **Right-click** a piece for its menu: split here, cut out or restore, and per piece of one track: mute, level (25 to 200 %), solo (silence the other tracks there), or on the video row hide the picture (black, the audio plays on).
- **Hide cut parts** (the menu, the toolbar toggle or `H`) closes the cut-out parts up, so the timeline shows the clip as it will be saved. A thin mark stays where something was cut; right-click it to restore that part.
- **Color per piece.** The Color pane works on the selected pieces, or on the whole clip when nothing is selected, and says which.
- **Text, shapes, drawing and blur.** The row of tools over the picture: Select, Text, Rectangle, Ellipse, Line, Arrow, Pen and Blur box. Pick one and click or drag on the picture; with Select, drag an item to move it, drag its handles to resize it, double-click a text to change its words, Delete removes it. Color, thickness (or text size) and fill are in the side column under "Text and drawing", with the time the item is on screen: the whole clip at first; "From the playhead", "Until the playhead", typed times or the bar's ends change it. A blur box blurs what is inside it, with a strength slider. Everything is undone and redone like any other edit and is saved with a project.
- **Fade.** Fade in and Fade out (seconds, 0 = off): the picture fades from and to black at the start and the end of the exported clip, and the sound with it.
- **Crop.** Under Canvas, the shape "Crop to a part of the picture" shows a box on the picture: drag it, or pull its edges and corners; what lies outside is dimmed and cut away. The exported clip has the box's own size in pixels. "See the result" shows the cropped picture, "Choose the part" the box again.
- **Canvas (vertical videos).** Under Canvas, pick a shape: 9:16 for TikTok, Shorts and Reels, 1:1, 4:5, 16:9 or a custom size, each with a choice of sizes (1080 × 1920 is the usual one). The result is then a picture of that size on which pieces of the clip's picture are placed. A layout sets the pieces in one click: Fill (the middle of the picture fills the canvas), Whole picture (the clip fitted, the background above and below), Gameplay and two strips (the gameplay in the middle, a zoomed strip above for the kill feed and below for the health bar) and Gameplay and one strip. "Arrange the pieces" (also the first button of the tool row over the picture) shows each piece as a numbered box: drag it to move it, drag a corner to make it bigger or smaller, drag an edge to crop it or show more, turn the mouse wheel over it to zoom what it shows; boxes snap to the canvas's edges and middle and to each other (hold Alt for no snapping). "The clip's picture" shows the original picture with the box each piece is cut from, to drag over another part; double-clicking a piece switches between the two views. "Add a piece" lets you drag a box around what a new piece should show; Delete removes the selected piece. A piece is never stretched. The background behind the pieces is the clip blurred, black or a color. "Save this layout…" keeps the shape, size, background and pieces under "My layouts" for any other clip. Text, shapes, drawings and blur boxes are placed on the canvas. Everything is undone and redone like any other edit and is saved with a project; an export with a canvas is always a re-encode.
- **Keyframes.** Text, shapes, drawings, blur boxes and canvas pieces can move and change size over time. Select the item (or the canvas piece while arranging), put the playhead where the movement should start and click "Add a keyframe"; move the playhead, then drag or resize the item on the picture: that sets a second keyframe, and the item travels between the two. Once an item has a keyframe, every move or resize sets one at the playhead. The keyframes show as diamonds on the item's time bar (click one to go there); the arrows jump to the previous and next one, "Remove this keyframe" deletes the one under the playhead, and "Smooth" makes the movement start and stop gently instead of running at one speed. A canvas piece keeps its proportions between keyframes too, so a slow zoom or pan across the picture is two keyframes. Cuts, moved parts and the speed are taken into account in the export.
- **The workspace.** While you edit, the editor fills the whole window: a top row with the clip's or project's name, the editor's buttons and the window's minimize, maximize and close; in a project the Library list on the left; the picture in the middle; the settings (Speed, Color, Canvas, Text and drawing, Fade, Export) on the right as groups that fold with a click on their title, with Export always reachable at the bottom; and the timeline across the full width below. Drag the borders between the parts to size them (double-click a border to reset it); the buttons in the top row fold the library and the settings away. Sizes and folds are remembered. The plain viewer for watching a clip is unchanged.
- **Rows on the timeline.** The picture is V1; everything you put on the picture (texts, shapes, drawings, blur boxes, pictures, GIFs, clips) shows as a block on a row above it (V2, V3 ...): drag a block to move it in time, drag an end to trim it, click it to select it, right-click it for Crop, Duplicate and Delete; dragging it up or down changes what lies on top. Blocks snap to the playhead, cuts, bookmarks and each other (hold Alt for no snapping). The sound rows are A1, A2 ... with their names; the + at the end of the heads adds an audio file. A timecode at the left of the tool row shows the playhead.
- **After the video.** The timeline has free space after the end of the video and the playhead can go there. A block or an added audio file that plays through cuts can be moved or stretched past the end: the result then runs on that long, with a black picture and without the clip's own sound.
- **Pieces.** What lies on V1 are pieces: a stretch of a clip (where it starts in the clip and how long it is) at a place on the timeline. Cutting never changes the clip's file and makes no new file; the edit stores numbers. A cut really removes: Split (S) or the blade makes two pieces, Delete takes the selected piece off the timeline and closes up (Shift+Delete leaves a gap), and no greyed stretch stays behind. Dragging a piece moves that piece alone. Each end of a piece is a handle: dragged inward it trims the piece (a gap stays where it was), dragged outward it brings back the footage that was cut away there, as far as the clip has footage (in a project: the library clip the piece came from); at the limit the handle stops, the pointer shows it and the tip says "No more footage". Pulling an end out never overwrites: an empty stretch beside it is used up first, then the later pieces move along; the same footage may be on the timeline twice. Dragging a piece up onto a row above V1 lays that stretch over the whole picture as a clip item, with its sound as an audio row; a gap stays on V1. An added song or sound is heard wherever its block lies on the timeline, with video under it or not. Projects saved by earlier versions open with their cut-out stretches as pieces and export the same.
- **Groups, and audio clips.** A piece on V1 and its own sound under it (Desktop, Microphone, Game) are a group: selected, moved, split, trimmed and deleted together. **U** (or Ungroup in the right-click menu) takes the selected pieces apart: the picture stays on V1 without its own sound, and each track that is heard there becomes an audio clip of its own, on the same row, at the same place; what is heard does not change. Each can then be moved, trimmed (its ends are handles that restore as far as the file has sound), split and deleted alone. **G** groups two or more selected things (pieces, audio clips, in any mix): a click selects them all, a drag moves them all, Delete removes them all, a split at the playhead splits each one under it, and dragging an end trims the same end of every member that ends at the same time. U breaks such a group apart. Every audio clip (an added song, an ungrouped sound) can be dragged onto any audio row, or below the last row for a new one, never onto a video row. A row can hold several clips; where they overlap, both are heard. A row's mute and level act on everything on it. In the export an audio clip is one more input: its file and the audio stream in it.
- **Speed per clip.** Select a piece and set its speed (0.25x to 4x) in the Speed section or by right-click: it gets shorter or longer on the timeline, its own sound follows with the pitch kept, and the pieces after it move along. The whole-result speed multiplies on top. Added songs, ungrouped sounds and overlay items stay at timeline speed.
- **Split, and moving a selection.** Split (S, the button, the blade) cuts only what is selected: a piece with the sound still grouped to it, an audio clip, a group, an overlay item. With nothing selected it cuts the piece under the playhead and its own sound, never an added song. An audio clip's mute and level are its own. Everything selected together moves together when one of them is dragged; Alt+Left / Alt+Right nudge the selection by a frame (Shift: ten).
- **Zoom and sharpness.** The Zoom section sets a zoom (100 to 500 %) with keyframes at the playhead; right-click the picture for "Zoom in here" and "Zoom out" (half a second each); drag on the picture to move the zoom; diamonds on V1 jump to a keyframe and drag to retime it. Color has a Sharpness slider (an unsharp mask in the export).
- **Fast opening.** A project or clip is playable as soon as it opens; sound rows, waveforms and film strips load afterwards, nearest the playhead first, and are cached. Adding clips never joins files: a project made before 0.1.38 keeps its old working file as its first source and references everything added after. Dropped files appear after a quick probe; the thumbnail and the copy (if chosen) follow in the background.
- **Exports that cannot hang silently.** A running export has a Cancel button; one that makes no new frame for 60 seconds is ended with a message and a way to see the log; every ffmpeg Glint starts ends with Glint. Clips recorded with different color settings in one project (which used to freeze ffmpeg part way) export normally.
- **Frame-exact cuts.** In an export every cut, for picture and sound together, moves to the nearest frame of its clip (half a frame at most), so no frame is shown short or skipped at a cut. A clip cut into hundreds of pieces exports: pieces of one clip share their inputs.
- **Audio rows by role.** A project shows at most Desktop, Mic, Game and Clip audio, whatever each recorder named its tracks (Glint, Medal and others in one project); a ready-made mix of a clip that also has its own tracks is left out so nothing is doubled. An edit made before 0.1.45 opens with its rows as they were and offers "Simplify the audio rows" (one undo step); "Every track" shows all rows again.
- **No clipping over 100 %.** A limiter in the preview and the export holds a level over 100 %, or several rows summed, just under full scale instead of cutting it flat.
- **Temperature, Glow and more looks.** Color has Temperature (cool to warm) and Glow (the bright parts bleed light; an export with Glow takes longer). Looks: None, Vivid, Punchy, Soft, Bright, Moody, Black and white, Dark fantasy, Dreamy, Neon night, Cold, Golden hour, Faded film.

- **Panels and timeline.** The editor's right side has tabs (Clip, Color, Overlays, Export) with the Export button pinned under them; the looks are one list (arrow keys step through them). The border between the player and the timeline drags at any number of rows, down to one row, and a double-click on it fits all rows. Dragging a clip's start or end shows the frame under the handle.
- **Reading video on the graphics card.** Settings → Capture → "Read video on the graphics card when exporting" (off by default, NVIDIA): the export's decoding runs on the card, which lowers processor load (measured: 48 % less on plain cuts, 17 % less with overlays and color) without making the export faster; any failure falls back to normal decoding.

**Backdrops.** Settings → Customise colors → Backdrop: None (the default) or one of five quiet animated backgrounds (Snowfall, Aurora, Starfield, Fireflies, Drift) in the theme's colors, behind the Library, Settings and tool pages. One canvas at 30 frames a second at most (under 10 MB); it stops while Glint is minimized, hidden or not the window in front, in the editor and clip viewer, and when Windows asks for reduced motion.

**Thumbnails.** F5 in the Library makes missing thumbnails again, Ctrl+F5 all of them (also the picture button beside "Rescan folder"; hold Ctrl for all). It runs as a job and ends with how many were made and why any failed. A thumbnail attempt is ended after 30 seconds, so one unreadable file cannot hold up the others.

**GAME mode and other monitors.** In GAME mode the game is recorded as before, and every other monitor that a clip key names keeps its own buffer; switching between GAME and DESKTOP does not restart those.

**Restarting Windows.** Glint notices when the Windows session is ending, stops its recorder and starts nothing after that, and a capture that will not start is retried slowly (2, 5, 15, then every 30 seconds) instead of at once.

**What's new and "New" tags.** The changelog is written in plain words with links to the new things. A new feature wears a small "New" tag during the first run after an update only.

**Polls from the developer.** Now and then the developer asks a short question in a small card. Answering sends only your answer and a random number made for this install. Turn it off under Settings → Updates; then Glint asks nothing. The server is in `server/polls` (one Python file and a setup script for Ubuntu); Glint reads its address from `server/polls/address.txt` in this repository, and shows no polls while that file is empty.

**Share links (optional).** Right-click a clip → Share link uploads it to your own Cloudflare account and copies a link that plays inside Discord as an embed. Off until you set it up under Settings → Share links; setup steps are in `server/share/README.md`.
- **A free-form video row.** A clip on V1 (in a project, and a single clip too) can be dragged to any time. Dropped on empty space (a gap, or the free space after the last clip) it sits exactly there, snapping to the playhead, other clips' ends, bookmarks and 0 (Alt = no snapping), and the place it came from stays empty. Dropped onto another clip it goes before or after that clip (the nearer half) and the rest moves along: nothing is overwritten. A gap is black and silent in the preview and the export, while overlay items and added audio files that play through cuts go on over it. Click a gap to select it; Delete or "Close gap" closes it, dragging its right edge makes it longer or shorter, "Close all gaps" is in its menu. "Cut out and leave a gap" (Shift+Delete) removes a selected piece without moving what follows; the ordinary cut-out still closes up. Dragging moves whole clips: a piece of a clip moves once it has been cut loose that way.
- **Blade.** The blade in the timeline's tool row (C; Esc or the Select tool ends it): a click on V1 splits the clip at that time, a click on an overlay block splits that item in two (a clip item's second half goes on where the first ends; keyframes stay with their half).
- **Slipping a song.** With an added audio file's block selected, the mouse wheel over it moves the song inside the block (0.25 s a notch, Shift 0.05 s, Ctrl 1 s): another part of the file plays at the same place.
- **Pictures, GIFs and clips on the picture.** The last button of the tool row over the picture (or dropping the file on the player; in a project also "Put on the picture" in the Library list's right-click menu) adds a PNG, JPG, WebP or BMP picture, an animated GIF or a video clip as an item on the picture. It is moved, resized, keyframed and timed like a text; a GIF loops, a clip plays from where its block starts ("Add its sound too" adds the same file as an audio row). Opacity can be set. An animated WebP shows as a still.
- **Crop.** Right-click the picture: on a picture, GIF or clip item, "Crop" lets you drag its edges and corners inward freely (Done, or Reset crop); on the clip's own picture it starts "Crop to a part of the picture" of the Canvas group. The same menu has Bring to front, Send to back, Duplicate and Delete.
- **Round blur.** A blur box has "Round corners" (up to a full ellipse) and "Soft edge" (the blur fades out toward the edge), in the preview and in the export.
- **Fullscreen** (the button beside Done, or `F`) puts the whole editor on the full screen. Esc leaves it.
- **Shortcuts.** The keyboard button lists every editor shortcut. Click one and press the new keys to change it; a key taken from another command is named. Reset one or all.
- **Zoom** the timeline with Ctrl + wheel, `+` / `-`, or Fit.
- **Export**, or **Export and render with Blur**. The preview plays what the export will produce: cuts are skipped and levels applied.

One kept range with nothing else changed is cut losslessly in an instant. Anything else re-encodes on the hardware encoder (falling back to x264). Changing audio produces one mixed track; otherwise all tracks are kept. The edit model is `ui/editmodel.js`; the export receives exactly what `emToOpts` builds.

Shortcuts in edit mode (also under the Shortcuts button): Space play/pause; S split; Delete cut out or restore the selected piece; M mute it; Up / Down its level; I / O trim; [ / ] previous / next cut; Left / Right 5 s (Shift 1 s); , / . one frame; + / - zoom; Shift+F fit; Ctrl+Z undo; Ctrl+Y redo; Ctrl+E export; Esc leave.

## Renders

A Blur render is a job. Jobs run in the background whether or not the clip is open: a button appears in the top bar while something is rendering (Settings → Appearance → Renders button can show it always or never), showing overall progress; its list shows each render with progress, Cancel, and Open for finished ones. "At once" (1 to 4) sets how many run together; the rest wait in order. Quitting Glint stops running renders.

## Import

Library → Import brings in clips recorded by NVIDIA ShadowPlay / NVIDIA app, Medal, Outplayed and SteelSeries GG Moments, or from any folder. Glint finds each program's clips folder, lists what is there grouped by game (from the folder layout, the file name, or Medal's own index), and adds what you tick. By default files stay where they are and are only listed; "Copy them into Glint's folder" copies them (into game folders if that setting is on). The originals are never moved, changed or deleted.

**Where imported clips go.** The import dialog asks: a folder for each game, one folder you name (the source's name by default), or no folder. Clips left where they are (not copied) are filed in a library folder by name only: Glint never moves, renames or changes another program's files, also not with "Sort by game" or when you move the clip to another folder.

**Logos in the import dialog.** Each program has a small mark in front of its name. NVIDIA's and SteelSeries' come from the Simple Icons set (CC0). Medal's is Medal's own icon from [medal.tv/brand](https://medal.tv/brand), used as that page allows: unchanged, white on a dark window and black on a light one (`ui/logos`). Outplayed has no published logo to use, so it has a neutral sign. All of these marks are trademarks of their owners; Glint is not sponsored or endorsed by any of them, and the files in `ui/logos` are not part of Glint's own code.

## Compress for Discord

Right-click a clip → Compress for Discord → 10, 20, 50 or 500 MB (the size used last is listed first). Glint re-encodes a copy with ffmpeg (x264, two passes, so the size is hit rather than guessed), shrinking the picture and frame rate only as far as the size needs: a short clip keeps 1440p60, a long one may come down to 720p30. The copy is saved beside the original as "… (Discord 10MB).mp4", tagged Discord in the library, and put on the clipboard ready to paste. It runs in the background queue with the Blur renders, one compression at a time; a clip already under the size is refused, and one too long for the size is refused with the length that would fit.

**Compress page.** Tools → Compress does the same for several clips or any video file: pick them, pick a size (a preset or your own), see the resolution, frame rate and bitrate each one will get, start, and copy or open the results. Its settings: the default size, the sizes in the right-click menu, speed, audio bitrate, highest resolution and frame rate, which audio tracks to keep, where results go, and whether the result is copied to the clipboard.

## Interface sounds, What's new

Settings → Interface sounds switches on soft sounds for clicks, switches, menus and results (off by default; a separate switch for the faint tick when the pointer moves over things, a volume slider, and a button to hear each sound). They are made in code, not audio files. They come out of the speakers like any other sound, so they are recorded into the replay buffer.

Clicking the logo, or Settings → Updates → What's new, lists what each release added, improved and fixed. It opens by itself once after an update.

## Reporting a bug

The bug button in the top bar opens a dialog: what happened, optionally how to reach you, and whether to include the end of Glint's log. "What will be sent" shows everything first: your text, a few lines about this PC and Glint (version, Windows build, graphics card, capture setup) and the log's last 150 lines, with your Windows user folder replaced by `~`. Nothing is sent until you press Send; it then goes to the developers' Discord. One report per 30 seconds.

For builders: the destination is not in the source. `src-tauri/bug-webhook.txt` (git-ignored; line 1 a Discord webhook URL, line 2 comma-separated user ids to mention) is read by `build.rs` and stored obfuscated in the program. Without the file the build works and the dialog points to GitHub issues instead. Obfuscation is not secrecy: anyone determined can recover a webhook from a program that uses it.

**Feedback.** The heart button beside it asks "How do you like Glint?": one to five stars, a text and, if you want, how to reach you. Like a bug report it shows exactly what will be sent, nothing leaves the PC until you press Send, and it goes to the same place as the bug reports (no log file, only the Glint version and the language).

## Recommended settings, interface size, language

- Capture settings that suit this PC carry a blue outline and a "Recommended" tag; others offer "Use recommended". The graphics card is read from the registry. Choosing an encoder (or Blur GPU type) made for another brand of card, or the CPU encoder, asks for confirmation first.
- Settings → Appearance → Interface size scales the whole window (70 % to 150 %).
- The search box at the top right of Settings filters the page as you type. It also finds the Capture page's settings (bitrate, encoder, replay buffer and so on), which can be changed right there.
- Settings → Language switches language (English, Korean, Greek). Texts are looked up by their English wording in `ui/lang/<code>-*.js`; a text without a translation shows in English. `node tools/i18n-check.js ui ko` (or `el`) lists texts that still lack a translation.
- Custom theme (CSS) is a fold-out box in Appearance. "Copy AI prompt" copies a prompt that explains Glint's theme variables to an AI, so it can write a theme from a one-line description.

**Apply recommended settings.** The button at the top of Capture works out encoder, capture method, resolution, frame rate, quality and buffer mode from this PC's graphics card, screen, memory and Windows version, lists every change with its reason, and applies them only after you agree. Results can be inconsistent: treat them as a start and tweak to your liking. Every choice that is on Auto shows what it is using, such as "Auto (NVIDIA NVENC)".

## Blur

The Blur page starts with a live preview, which can be folded away with its Hide button (it is then not drawn at all): the same moving shapes drawn as the original and as Blur would render them with the current settings, following Blur's own maths (frames blended per output frame, weighting, gamma, interpolation, filters). It updates while a control is dragged, and a line under it says in plain numbers what will happen, including when a combination would do nothing. Quick presets set the common fields; less-used settings are in collapsible groups.

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
glint.exe --capmode-test report.txt
glint.exe --monprev-test report.txt
glint.exe --speed-test report.txt
powershell -File tools\e2e.ps1 -Exe glint.exe
glint.exe --audio-test report.txt
glint.exe --blur-test in.mp4 out.mp4 report.txt
```

`--hotkey-test` injects `F13` combinations and checks binding resolution. `--bench` runs the real recorder, reports CPU, RAM, GPU encoder load and delivered frame rate, then saves a clip and verifies it with `ffprobe`; `--bitrate 50` sets a fixed rate and `--buffer ram|mixed|disk` (with `--ram-secs N`) chooses where the buffer lives. `--buffer-test` feeds the video and audio rings made-up data on a made-up clock and checks that clips cut across RAM and disk are byte-exact, that files are deleted as the buffer moves on, and that an unwritable folder falls back to RAM. `--folders-test` moves, renames and deletes library folders on scratch files and refuses to run unless `GLINT_DATA_DIR` is set. `--capmode-test` checks the GAME / DESKTOP mode on made-up window lists and made-up audio (game detection, the capture arguments, waiting for a game, the tracks of a game-mode clip); it captures nothing. `--audio-test` lists the audio devices and checks how a device is chosen by name; it records nothing. `tools\e2e.ps1` is the editor's end-to-end test: `tools/e2e-scenarios.js` plays edits (split, remove, trim and restore, move, gaps, speed, a tail, an old project) through the real edit model under node and writes what each must export; `glint.exe --e2e-test` then exports every one and checks the length, which second of the source is on screen at sample times, and where the sound is heard. Glint itself never starts node. `--blur-test` renders one file through the Blur wrapper. Set `GLINT_DATA_DIR` to keep test output out of your real library.

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
  gif.rs        export a clip as a GIF (palette filters, size limit)
  upscale.rs    upscale a clip for YouTube (nearest neighbour or Lanczos, hardware encoders)
  rules.rs      render rules: a chain of steps (color, Blur, upscale, compress, GIF, copy) as one job
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
