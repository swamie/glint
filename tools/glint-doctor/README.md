# Glint Doctor (the tool)

Optional, and separate from Glint: nothing in `src-tauri` or `ui` knows about
it. A small console program a Glint user runs once, when the developer has
given them a support code. It collects diagnostics, takes the personal data
out, shows what it will send, and sends it to the developer's Worker
(`server/doctor`). After one successful run it does not work again.

The other half, and how codes are made, is in `server/doctor/README.md`.

## What the user does

1. Gets `glint-doctor-ABCD-EFGH-JKMN.exe` from the developer (the code is in
   the file name) and double-clicks it. Windows SmartScreen may ask, as it does
   for Glint: More info, Run anyway.
2. Types a sentence about the problem, or presses Enter.
3. Presses Enter to send, V to read everything first in Notepad, or Q to quit.
4. Sees "Sent. Thank you!" and, a minute or two later, a short summary.

If the file was renamed, the tool asks for the code instead.

## What it reads

Read only. The one thing it writes is `doctor-used.json` in Glint's data folder.

| Section | From |
| --- | --- |
| Glint version, install folder contents | the uninstall entry GlintSetup.exe writes (HKCU), `update.json` |
| Windows version, graphics cards with driver version and date, CPU, memory | the registry (the same values Glint's bug report uses), `GlobalMemoryStatusEx` |
| WebView2 version | the registry |
| Free disk space | for the data folder, the clips folder and the buffer folder |
| Settings | `%APPDATA%\Glint\settings.json` |
| ffmpeg | where Glint would find it, `-version`, which hardware encoders it has |
| Running programs | only: is Glint running, is ffmpeg, which other recorders (OBS, Medal, ...) |
| Crashes | "Application Error" and "Application Hang" events of the last 30 days that name Glint, from the Windows event log; names of crash dump files |
| Data folder | file names and sizes; how many clips, projects and games (never their names) |
| Log | lines of `glint.log` that look like faults (each once, counted) and its last 700 lines |

## What is taken out first

`src/scrub.rs`, with tests. The user's profile folder becomes `~` (the same
rule as Glint's bug report), and so does the Windows user name as a word. Also
removed: other users' folder names, the PC's name, network share hosts, clip
and picture file names (the folders and the file type stay), e-mail addresses,
IP addresses, Discord webhooks, the query of every web address and any host
that is not one of Glint's own, and anything that looks like a key or token.
In the settings, values are dropped by the setting's name (token, key,
webhook, anything starting with `share`), web addresses are dropped, and
custom CSS, presets and folder colors are reduced to a count.

The Worker runs a second pass before storing anything.

## Building and shipping

    cd tools/glint-doctor
    # once: put the Worker's address in endpoint.txt
    cargo test
    cargo build --release
    # -> target\release\glint-doctor.exe: one small exe, no installer, no DLLs

It is its own Cargo project with an empty `[workspace]`, so building Glint
never builds it and the other way round. Dependencies: `serde_json`, `regex`,
`winreg`. Sending uses `curl.exe`, which is part of Windows 10 1803 and newer,
as Glint's bug report does.

For each user: mint a code (`node server/doctor/scripts/doctor-admin.mjs mint
"who"`), copy the exe to `glint-doctor-<code>.exe`, send it.

## Options

    glint-doctor.exe --code ABCD-EFGH-JKMN   the code as an argument
    glint-doctor.exe --dry-run report.txt    collect, write to a file, send nothing (works without a code)
    glint-doctor.exe --keep                  do not remove the exe after a successful run
    glint-doctor.exe --yes                   no questions (for tests)

`GLINT_DOCTOR_URL` replaces the built-in address; `GLINT_DATA_DIR` is honored
as in Glint.
