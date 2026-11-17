# GAME mode detection on real games: laptop session report

Branch `gamedetect-laptop`, from `v0.1.44`. Nothing was pushed to `main`; the version, the changelog and the README are unchanged.

## The test PC

- Lenovo laptop, AMD Radeon 840M (integrated only), one 1920×1200 60 Hz panel at **150 % display scaling**.
- Windows 11 build 26200. Windows' "Screenshot borders" setting lets apps hide the border, so capture method Auto resolves to Graphics Capture (`wgc`) here.
- ffmpeg 9.0.1 (Gyan essentials, with `gfxcapture` and `ddagrab`). Encoder for this PC: AMF.
- Rust stable GNU toolchain plus WinLibs MinGW-w64 (the build needs its `dlltool.exe`).

## The tool: `glint.exe --detect-watch <folder>`

`src-tauri/src/detectwatch.rs`. A headless `Core` in GAME mode runs the app's own upkeep thread (`Core::watch`: the two-second look, `capmode::found`, `gamedetect::decide`, `capmode::follow`, the game's sound, the restart on a display-mode change). It exercises the code users run, not a copy.

- **Every look:** one JSON line in `looks.jsonl` with the window in front, the decision and its reason, every candidate with its evidence and points, the failed and exclusive marks, the shell's D3D-fullscreen flag, and what ffmpeg really captures: `RecStatus::source` (new: the capture filter ffmpeg was started with), its frames and its last error.
- **Every change:** the full window list goes into `watch.log`, then a real clip is saved through `Core::clip_blocking`, the clip key's path. Window titles stay in the output folder; keep that folder out of the repository.
- **Each clip is checked:**
  - frames and distinct frames;
  - four PNG frames with their mean color;
  - a picture of the monitor taken at the same moment, showing what the player saw;
  - each sound track's level, also in a narrow band at 1 kHz.
- **Options:**
  - `--pick <exe or path>`: a hand pick, through the menu's own `apply_mark`.
  - `--method`, `--encoder`.
  - `--desktop-audio`: adds a Desktop track as the reference for the Game track.
  - `--every N`, `--settle N`, `--clip-secs N`, `--secs N`.
- Refuses to run without `GLINT_DATA_DIR`.
- Outside the files named for this task, only `main.rs` changed: the module line and the six-line `--detect-watch` dispatch, beside the other self-test flags.

## Matrix

- A row passes only where I looked at frames of the recording.
- "Only the game's sound" is proven only where another program's sound was playing and measured.
- Build "0.1.44" is the unchanged release, "fix 1" is commit `6be19b0`, other rows name the commit they ran on. "head" is `f01a19c`, this branch's last code commit (later commits only change this report).

