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
| `c.proc.start(name, args, { audio, quiet })`, `c.proc.stop(name)`, `c.proc.status(name)` | a process of the plugin: Glint's ffmpeg with the plugin's arguments |
| `c.saveNow()`, `c.confirm({ title, text, ok, cancel })` | save and wait until the backend has the settings; Glint's own question: `"yes"`, `"no"`, or `null` when closed unanswered |
| `c.invoke(command, args)` | any of Glint's backend commands |
| `c.files.keep(path)`, `c.files.drop(path)`, `c.fileSrc(path)` | keep Glint's own copy of a picture the user gave the plugin (PNG, JPEG, GIF, WebP or BMP, up to 64 MB): `{ path, name, size }`; delete that copy; the address to show a local file from |

Use Glint's own markup (`page-head`, `section`, `row`, `btn`, `note`, `field`) and its CSS variables so a plugin's page looks like the rest of the program and follows the user's theme.

### Processes

`c.proc.start("live", args, { audio: true })` runs Glint's ffmpeg with `args`. It ends with Glint, its last messages come back in `status.log`, and an address in the arguments is never logged in full. `quiet: true` is for a short question to ffmpeg (its list of cameras, say): its end shows no message. With `audio: true` Glint writes its live desktop and microphone sound to the process's stdin as 16-bit stereo, and `{audio_rate}` in the arguments becomes its sample rate:

```
-f s16le -ar {audio_rate} -ac 2 -i pipe:0
```

For a keybind to start a process while the window is closed, save the arguments in the plugin's settings from `onSave`:

```js
onSave: (c) => { c.s.procs = { live: { args: buildArgs(c), audio: true } }; }
```

## Trying a plugin without publishing it

- **In a browser:** `node tools/serve.js <path to this repository>` and open `http://localhost:4173/ui/index.html`. The preview's marketplace is this folder, and Install loads the plugin as Glint would (a stand-in backend: nothing is captured or sent).
- **In Glint:** copy the plugin's folder to `%APPDATA%\Glint\plugins\<id>\` and restart Glint. The Plugins page lists it under "Installed by hand".

## Publishing an official plugin

1. Put it in `plugins/official/<id>/`, or change it there and raise `version` in its `plugin.json`.
2. Run `node tools/plugin-index.js`. It rewrites `plugins/index.json` with every file's size and SHA-256.
3. Commit both and merge to `main`. That is the release: Glint reads the list and the files from `main` of this repository, and installed copies are offered "Update to x.y.z".

Glint installs a plugin only if every downloaded file matches the list, so a plugin changed without step 2 fails to install instead of installing something unlisted. GitHub can take a few minutes to serve a new list and new files together; an install in that window says to try again shortly.

## Cameras

Glint's window never shows the browser's own "wants to use your cameras" prompt. A plugin that opens a camera in the window (`navigator.mediaDevices.getUserMedia`) asks first with `c.confirm`, saves the answer as `camera_access` (`"allow"` or `"deny"`) in its settings with `c.saveNow()`, and only then opens the camera. Glint's backend answers the window's camera request from that setting (`src-tauri/src/camperm.rs`): allowed when an installed plugin that is switched on has `camera_access: "allow"`, refused otherwise. The microphone is always refused.
