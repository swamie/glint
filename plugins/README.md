# Glint plugins

A plugin is a folder of plain files that Glint loads from `%APPDATA%\Glint\plugins\<id>\`. Users get the official ones from **Tools → Plugins** in Glint.

```
plugins/
  index.json         the marketplace's list (generated when a plugin is published, never edited by hand)
  official/<id>/     one official plugin: plugin.json, its script, optional CSS, market.json
  locked/<id>/       a locked plugin: in the marketplace for everyone, installed only with its code (see "Locked plugins")
```

## plugin.json

```json
{
  "id": "stream",
  "name": "Stream",
  "version": "0.1.0",
  "description": "One or two sentences for the marketplace card.",
  "author": "Glint developers",
  "min_glint": "0.1.53",
  "ui": "stream.js",
  "css": "stream.css",
  "actions": [
    { "id": "toggle", "label": "Start / stop streaming", "process": "live",
      "started": "Stream started", "stopped": "Stream stopped" }
  ]
}
```

- `id` is the folder's name: lower-case letters, digits and dashes.
- `ui` is a script run in Glint's window each time it opens. `css` is optional.
- `actions` are what a keybind can carry. They are listed here, not in the script, because Glint's window (and every script in it) is gone while Glint sits in the tray. An action with a `process` starts or stops that process by itself; `started` and `stopped` are the overlay's words.
- `downloads` lists larger files the plugin needs but does not carry, such as a program or a model. See [Downloads](#downloads).
- `hosts` lists the web hosts the script may reach: `c.http({ method, url, headers, body })` makes an https request to one of them (`{ status, body }`, made by Glint's backend, redirects not followed) and `c.open(url)` opens a page on one of them in the user's browser. Any other address is refused.
- `api: true` asks for the local API (a port on 127.0.0.1 for a program of the plugin's own).
`market.json` beside it holds what only the marketplace card needs: `icon` (the `d` of one SVG path on a 24 x 24 grid) and `tag` (a short label such as "Preview").

## The script

The script gets one argument, `Glint`, and registers what it adds:

```js
const { ctx, t, esc } = Glint;