| Game | Store | Window mode | Build | Glint chose | Source | Reason (log) | Frames show the game | Only the game's sound |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Terraria | Steam | borderless 1920×1200 | 0.1.44 | Terraria | Window | library:steam, fullscreen | yes, also while a browser was in front | not proven |
| Terraria | Steam | "Fullscreen 1280×800" that stayed a window in the middle of the desktop | 0.1.44 | Terraria | MonitorDda | "runs in exclusive fullscreen" (**wrong**) | **no**: the whole desktop, the game small in the middle; nothing at all while the browser was in front | not proven |
| Terraria (hand-picked) | Steam | windowed 800×600, in front and behind another window | fix 1 | Terraria | Window | your pick | yes, both | — |
| Terraria (hand-picked) | Steam | minimized | fix 1 | Terraria | Window | minimized, keeps the last picture | yes (the last picture) | — |
| Terraria (hand-picked) | Steam | exclusive 1280×800, then exclusive 1920×1200 | fix 1 | Terraria | MonitorDda | exclusive fullscreen | yes | Game track silent exactly while Terraria's own volume was 0, sound otherwise |
| Terraria | Steam | borderless 1280×800 window in the middle of the desktop, another window over part of it | `4886016` | Terraria | Window | library:steam | **yes**, only the game, whole | — |
| Terraria | Steam | exclusive 1280×800, then "Go Windowed" (1280×800 in the middle) | `4886016` | Terraria | MonitorDda, then Window | "no longer in exclusive fullscreen: it no longer covers its monitor" | yes, also with another window in front | — |
| Terraria | Steam | exclusive 1920×1200, then "Go Windowed" with Borderless on (covers the monitor) | head | Terraria | MonitorDda, then Window | "no longer in exclusive fullscreen: it is in front and the shell does not say D3D fullscreen" | yes, also with another window in front | — |
| Terraria, method set to Desktop Duplication by hand | Steam | 1280×800 window in the middle | `4886016` | Terraria | MonitorDda | "window capture is not available on this PC …: the capture method is set to Desktop Duplication, which records whole monitors" | whole monitor, other windows included (the user's choice, now said so) | — |
| tModLoader (runs as `dotnet.exe`) | Steam | windowed 800×720 | 0.1.44 | tModLoader | Monitor | "window gave no picture" (**wrong**) | **no**: the whole screen (Steam and other windows), the game behind them | — |
| tModLoader (hand-picked) | Steam | windowed 800×720, minimized, in front and behind | `1b4767c` | tModLoader | Window | your pick | **yes**, only the game, also while another window covered the whole screen | **yes**: a 1 kHz tone from another program at −24 dB in the Desktop reference track, absent from the Game track (−50 dB, the same as without it) |
| tModLoader | Steam | windowed 800×720 from launch (the AMF stall during its loading), then its own Settings → Video → Go Fullscreen (a borderless 1920×1200 window, no D3D flag), then another program in focus | head | tModLoader | Window throughout | library:steam, seen | **yes**, only the game, also while another program had focus; `glint.log` has the AMF step-down line and no "gave no picture" after it | — |
| Minecraft Java (Prism Launcher, its own Java runtime) | — | 854×480 window, then grown to fill the screen | head | Minecraft (`javaw.exe`, by its GLFW window and title) | Window | minecraft | yes, only the game, while another program had focus | — |
| Wallpaper Engine (negative case) | Steam | wallpaper running, its UI minimized | `2da396c` (detection as `1b4767c`; it was no longer running for a check at head) | nothing | Wait | waiting for a game (`wallpaperui.exe` is denied; the wallpaper itself is no top-level window) | n/a | n/a |

Also checked:

- **`--live-check` at head**: its window capture, the window that keeps itself out of capture (black, as it should be), the monitor by handle and Desktop Duplication all give the right picture.

- **The real app**, not the tool, with a test data folder. Its log measures windows in physical pixels at 150 % ("1920x1200 on monitor 1 of 1920x1200" for a borderless Terraria, not 1280×800). It reads the method set to Desktop Duplication and gives the new reason.
- **The UI texts in English, Korean and Greek**: the return values of `capWhyText`, `capWhyDetail` and `capStatusText` in a browser preview of `ui/` (`tools/serve.js`, the mock backend). Access to the real app's window was denied, so the texts were not seen inside the app itself.
- **The yellow border.** With the border forced on (`display_border=1`), Windows drew it around Terraria's window on the screen. The 224 frames captured from that window have no yellow at any edge. The border is on the screen only and never in the clip.

## Bugs found and fixed

1. **A mode switch that only flashed exclusive fullscreen kept the window "exclusive" for life.**
   - Seen: on a 150 % display, Terraria's fullscreen switch made the shell report D3D fullscreen for two looks while the window still covered the monitor. Afterwards the game was a window in the middle of the desktop.
   - Cause: `Memory::exclusive` was cleared only when the window disappeared.
   - Effect: GAME mode recorded the whole monitor by Desktop Duplication, and nothing while another window was in front.
   - Fix (`gamedetect::decide`): a window is exclusive only while it covers its monitor or is minimized. Commit `6be19b0`.
2. **After exclusive fullscreen, "Go Windowed" to a maximized or borderless window stayed "exclusive".**
   - Cause: those windows cover the monitor too.
   - Evidence: while Terraria was really exclusive and in front, the shell said D3D fullscreen on all 88 looks; after "Go Windowed", on none. That is one D3D9 game on one PC.
   - Fix: a window in front, not minimized, without the shell's flag is not exclusive. A single odd reading is absorbed by the 4 s hold. Commit `4648327`.
   - Limit: the rule needs the game in front. A player who goes to a borderless window and alt-tabs away at once stays paused until the game is in front again.
3. **An encoder restart failed the game's window for life (the user's symptom).**
   - Seen: AMF on the Radeon refused the capture's GPU frames. The recorder restarted ffmpeg twice, then sent frames through system memory, about 18 s after the start while tModLoader loaded.
   - Cause: GAME mode's "no first frame in 12 s" wait counted across those ffmpeg runs, and ran out 1.4 s after the encoder was fixed. The window was then marked failed until it closed.
   - Effect: the whole monitor was recorded. The clip shows Steam and other windows, with the game behind them.
   - Fixes: the wait is per ffmpeg run (`RecStatus::pid`). A failed window gets another chance once it changes mode (its size, or covering its monitor). Commit `1b4767c`.
