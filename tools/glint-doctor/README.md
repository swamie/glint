# Glint Doctor (the tool)

Optional, and separate from Glint: nothing in `src-tauri` or `ui` knows about
it. It is a small console program a Glint user runs once, after the developer
has given them a support code. It collects diagnostics, takes the personal data
out, shows what it will send, and sends it to the developer's Worker
(`server/doctor`). After one successful run it does not work again.

How to deploy the Worker, set its secrets, mint a code and read the report is
in `server/doctor/README.md`. In short:

1. Deploy the Worker and set its secrets (`server/doctor/README.md`, steps 1 and 2).
2. Put the Worker's address in `endpoint.txt` here and build the tool (below).
3. Mint a code: `node server/doctor/scripts/doctor-admin.mjs mint "who it is for"`.
4. Copy `glint-doctor.exe` to `glint-doctor-<code>.exe` and send that file to the user.
5. Read the report in Discord or with `doctor-admin.mjs show <ticket>`.

## Building

    cd tools/glint-doctor
    # once: replace the placeholder in endpoint.txt with the Worker's address
    cargo test
    cargo build --release
    # -> target\release\glint-doctor.exe: one small exe, no installer, no DLLs

`endpoint.txt` is built into the exe. It holds an address, not a secret, and
the copy in the repository is a placeholder: a tool built with it says so and
stops. The API key is never in the tool.

It is its own Cargo project with an empty `[workspace]`, so building Glint
never builds it, and the other way round. Dependencies: `serde_json`, `regex`,
`winreg`. The C runtime is linked in (`.cargo/config.toml`). Sending uses
`curl.exe`, which is part of Windows 10 1803 and newer, as Glint's bug report
does.

To check the Windows-only code from Linux:

    rustup target add x86_64-pc-windows-gnu
    sudo apt install gcc-mingw-w64-x86-64
    cargo check --target x86_64-pc-windows-gnu

## What the user does

1. Gets `glint-doctor-ABCD-EFGH-JKMN.exe` from the developer (the code is in
   the file name) and double-clicks it. Windows SmartScreen may ask, as it does
   for Glint: More info, Run anyway.
2. Types a sentence about the problem, or presses Enter.
3. Presses Enter to send, V to read everything first in Notepad, or Q to quit.
   After Q the code is still good.
4. Sees "Sent. Thank you!" and, a minute or two later, a short summary in plain
   words (unless the developer switched that off).

If the file was renamed, the tool asks for the code instead.

## What it reads

Read only. The one thing it writes is `doctor-used.json` in Glint's data folder,
and after a successful run it removes its own exe (not with `--keep`).

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

The upload is at most 360 KB; the oldest log lines go first.

## What is taken out first

`src/scrub.rs`, with tests. The user's profile folder becomes `~`, by the same
rule as Glint's bug report. The Windows user name and the profile folder's name
become `~` where they stand as words of their own (not when the name is an
ordinary word such as "glint" or "admin"). Also removed:

- other users' folder names, the PC's name, the Windows domain, network share
  hosts, Windows account ids (SIDs), and the organization in a
  "OneDrive - <org>" folder;
- clip, picture and sound file names (the folders and the file type stay);
- e-mail addresses (also URL-encoded), IPv4 and IPv6 addresses;
- Discord webhooks (with or without an API version in the address), the query
  of every web address, and any host that is not one of Glint's own;
- anything that looks like a key or token: `token=`, `access_token=`,
  `Bearer ...`, `x-api-key: ...`, JSON web tokens, Discord bot tokens, long hex
  and long random-looking strings.

In the settings, values are dropped by the setting's name (token, secret,
password, webhook, key, email, ...), and the share-link settings (`share`:
`url`, `token`, `name`) are dropped one by one. A dropped value says whether it
was set, so the report still shows, for example, that share links are set up.
Web addresses are dropped, and custom CSS, presets, folder colors and tags are
reduced to a count.

This is best effort on free text: a name inside a window title that the log
quotes can get through. That is why the user sees everything before it is
sent. The Worker runs a second pass before anything is stored.

## Options

    glint-doctor.exe --code ABCD-EFGH-JKMN   the code as an argument
    glint-doctor.exe --dry-run report.txt    collect, write to a file, send nothing (works without a code)
    glint-doctor.exe --keep                  do not remove the exe after a successful run
    glint-doctor.exe --yes                   no questions (for tests)

`GLINT_DOCTOR_URL` replaces the built-in address (the Worker's tests use it);
`GLINT_DATA_DIR` is honored as in Glint.

## Running once

The Worker is the guarantee: it marks a code used in the same atomic step that
stores the upload, and refuses it after that. On the PC, `doctor-used.json`
stops a second run and the exe removes itself; both are easy to undo and only
spare an honest user confusion. A copied exe with a used code gets "That
support code has already been used."