Glint.register({
  defaults: { greeting: "Hello" },            // its settings' first values
  pages: [{                                   // pages under Tools
    id: "hello", title: "Hello", icon: '<svg viewBox="0 0 24 24">...</svg>',
    render: (c) => `<div class="page-head"><div><h1>${t("Hello")}</h1></div></div>
      <div class="section"><h2>${t("Greeting")}</h2>${c.row(t("Text"), "", c.text("greeting"))}</div>`,
    badge: (c) => "",                         // optional: a small label on its sidebar item
  }],
  settings: [{ title: "Hello", render: (c) => c.row(t("Text"), "", c.text("greeting")) }],  // sections on the Settings page
  topbar: (c) => "",                          // markup for the top bar, made of Glint's own .btn buttons (e.g. "Streaming 1080p @ 60fps")
  actions: { toggle: (c) => c.render() },     // a keybind's action fired while the window is open
  onAct: (name, el, c) => {},                 // a click on an element with data-pact="name"
  onSave: (c) => {},                          // just before the settings are saved
  onProc: (status, c) => {},                  // one of its processes started or ended
  onDownload: (d, c) => {},                   // one of its downloads moved on: { id, phase, done, total, error }
  editor: {                                   // a section in the editor's side pane
    title: "Hello", tab: "overlays",          // tab: "clip" (the default), "color" or "overlays"
    render: (ed, c) => "",                    // its markup; `ed` is the open editor (below)
    onOpen: (ed, c) => {}, onSync: (ed, what, c) => {}, onClose: (ed, c) => {},
  },
  clipMenu: (clips, c) => [{ id: "hello", label: "Say hello", icon: "<svg>...</svg>" }],  // entries in a clip's right-click menu (Glint 0.1.56)
  onClipMenu: (id, clips, c) => {},           // one of them was picked, for these clips
  onDrop: (paths, point, c) => {},            // files dropped on Glint's window while one of its pages is open
  onAudio: (status, c) => {},                 // the sound it asked for changed state: { source, state, error } (Glint 0.1.60)
  onTick: (c) => {},                          // once a second while it holds the window open (Glint 0.1.60)
  start: (c) => {},                           // once, after everything is loaded
  lang: { ko: { "Hello": "안녕하세요" } },      // its texts in Glint's other languages
});
```

`ctx` (the `c` above):

| | |
|---|---|
| `c.s` | the plugin's own settings object. Saved inside Glint's settings file under `plugins.<id>`. |
| `c.row(name, hint, control)` | one settings row, as on Glint's own pages |
| `c.toggle(key)`, `c.select(key, [[value, label]], isNumber)`, `c.number(key, min, max)`, `c.text(key, { placeholder, password, width })` | controls bound to one of its settings. They save by themselves. |
| `c.save()`, `c.render()`, `c.chrome()` (top bar and sidebar only), `c.go(page)`, `c.flash(text, bad)`, `c.fail(error)` | save the settings, redraw, open a page, show a message |
| `c.glint` | Glint's own state, to read: `settings`, `status`, `monitors`, `hardware`, `version` |
| `c.proc.start(name, args, { audio, quiet, tool, program, out })`, `c.proc.stop(name)`, `c.proc.status(name)` | a process of the plugin: Glint's ffmpeg with the plugin's arguments, or (`tool`) a program from its downloads |
| `c.proc.run(name, args, { tool, program, out, onTick })` | start a process that does one job and wait for it: its last status (`code` 0 = it finished its work) |
| `c.dl.state()`, `c.dl.get(id)`, `c.dl.cancel(id)`, `c.dl.remove(id)` | the plugin's downloads: `[{ id, name, size, installed, busy }]`, fetch one, stop it, delete it |
| `c.work.read(name)`, `c.work.clear()` | a text file one of its processes wrote into the plugin's scratch folder (`{work}` in the arguments); empty that folder |
| `c.saveText(name, text)` | a save dialog for a text the plugin made (.srt, .vtt, .txt, .csv or .json): the path, or null when cancelled |
| `c.editor()` | the open editor (below), or null |
| `c.saveNow()`, `c.confirm({ title, text, ok, cancel })` | save and wait until the backend has the settings; Glint's own question: `"yes"`, `"no"`, or `null` when closed unanswered |
| `c.invoke(command, args)` | any of Glint's backend commands |
| `c.windows()`, `c.copy(text)` | the open windows, `[{ hwnd, title, exe, minimized }]`; put text on the clipboard |
| `c.clips.list()`, `c.clips.get(id)`, `c.clips.open(id)`, `c.clips.export(id, opts)` | (Glint 0.1.56) the library's clips, `{ id, title, path, width, height, duration, fps, game, folder, draft }`; open one in the editor; make a new clip from one with the editor's export (`opts` are the editor's export options), which resolves to the new clip and shows in the renders list while it runs |
| `c.media.now(have)`, `c.media.apps()` | (Glint 0.1.60) what is playing on this PC: `{ found, title, artist, album, app, exe, playing, source, pos_ms, dur_ms, art_key, art }` (`pos_ms` of `dur_ms` is where the track is, both 0 when the player does not say; `art` is the cover as a data URL, left out when you pass the `art_key` you already have); the programs that play sound right now, `[{ exe, name }]` |
| `c.audio.start(source, onChunk)`, `c.audio.keep()`, `c.audio.stop()` | (Glint 0.1.60) the live sound of one program (`"spotify.exe"`) or of everything but Glint (`"all"`). See [Live sound](#live-sound) |
| `c.keepOpen(on)` | (Glint 0.1.60) while on, the X button hides the window instead of closing the page, and `onTick` runs every second |
| `c.files.keep(path)`, `c.files.drop(path)`, `c.fileSrc(path)` | keep Glint's own copy of a picture the user gave the plugin (PNG, JPEG, GIF, WebP or BMP, up to 64 MB): `{ path, name, size }`; delete that copy; the address to show a local file from |

Use Glint's own markup (`page-head`, `section`, `row`, `btn`, `note`, `field`) and its CSS variables so a plugin's page looks like the rest of the program and follows the user's theme.

### Processes

`c.proc.start("live", args, { audio: true })` runs Glint's ffmpeg with `args`. It ends with Glint, its last messages come back in `status.log`, and an address in the arguments is never logged in full. `quiet: true` is for a short question to ffmpeg (its list of cameras, say): its end shows no message. With `audio: true` Glint writes its live desktop and microphone sound to the process's stdin as 16-bit stereo, and `{audio_rate}` in the arguments becomes its sample rate:

```
-f s16le -ar {audio_rate} -ac 2 -i pipe:0
```

`status.code` is the exit code once a process has ended by itself, and `status.error` stays empty when that is 0.

In the arguments, `{work}` is the plugin's own scratch folder (made when first used) and `{dl:<id>}` is one of its downloads: the file, or its folder when it was unzipped. For a downloaded program both are given in their short 8.3 form when the path has letters outside ASCII (a user named José), because many command line programs cannot open such a path.

`out: "result.json"` (Glint 0.1.56) keeps what a process prints: its stdout becomes that file in the scratch folder and everything it says on stderr becomes `result.json.log` beside it, both read with `c.work.read(name)`. Without it only the last 40 lines of stderr are kept, in `status.log`.

For a keybind to start a process while the window is closed, save the arguments in the plugin's settings from `onSave`:

```js
onSave: (c) => { c.s.procs = { live: { args: buildArgs(c), audio: true } }; }
```

### Downloads

A plugin's own files are small text files. Anything larger (a program, a model) is a download, listed in `plugin.json`:

```json
"downloads": [
  { "id": "sherpa", "name": "Speech engine", "url": "https://github.com/.../sherpa-onnx-v2.0.2-win-x64-shared-MT-Release-no-tts.tar.bz2",
    "sha256": "...", "size": 26896305, "unzip": true, "run": "sherpa-onnx-offline.exe", "run_also": ["sherpa-onnx-vad.exe"],
    "only": ["bin/sherpa-onnx-offline.exe", "bin/sherpa-onnx-vad.exe", "bin/onnxruntime.dll"] },
  { "id": "vad", "name": "Voice detector", "url": "https://.../silero_vad.onnx", "sha256": "...", "size": 643854 }
]
```

- Nothing is fetched until the script calls `c.dl.get(id)`, so say in the plugin's own page what will be downloaded and how large it is before the button that does it.
- `url` must be https and should name one exact file (a release tag, a commit), never "latest": Glint keeps the file only if its `size` and `sha256` match, and deletes it otherwise. An official plugin's `plugin.json` is itself checked against the marketplace's list, so these sums cannot be swapped on the way.
- `unzip: true` unpacks an archive into the download's folder: a zip, or (Glint 0.1.56) a `.tar.bz2`, which Glint unpacks itself because Windows 10's tar.exe cannot. For a `.tar.bz2`, `only` lists the files to keep, each the end of its path inside the archive; everything else is skipped.
- `run` names a program inside it (found up to three folders down): `c.proc.start(name, args, { tool: "<id>" })` runs that program instead of ffmpeg. `run_also` (Glint 0.1.56) names more programs of the same download; `{ tool: "<id>", program: "other.exe" }` runs one of them.
- Downloads live in `%APPDATA%\Glint\plugin-files\<id>\dl\`, outside the plugin's folder, so an update of the plugin keeps them. Removing the plugin deletes them.

### The editor

A plugin with `editor` gets a section in the editor's side pane whenever a clip or project is open there. `render(ed, c)` returns its markup (Glint's own `btn`, `field`, `check`, `e-sliders`, `small`, `progress`); a click on an element with `data-pact` goes to `onAct`. Call `ed.redraw()` to draw it again. `ed`:

| | |
|---|---|
| `ed.clip` | `{ id, title, path, width, height, duration, virtual }` |
| `ed.model` | the edit model (`ui/editmodel.js`), to read |
| `ed.change(fn)` | one undo step: `fn(model)` changes the model, the timeline and the picture follow |
| `ed.time()`, `ed.seek(sec)` | the playhead, in timeline seconds |
| `ed.footage()` | what the main row shows, in order: `[{ at, len, speed, path, from, tracks, missing }]` (timeline seconds `at` .. `at + len` show the file `path` from its second `from`) |
| `ed.removedAt(sec)`, `ed.outRange(from, to)` | whether a timeline second is cut out; a timeline range in seconds of the finished clip |
| `ed.picture()`, `ed.textWidth(text, size)` | the result's size in pixels; a line's width as a fraction of the picture's width |
| `ed.texts(tag)`, `ed.select(id)` | the text items on the picture that carry `tag`; select an item |
| `ed.export(extra)`, `ed.exporting` | (Glint 0.1.56) the editor's own Export, with `extra` added to its options: `{ fps_cap, max_kbps, label }` (at most this many frames a second, at most this many kbit/s, and the word in the new clip's name); whether an export is running |

`ed.textWidth(text, size, { width, height })` measures on a picture of another size, for a change that also changes the picture's shape.

The picture's shape is the model's `canvas` (`ui/canvas.js`): `null` for the clip's own, else `{ width, height, background: "blur" | "black" | "#rrggbb", pieces: [{ id, sx, sy, sw, sh, x, y, w, h }] }`, where each piece shows a box of the clip's picture (`s*`, fractions of it) in a box of the canvas. The Vertical plugin sets it inside `ed.change`.

Things on the picture are the model's `overlays`. A text item is `{ id, kind: "text", from, to, x, y, text, color, size, outline }` with positions and sizes as fractions of the picture; `align: "center"` makes `x` the middle of its lines, and `tag` (the plugin's id) marks the items a plugin made so it can find them again. Items added this way are on the timeline, can be moved and retyped by the user, and are part of the export like any other.

### Live streams

A stream made of one ffmpeg ends whenever its capture does, and a screen capture ends every time a game enters or leaves exclusive fullscreen. `c.proc.live("live", spec)` (Glint 0.1.55) runs a stream as two kinds of ffmpeg with Glint between them:

- `spec.send`: the sender, which holds the connection. `{video}` is where it reads the finished video from (MPEG-TS, to be copied, with `-copyts`); the sound is 16-bit stereo at `{audio_rate}` on its stdin.
- `spec.feed`: the feeder, which captures and encodes the picture and writes MPEG-TS to `pipe:1`. Glint starts a new one whenever it ends, and the sender never notices. `{source}` in its filter graph is the capture, as frames the encoder takes; `{t0}` is the stream's start in microseconds, for `setpts=(RTCTIME-{t0})/(TB*1000000)`, which keeps picture and sound in step over any number of feeders.
- `spec.source`: `{ kind: "monitor" | "game" | "window", monitor, hwnd, exe, title, fps, cursor, width, height, encoder, raw }`. `game` is the game Glint sees: its window, its screen while it is in exclusive fullscreen, a black picture while it is minimized or gone. `raw: true` asks for plain BGRA frames, for a filter graph of the plugin's own.
- `spec.audio`: `{ desktop: { on, gain, device }, mic: { on, gain, device }, game: { on, gain }, apps: [{ exe, on, gain }] }`. Glint mixes these from captures of its own. `game` and `apps` are single programs' sound; `desktop` is everything a device plays, which includes them.

`c.proc.update("live", { audio })` changes the sound at once while live; `{ source }` or `{ feed }` take effect with a new feeder. `c.proc.status` and `onProc` carry `live: { state: "starting" | "live" | "recovering" | "reconnecting", note, restarts, reconnects, showing }`. When the sender ends, Glint connects again a few times, waiting longer each time, before the stream ends with the reason. `c.windows()` lists the open windows for a picker.

For the keybind, save the stream as `c.s.procs = { live: { live: spec } }`.

## Trying a plugin without publishing it

- **In Glint:** copy the plugin's folder to `%APPDATA%\Glint\plugins\<id>\` and restart Glint. The Plugins page lists it under "Installed by hand".

## Publishing an official plugin

Official plugins are published by the Glint maintainer. Glint reads the list and the files from `main` of this repository, and installed copies are offered "Update to x.y.z". To suggest a plugin or a change, open an issue.

Glint installs a plugin only if every downloaded file matches the size and SHA-256 in `plugins/index.json`, so a plugin changed without updating the list fails to install instead of installing something unlisted. GitHub can take a few minutes to serve a new list and new files together; an install in that window says to try again shortly.

## Locked plugins

A plugin in `plugins/locked/<id>/` is listed in the marketplace like any other, with a "Needs a code" mark and an "Enter code" button in place of Install. Glint does not download it from this repository: its files come from the share server (`https://clips.glintclip.com/plugin/<id>/<file>`), which hands one out only to a request that carries the plugin's code. Each file is still checked against the SHA-256 in `plugins/index.json`. The code a copy was installed with is kept in its receipt, so updates do not ask again unless the code was changed.

