# Glint plugins

A plugin is a folder of plain files that Glint loads from `%APPDATA%\Glint\plugins\<id>\`. Users get the official ones from **Tools → Plugins** in Glint.

```
plugins/
  index.json         the marketplace's list (made by tools/plugin-index.js, never edited by hand)
  official/<id>/     one official plugin: plugin.json, its script, optional CSS, market.json
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
- `api: true` asks for the local API (a port on 127.0.0.1 for a program of the plugin's own). See the top of `src-tauri/src/plugins.rs`.

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
  onDrop: (paths, point, c) => {},            // files dropped on Glint's window while one of its pages is open
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
| `c.proc.start(name, args, { audio, quiet, tool })`, `c.proc.stop(name)`, `c.proc.status(name)` | a process of the plugin: Glint's ffmpeg with the plugin's arguments, or (`tool`) a program from its downloads |
| `c.proc.run(name, args, { tool, onTick })` | start a process that does one job and wait for it: its last status (`code` 0 = it finished its work) |
| `c.dl.state()`, `c.dl.get(id)`, `c.dl.cancel(id)`, `c.dl.remove(id)` | the plugin's downloads: `[{ id, name, size, installed, busy }]`, fetch one, stop it, delete it |
| `c.work.read(name)`, `c.work.clear()` | a text file one of its processes wrote into the plugin's scratch folder (`{work}` in the arguments); empty that folder |
| `c.saveText(name, text)` | a save dialog for a text the plugin made (.srt, .vtt, .txt, .csv or .json): the path, or null when cancelled |
| `c.editor()` | the open editor (below), or null |
| `c.saveNow()`, `c.confirm({ title, text, ok, cancel })` | save and wait until the backend has the settings; Glint's own question: `"yes"`, `"no"`, or `null` when closed unanswered |
| `c.invoke(command, args)` | any of Glint's backend commands |
| `c.windows()`, `c.copy(text)` | the open windows, `[{ hwnd, title, exe, minimized }]`; put text on the clipboard |
| `c.files.keep(path)`, `c.files.drop(path)`, `c.fileSrc(path)` | keep Glint's own copy of a picture the user gave the plugin (PNG, JPEG, GIF, WebP or BMP, up to 64 MB): `{ path, name, size }`; delete that copy; the address to show a local file from |

Use Glint's own markup (`page-head`, `section`, `row`, `btn`, `note`, `field`) and its CSS variables so a plugin's page looks like the rest of the program and follows the user's theme.

### Processes

`c.proc.start("live", args, { audio: true })` runs Glint's ffmpeg with `args`. It ends with Glint, its last messages come back in `status.log`, and an address in the arguments is never logged in full. `quiet: true` is for a short question to ffmpeg (its list of cameras, say): its end shows no message. With `audio: true` Glint writes its live desktop and microphone sound to the process's stdin as 16-bit stereo, and `{audio_rate}` in the arguments becomes its sample rate:

```
-f s16le -ar {audio_rate} -ac 2 -i pipe:0
```

`status.code` is the exit code once a process has ended by itself, and `status.error` stays empty when that is 0.

In the arguments, `{work}` is the plugin's own scratch folder (made when first used) and `{dl:<id>}` is one of its downloads: the file, or its folder when it was unzipped.

For a keybind to start a process while the window is closed, save the arguments in the plugin's settings from `onSave`:

```js
onSave: (c) => { c.s.procs = { live: { args: buildArgs(c), audio: true } }; }
```

### Downloads

A plugin's own files are small text files. Anything larger (a program, a model) is a download, listed in `plugin.json`:

```json
"downloads": [
  { "id": "whisper", "name": "whisper.cpp", "url": "https://github.com/.../whisper-bin-x64.zip",
    "sha256": "...", "size": 8928640, "unzip": true, "run": "whisper-cli.exe" },
  { "id": "model-base", "name": "Speech model", "url": "https://.../ggml-base.bin", "sha256": "...", "size": 147951465 }
]
```

- Nothing is fetched until the script calls `c.dl.get(id)`, so say in the plugin's own page what will be downloaded and how large it is before the button that does it.
- `url` must be https and should name one exact file (a release tag, a commit), never "latest": Glint keeps the file only if its `size` and `sha256` match, and deletes it otherwise. An official plugin's `plugin.json` is itself checked against the marketplace's list, so these sums cannot be swapped on the way.
- `unzip: true` unpacks a zip into the download's folder. `run` names a program inside it (found up to three folders down): `c.proc.start(name, args, { tool: "<id>" })` runs that program instead of ffmpeg.
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

Things on the picture are the model's `overlays`. A text item is `{ id, kind: "text", from, to, x, y, text, color, size, outline }` with positions and sizes as fractions of the picture; `align: "center"` makes `x` the middle of its lines, and `tag` (the plugin's id) marks the items a plugin made so it can find them again. Items added this way are on the timeline, can be moved and retyped by the user, and are part of the export like any other.

