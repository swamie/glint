# GAME mode detection on real games: laptop session report

Branch `gamedetect-laptop`, from `v0.1.44`. Status: **draft, updated as rows land.**

## The test PC

- Lenovo laptop, AMD Radeon 840M (integrated only), one 1920×1200 60 Hz panel at **150 % display scaling**.
- Windows 11 build 26200. Windows' "Screenshot borders" setting allows apps to hide the border, so capture method Auto resolves to Graphics Capture (`wgc`).
- ffmpeg 9.0.1 (Gyan essentials, has `gfxcapture` and `ddagrab`). Encoder for this PC: AMF.
- Rust stable GNU toolchain plus WinLibs MinGW-w64 (the build needs its `dlltool.exe`).

## The tool: `glint.exe --detect-watch <folder>`

`src-tauri/src/detectwatch.rs`. It runs a headless `Core` in GAME mode with the app's own upkeep thread (`Core::watch`: the two-second look, `capmode::found`, `gamedetect::decide`, `capmode::follow`, the game's sound, the restart on a display-mode change), so it exercises the code users run, not a copy.

- Every look: one JSON line in `looks.jsonl` with the window in front, the decision and its reason, every candidate with its evidence and points, the failed and exclusive marks, the shell's D3D-fullscreen flag, and what ffmpeg really captures (`RecStatus::source`, new: the capture filter ffmpeg was started with), its frames and its last error.
- Every change: the full window list into `watch.log` (titles stay in the output folder; keep it out of the repository), then a real clip through `Core::clip_blocking` (the clip key's path).
- Each clip is checked: frames and distinct frames, four PNG frames with their mean color, a picture of the monitor taken at the same moment (to compare with what the user saw), and each sound track's level, also in a narrow band at 1 kHz.
- Options: `--pick <exe or path>` (through the menu's own `apply_mark`), `--method`, `--encoder`, `--desktop-audio` (a Desktop track as the reference for the Game track), `--every N`, `--settle N`, `--clip-secs N`, `--secs N`. Refuses to run without `GLINT_DATA_DIR`.

## Matrix

A row passes only where I looked at frames of the recording. "Only game sound" is proven only where a sound of another program was playing and measured.

| Game | Store | Window mode | Build | Glint chose | Source | Reason (log) | Frames show the game | Only the game's sound |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Terraria | Steam | borderless 1920×1200 | 0.1.44 | Terraria | Window | library:steam, fullscreen | yes, also while a browser was in front | not proven |
| Terraria | Steam | "Fullscreen 1280×800", stayed a window in the middle of the desktop | 0.1.44 | Terraria | MonitorDda | "runs in exclusive fullscreen" (wrong) | **no**: the whole desktop with the game small in the middle; nothing at all while the browser was in front | not proven |
| Terraria (hand-picked) | Steam | windowed 800×600, in front / behind another window | fix 1 | Terraria | Window | your pick | yes, both | — |
| Terraria (hand-picked) | Steam | minimized | fix 1 | Terraria | Window | minimized, keeps the last picture | yes (last picture) | — |
| Terraria (hand-picked) | Steam | exclusive 1280×800, then exclusive 1920×1200 | fix 1 | Terraria | MonitorDda | exclusive fullscreen | yes | Game track silent exactly while Terraria's own volume was 0, sound otherwise |
| Terraria (hand-picked) | Steam | exclusive, then "Go Windowed" (maximized 1920×1170) and borderless | fix 1 | Terraria | MonitorDda | still "exclusive" (wrong, fixed by fix 2) | yes (the window covered the monitor), paused while away | — |
| Terraria | Steam | borderless 1280×800 window in the middle of the desktop, another window over it | final | Terraria | Window | library:steam | **yes**, only the game, whole | — |
| Terraria | Steam | exclusive 1280×800, then "Go Windowed" (1280×800 in the middle) | final | Terraria | MonitorDda, then Window | "no longer covers its monitor" | yes | — |
| Terraria, `--method dda` | Steam | 1280×800 window in the middle | final | Terraria | MonitorDda | "window capture is not available …: the capture method is set to Desktop Duplication, which records whole monitors" | **no, by design**: the whole monitor, other windows included (see open decision) | — |
| tModLoader (runs as `dotnet.exe`) | Steam | windowed 800×720 | 0.1.44 | tModLoader | Monitor | "window gave no picture" (wrong) | **no**: the whole screen (Steam, other windows), the game behind them | — |
| tModLoader (hand-picked) | Steam | windowed 800×720, minimized, in front / behind | final | tModLoader | Window | your pick | **yes**, only the game, also while another window covered the whole screen | **yes**: a 1 kHz tone from another program at −24 dB in the Desktop reference track, absent from the Game track (−50 dB, as without it) |
| Wallpaper Engine (negative) | Steam | wallpaper running, its UI minimized | final | nothing | Wait | waiting for a game (`wallpaperui.exe` denied; the wallpaper is no top-level window) | n/a | n/a |

Not tested yet: listed under "What I could not test".

## Bugs found and fixed

1. **A mode switch that only flashed exclusive fullscreen kept the window "exclusive" for life.** On a 150 % display, Terraria's fullscreen switch made the shell report D3D fullscreen for two looks while the window still covered the monitor; afterwards the game was a window in the middle of the desktop. `Memory::exclusive` was cleared only when the window disappeared, so GAME mode recorded the whole monitor by Desktop Duplication, and nothing while another window was in front. Fix (`gamedetect::decide`): a window is exclusive only while it covers its monitor or is minimized. Commit `6be19b0`.
2. **After exclusive fullscreen, "Go Windowed" to a maximized or borderless window stayed "exclusive".** Those windows cover the monitor too. While Terraria was really exclusive and in front the shell said D3D fullscreen on all 88 looks, after "Go Windowed" on none (one D3D9 game on one PC). Fix: a window in front, not minimized, without the shell's flag is not exclusive; a single odd reading is absorbed by the 4 s hold. Commit `4648327`.
3. **An encoder restart failed the game's window for life (the user's symptom).** AMF on the Radeon refused the capture's GPU frames; the recorder restarted ffmpeg twice and then sent frames through system memory, about 18 s after the start while tModLoader loaded. GAME mode's "no first frame in 12 s" wait counted across those runs and ran out 1.4 s after the encoder was fixed. The window was marked failed until it closed and the whole monitor was recorded: the clip shows Steam and other windows, with the game behind them. Fixes: the wait is per ffmpeg run (`RecStatus::pid`); a failed window gets another chance once it changes mode (size, or covering its monitor). Commit `1b4767c`.
4. **A failed window capture was reported as "window" for 4 s while its monitor was recorded.** `cfgs` left the failed window at once, `decide` held the old source and reason. Leaving a failed window capture is no longer held. Commit `4886016`.

Self-test additions: each fix has checks in `--capmode-test` built from the recorded window states.

## Readable failures

Commit `4886016`.

- `Why::detail` says why the whole monitor is recorded instead of the window: for "window capture is not available" the method set to Desktop Duplication, Windows' Screenshot-borders setting (Auto then takes Desktop Duplication), Windows 10, an ffmpeg without `gfxcapture`, or no ffmpeg; for "gave no picture" whether one capture run delivered nothing for 12 s or ffmpeg refused the window. The log line, the switch's tooltip, the status bar and the menu say it. Korean and Greek in `ui/lang/ko-gd2.js` and `ui/lang/el-gd2.js` (`i18n-check`: CONFLICT 0 for both).
- Each decision change in `glint.log` is one self-contained line (a bug report carries only the last 150 lines): the window's exe, size, monitor and that monitor's mode, whether it covers it, is minimized, is in front (never its title); how it is recorded; the shell's D3D-fullscreen state; the method setting, what it resolves to and why; the encoder. Example from this PC:
  `game mode: window capture is not available on this PC; recording Terraria's monitor 1 [library:steam, seen]: the capture method is set to Desktop Duplication, which records whole monitors | window 0x107b6 (terraria.exe, 1280x800 on monitor 1 of 1920x1200, in front); recorded by its monitor (Desktop Duplication); shell D3D fullscreen no; method "dda" -> dda (…); encoder …`
- Every exclusive or failed mark set or cleared is logged once with its rule; a failure carries ffmpeg's last error and the encoder.

### Signatures to look for in a user's 0.1.44 log

- Bug 3: `capture 0: the amf encoder delivered no frames from the GPU` (or nvenc/qsv) followed within seconds by `game mode: window 0x… gave no picture in 12 s` and `recording its monitor … instead`.
- Bugs 1 and 2: `runs in exclusive fullscreen; recording its monitor … by Desktop Duplication` while the user says the game was windowed, often followed by `is not in front; its monitor is not recorded meanwhile`.
- The method path (not a bug, now explained): `window capture is not available on this PC`. In 0.1.44 the cause is not in the log; it is one of: the method set to Desktop Duplication, Windows 10, Windows' Screenshot-borders setting, or an ffmpeg without `gfxcapture`.

The tModLoader path reproduces the user's words; it is not shown to be that user's path. Their Windows version and GPU (in the bug report header) would tell.

## Test counts (final build)

| Self-test | Before | Now |
| --- | --- | --- |
| `--capmode-test` | 163, 0 failed | 173, 0 failed |
| `--gameaudio-test` | 58, 0 failed | 58, 0 failed |
| `--monprev-test` | 10, 0 failed | 10, 0 failed |
| `--buffer-test` | (not run) | 68, 0 failed |

One existing scenario changed its expectation: a fullscreen window whose capture gave no picture now goes to Desktop Duplication at once instead of after the 4 s hold (bug 4).

## Other observations

- **AMF stalls on every start on this PC**: the first one or two ffmpeg runs fail on GPU frames, 4 to 18 s pass before frames come through system memory. The existing fallback works; the start of each capture is lost.
- **About 4 s of the whole desktop** is recorded while leaving exclusive fullscreen (the source change waits the hold while the capture is already Desktop Duplication of a monitor that now shows the desktop).
- **Window capture keeps running while Windows is locked** (frames kept coming).
- The library index is empty on the very first look after start (it loads in the background), so the first look lacks library evidence.
- The GNU build prints `ld.exe: .rsrc merge failure: multiple non-default manifests`.
- Terraria mutes itself when it is not in front (its own setting), so its Game track is silent then; not a Glint issue.

## Open decision for the owner

The Desktop Duplication path (Windows 10, Windows' Screenshot-borders setting, or the method set by hand) records the game's whole monitor for a windowed game: the desktop and other windows around and over it. It is stated now, but it is still the user's symptom for everyone on that path. Options: keep it with a remedy in the text; use window capture in GAME mode whenever ffmpeg has `gfxcapture` (the yellow border may show); or crop the monitor capture to the game window's rectangle (other windows over the game would still be recorded).

## What I could not test

- **Hybrid graphics**: this laptop has no dedicated GPU.
- **A second monitor**: none attached (pending).
- **Epic with Easy Anti-Cheat, Riot (Vanguard), an old DX9-only title other than Terraria**: not installed; the owner judged them too big.
- **Albion Online** (Xbox app, BattlEye twin `Albion-Online_BE.exe`): installed, needs the owner's sign-in and play; skipped this session.
- **Roblox, a Microsoft Store game**: not installed.
- **Minecraft Java** (`javaw.exe` via Prism Launcher): pending.
- **tModLoader in fullscreen on the final build**: my injected Alt+Enter did not switch it.
- The second-chance rule of bug 3 (a failed window retried after a mode change) is covered by a unit test only; with the per-run wait no window failed again live.