4. **A failed window capture was reported as "window" for 4 s while its monitor was recorded.**
   - Cause: `cfgs` left the failed window at once, but `decide` held the old source and reason.
   - Fix: leaving a failed window capture is no longer held. Commit `4886016`.
5. **GAME mode recorded the whole monitor whenever "auto" picked Desktop Duplication for monitors** (Windows 10, or Windows' Screenshot-borders setting off). The owner's decision: GAME mode now captures the game's window by Graphics Capture whenever ffmpeg has `gfxcapture`. Only Desktop Duplication chosen by hand, or an ffmpeg without `gfxcapture`, leaves GAME mode without window capture, and the reason says so. Commit `f01a19c`.
   - **The cost:** on Windows 10, and on Windows 11 with the Screenshot-borders setting off, every GAME-mode player now sees Windows' yellow border around the game's window on the screen. It is not in the clip: with the border forced on here, Windows drew it on the screen and none of the 224 captured frames had it. The README's Windows 10 paragraph says the opposite and needs changing (see "For the merge").
   - **Check before release** (the owner's main PC): turn "Let desktop apps turn off the screenshot border" off, run `glint.exe --detect-watch <folder>` with a game running, and confirm that `looks.jsonl` shows a source `gfxcapture=hwnd=…` with frames, that ffmpeg only warns about the border, and that the clips show the game.

Each fix has checks in `--capmode-test`, built from the recorded window states.

## Readable failures

Commits `4886016` and `f01a19c`.

- **`Why::detail` says why the whole monitor is recorded instead of the window.**
  - For "window capture is not available": the method set to Desktop Duplication, an ffmpeg without `gfxcapture`, or no ffmpeg.
  - For "gave no picture": either one capture run delivered nothing for 12 s, or ffmpeg refused the window.
  - It appears in the log line, the switch's tooltip, the status bar and the menu, with a remedy.
  - Korean and Greek are in `ui/lang/ko-gd2.js` and `ui/lang/el-gd2.js`; `i18n-check` reports CONFLICT 0 for both.
- **Each decision change in `glint.log` is one self-contained line**, because a bug report carries only the last 150 lines. The line gives:
  - the window's exe, its size, its monitor and that monitor's mode;
  - whether it covers the monitor, is minimized, is in front (never its title);
  - how it is recorded and the shell's D3D-fullscreen state;
  - the method setting, what it resolves to for monitors and why;
  - the encoder.

  Example from this PC:
  `game mode: Terraria runs in exclusive fullscreen; recording its monitor 1 by Desktop Duplication instead [library:steam, seen, fullscreen, front] | window 0x107b6 (terraria.exe, 1920x1200 on monitor 1 of 1920x1200, covers it, in front); recorded by its monitor (Desktop Duplication); shell D3D fullscreen yes; method "auto" -> wgc, encoder amf`
- **Every exclusive or failed mark set or cleared is logged once with its rule**, for example `… is no longer in exclusive fullscreen: it is in front and the shell does not say D3D fullscreen`. A failure also logs ffmpeg's last error and the encoder.

### Signatures to look for in a user's 0.1.44 log

- **Bug 3:** `capture 0: the amf encoder delivered no frames from the GPU` (or nvenc / qsv), followed within seconds by `game mode: window 0x… gave no picture in 12 s` and `recording its monitor … instead`.
- **Bugs 1 and 2:** `runs in exclusive fullscreen; recording its monitor … by Desktop Duplication` while the user says the game was windowed, often followed by `is not in front; its monitor is not recorded meanwhile`.
- **Bug 5:** `window capture is not available on this PC`. In 0.1.44 the cause is not in the log. It is one of: the method set to Desktop Duplication, Windows 10, Windows' Screenshot-borders setting, or an ffmpeg without `gfxcapture`. Windows 10 and the border setting are fixed by bug 5.

The tModLoader path reproduces the user's words. It is not shown to be that user's path: their Windows version and GPU are unknown.

## Test counts (final build)

| Self-test | Before | Now |
| --- | --- | --- |
| `--capmode-test` | 163, 0 failed | 174, 0 failed |
| `--gameaudio-test` | 58, 0 failed | 58, 0 failed |
| `--monprev-test` | 10, 0 failed | 10, 0 failed |
| `--buffer-test` | (not run) | 68, 0 failed |

One existing scenario changed its expectation (bug 4): a fullscreen window whose capture gave no picture now goes to Desktop Duplication at once, instead of after the 4 s hold.

## Other observations

- **AMF stalls on every capture start on this PC.** The first one or two ffmpeg runs fail on GPU frames, and 4 to 18 s pass before frames come through system memory. The existing fallback works, but the start of each capture is lost.
- **About 4 s of the whole desktop is recorded while leaving exclusive fullscreen.** The source change waits the hold while the capture is still Desktop Duplication of a monitor that now shows the desktop.
- **Window capture keeps running while Windows is locked.** Frames kept coming.
- **The library index is empty on the very first look after a start**, because it loads in the background. The real app's first decision for Terraria gave `[folder, fullscreen]` instead of `library:steam`.
- **The Capture settings page (`ui/capture2.js`, not changed: outside the files of this task)** still suggests "choose Desktop Duplication here" when Windows does not hide the border. In GAME mode that choice now means no window capture. Worth a GAME-mode wording.
- **Terraria mutes itself when it is not in front** (its own setting), so its Game track is silent then. Not a Glint issue.
- **The GNU build prints `ld.exe: .rsrc merge failure: multiple non-default manifests`.** The built app is still per-monitor DPI aware: see the physical-pixel log line above.

## For the merge

- **README, now stale:** the "Windows 10" paragraph (Auto uses Desktop Duplication there, so no border: GAME mode now uses window capture there) and the Game or Desktop section's fallback sentence (the whole monitor is now recorded only when Desktop Duplication is chosen by hand or ffmpeg lacks `gfxcapture`, or the window gives no picture).
- **Changelog:** bugs 1 to 5 and the readable reasons.
- **`ui/capture2.js`:** its "choose Desktop Duplication here" border hint needs a GAME-mode wording.
- **The ~4 s of desktop** recorded while leaving exclusive fullscreen (see "Other observations"), not changed.
- **Not committed, on purpose:** frames and clips of this session (they show private windows) stay on the laptop.

## What I could not test

- **Hybrid graphics:** this laptop has no dedicated GPU.
- **A second monitor:** none was attached.
- **Epic with Easy Anti-Cheat, Riot (Vanguard), an old DX9-only game other than Terraria:** not installed. The owner judged them too big.
- **Albion Online** (Xbox app, BattlEye twin `Albion-Online_BE.exe`): installed, but it needs the owner's sign-in and play. Skipped.
- **Overwatch** (Battle.net): installed, but Battle.net is not. Skipped.
- **Roblox, a Microsoft Store game:** not installed.
- **Bug 5 under its real conditions** (Windows 10, or the Screenshot-borders setting off): this PC is neither, and I did not change a Windows privacy setting. It is covered by a unit test plus the window-capture rows above.
- **The second-chance rule of bug 3** (a failed window retried after a mode change): covered by a unit test only. With the per-run wait, no window failed again live.