The codes are a secret held by the share server; they are never in this repository. The code keeps the app from installing the plugin. It does not hide the source: while `plugins/locked/` is in a public repository, its files can be read there.

## What is playing

`c.media.now()` (Glint 0.1.60) is the track Windows knows about: Spotify, a browser tab (YouTube Music and any site that fills in its media session), Media Player and most music players. Players that tell Windows nothing (VLC, foobar2000, Winamp, MusicBee, AIMP) are read from their window title (`source: "window"`). `exe` is the player's program file in lower case, or `""` when Glint cannot tell. Glint asks Windows once a second while a plugin keeps asking and stops 15 seconds after the last question, so ask every second or two while you show it and not at all otherwise.

## Live sound

`c.audio.start(source, onChunk)` (Glint 0.1.60) sends your script the sound a program is playing, as it plays: `onChunk` gets `Int16Array` pieces of about 40 ms, 48 kHz stereo, and nothing while the source is silent. `source` is a program's file name (that program and everything it started) or `"all"` (everything this PC plays except Glint itself). It needs Windows 10 version 2004 or newer.

This is the user's sound, so the rules are strict:

- It reaches your `onChunk` and nothing else. Glint never writes it to a file or into the replay buffer. Do not save it either.
- Ask for it only while something needs it, and stop it (`c.audio.stop()`) the moment nothing does. Say on your page, and in the top bar while it runs, that sound is being shared and with whom.
- Call `c.audio.keep()` every few seconds. Without it for 20 seconds the sound stops by itself; it also stops when the page is loaded again or the window is gone. While it runs the tray icon's tip says so.
- `onAudio(status, c)` tells you when its state changes: `"waiting"` (the program is not running), `"capturing"`, `"failed"` (with `error`), `"stopped"`.

A plugin with live connections in the page (the Party plugin's WebRTC links) calls `c.keepOpen(true)` while they are up: the X button then hides the window instead of destroying the page, so they carry on from the tray. A hidden page's own timers are slowed down, so use `onTick` as the clock for anything that must keep its pace. Call `c.keepOpen(false)` as soon as nothing needs the page. With "close to tray" off, X still quits Glint.

## Cameras

Glint's window never shows the browser's own "wants to use your cameras" prompt. A plugin that opens a camera in the window (`navigator.mediaDevices.getUserMedia`) asks first with `c.confirm`, saves the answer as `camera_access` (`"allow"` or `"deny"`) in its settings with `c.saveNow()`, and only then opens the camera. Glint's backend answers the window's camera request from that setting: allowed when an installed plugin that is switched on has `camera_access: "allow"`, refused otherwise. The microphone is always refused.
