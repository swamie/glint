# Claude plugin for Glint (MCP)

An optional add-on. It lets Claude (Claude Desktop or Claude Code) work in Glint's editor: bring clips in, cut, trim, reorder, add voice messages and music, put text on the picture and export. Since Claude can't listen to audio or watch video directly, the plugin also gives it timed transcripts, loudness and silence data, and screenshots of frames.

It is not part of Glint's build. Glint without it is unchanged, and nothing plugin-related runs until a plugin is installed.

## How it fits together

```
Claude  --MCP (stdio)-->  glint-mcp.exe  --127.0.0.1 + token-->  Glint  --event-->  bridge.js (in the editor)
                              |
                              +-- ffmpeg / ffprobe (frames, loudness)
                              +-- whisper.cpp (transcripts, optional)
                              +-- your shared folders (config.json)
```

- `server/` is `glint-mcp.exe`, the MCP server. Claude starts it.
- `bridge.js` runs inside Glint's window and makes each edit with the editor's own functions. Every change is one undo step, a project saves itself, and you see it happen on the timeline.
- `plugin.json` tells Glint to load `bridge.js` and open its local API (see `src-tauri/src/plugins.rs`).

## Install

Needs Rust (the same toolchain Glint builds with) and Windows 10 or 11.

```powershell
cd plugins\claude-mcp
.\install.ps1 -Transcripts
```

That builds `glint-mcp.exe`, copies the plugin to `%APPDATA%\Glint\plugins\claude-mcp\`, adds a `glint` server to Claude Desktop's config, and downloads whisper.cpp with the `base.en` model (about 150 MB) for transcripts. Then restart Glint and Claude Desktop.

- Claude Code instead of Claude Desktop: `.\install.ps1 -NoClaudeDesktop -Transcripts`, then `claude mcp add glint -- "%APPDATA%\Glint\plugins\claude-mcp\glint-mcp.exe"`.
- Better transcripts (slower, bigger): `-Model small.en`. For other languages use a model without `.en`, such as `small`.
- Turn it off without removing it: put an empty file named `disabled` in the plugin's folder and restart Glint.
- Remove it: `.\install.ps1 -Uninstall`.

## Sharing folders

Claude can only read files in Glint's clips folder and in folders you share. Each shared folder has a note so Claude knows what's in it. Shared folders are read-only unless you add `--write`.

```powershell
$g = "$env:APPDATA\Glint\plugins\claude-mcp\glint-mcp.exe"
& $g grant "D:\Assignment\Sam voice messages" --note "Voice messages from Sam for the assignment, one line of the script each"
& $g grant "D:\Assignment\Script" --note "The script (script.pdf). Speakers: Sam, Peter"
& $g grant "D:\Assignment\Sorted" --note "Put organized copies here" --write
& $g folders
& $g revoke "D:\Assignment\Script"
```

Claude never deletes anything. With `--write` it may copy files in, move files between writable folders, and save text files (for example a list of which voice message matches which script line). It never overwrites a file.

## What Claude can do

| Area | Tools |
|---|---|
| Library | `list_clips`, `list_projects`, `import_files`, `create_project`, `open_in_editor`, `close_editor` |
| Timeline | `get_timeline`, `add_clips`, `assemble_timeline`, `split`, `delete_pieces`, `trim_piece`, `move_piece`, `close_gaps`, `set_piece_speed`, `undo`, `redo`, `seek` |
| Sound | `set_row_volume`, `add_audio`, `edit_audio_clip` |
| Picture | `add_text`, `remove_overlay`, `set_fades`, `set_color` |
| Export | `export_video` (a new clip in the library) |
| Seeing and hearing | `media_info`, `get_frames`, `contact_sheet`, `audio_levels`, `transcribe`, `transcribe_timeline` |
| Your folders | `list_folders`, `list_files`, `read_document` (txt, md, pdf, docx, srt, vtt, csv, json), `copy_file`, `move_file`, `save_text` |

`assemble_timeline` is the quickest way to splice: Claude reads the transcripts, picks the ranges to keep from each clip, and lays them out in order in one step (one undo).

Things to ask Claude, for example:

- "Make a project from my last three clips, cut out the dead air and the bits where I'm loading, and export it."
- "Here's my script in the Script folder. Match each of Sam's voice messages to its line, tell me which lines are missing or said differently, put them on the timeline in script order, and save copies named after the lines into Sorted."

## Privacy

Everything runs on your computer. Transcripts are made locally by whisper.cpp and cached in the plugin's `cache` folder. Frames and transcripts are sent to Claude only when it asks for them, as part of your conversation. Glint's local API listens on 127.0.0.1 only and needs the token in `%APPDATA%\Glint\plugins\api.json`.

## Building by hand

```powershell
cd plugins\claude-mcp\server
cargo build --release
```

From Linux: `cargo build --release --target x86_64-pc-windows-gnu` (needs `gcc-mingw-w64-x86-64`).