### Live streams

A stream made of one ffmpeg ends whenever its capture does, and a screen capture ends every time a game enters or leaves exclusive fullscreen. `c.proc.live("live", spec)` (Glint 0.1.55, `src-tauri/src/live.rs`) runs a stream as two kinds of ffmpeg with Glint between them:

- `spec.send`: the sender, which holds the connection. `{video}` is where it reads the finished video from (MPEG-TS, to be copied, with `-copyts`); the sound is 16-bit stereo at `{audio_rate}` on its stdin.
- `spec.feed`: the feeder, which captures and encodes the picture and writes MPEG-TS to `pipe:1`. Glint starts a new one whenever it ends, and the sender never notices. `{source}` in its filter graph is the capture, as frames the encoder takes; `{t0}` is the stream's start in microseconds, for `setpts=(RTCTIME-{t0})/(TB*1000000)`, which keeps picture and sound in step over any number of feeders.
- `spec.source`: `{ kind: "monitor" | "game" | "window", monitor, hwnd, exe, title, fps, cursor, width, height, encoder, raw }`. `game` is the game Glint sees: its window, its screen while it is in exclusive fullscreen, a black picture while it is minimized or gone. `raw: true` asks for plain BGRA frames, for a filter graph of the plugin's own.
- `spec.audio`: `{ desktop: { on, gain, device }, mic: { on, gain, device }, game: { on, gain }, apps: [{ exe, on, gain }] }`. Glint mixes these from captures of its own. `game` and `apps` are single programs' sound; `desktop` is everything a device plays, which includes them.

`c.proc.update("live", { audio })` changes the sound at once while live; `{ source }` or `{ feed }` take effect with a new feeder. `c.proc.status` and `onProc` carry `live: { state: "starting" | "live" | "recovering" | "reconnecting", note, restarts, reconnects, showing }`. When the sender ends, Glint connects again a few times, waiting longer each time, before the stream ends with the reason. `c.windows()` lists the open windows for a picker.

For the keybind, save the stream as `c.s.procs = { live: { live: spec } }`.

## Trying a plugin without publishing it

- **In a browser:** `node tools/serve.js <path to this repository>` and open `http://localhost:4173/ui/index.html`. The preview's marketplace is this folder, and Install loads the plugin as Glint would (a stand-in backend: nothing is captured, sent or downloaded; a download's bar just fills, and the Subtitles plugin gets a few made-up lines).
- **In Glint:** copy the plugin's folder to `%APPDATA%\Glint\plugins\<id>\` and restart Glint. The Plugins page lists it under "Installed by hand".

## Publishing an official plugin

1. Put it in `plugins/official/<id>/`, or change it there and raise `version` in its `plugin.json`.
2. Run `node tools/plugin-index.js`. It rewrites `plugins/index.json` with every file's size and SHA-256.
3. Commit both and merge to `main`. That is the release: Glint reads the list and the files from `main` of this repository, and installed copies are offered "Update to x.y.z".

Glint installs a plugin only if every downloaded file matches the list, so a plugin changed without step 2 fails to install instead of installing something unlisted. GitHub can take a few minutes to serve a new list and new files together; an install in that window says to try again shortly.

## Cameras

Glint's window never shows the browser's own "wants to use your cameras" prompt. A plugin that opens a camera in the window (`navigator.mediaDevices.getUserMedia`) asks first with `c.confirm`, saves the answer as `camera_access` (`"allow"` or `"deny"`) in its settings with `c.saveNow()`, and only then opens the camera. Glint's backend answers the window's camera request from that setting (`src-tauri/src/camperm.rs`): allowed when an installed plugin that is switched on has `camera_access: "allow"`, refused otherwise. The microphone is always refused.
