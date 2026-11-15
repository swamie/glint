"use strict";
// "What's new": the changelog dialog. It opens by itself once after an update (the version last
// seen is kept in localStorage["glint.seenVersion"]), from the logo in the top bar (#ver) and from
// Settings → Updates (both data-act="whats-new", handled in app.js).
// Loaded last, after app.js and sfx.js, so their globals (S, $, $$, esc and the t function) exist.
// All names here start with `wn` / `WN_`; markup uses `wn-` classes. While a `.wn-dlg` exists the
// app's own key handlers stand aside (app.js; the editor's copy of that line needs it too); this
// file's capture-phase listener handles Esc, and the dialog keeps Tab inside itself.

/**
 * The releases, newest first: { version, date, new, improved, fixed }. A function, so the texts
 * are translated in the language in use when the dialog opens. Written for users: what they can
 * now do. Only the newest two releases have Korean and Greek texts; older ones stay English.
 */
const wnReleases = () => [
  {
    version: "0.1.36", date: "2026-11-16",
    new: [
      t("The editor's main video row is free-form. Drag a clip to any time: dropped on empty space it sits exactly there and leaves a gap behind, dropped onto another clip it goes before or after it. A single clip can be moved later the same way."),
      t("Gaps: empty stretches on the video row are black and silent in the preview and in the export, while text, pictures and added songs go on over them. Click a gap and press Delete to close it, drag its edge to make it longer or shorter, or pick “Close all gaps”. “Cut out and leave a gap” (Shift+Delete) removes a piece without moving what comes after it."),
      t("A blade tool on the timeline (C): click the video or an overlay block to split it right there."),
    ],
    fixed: [
      t("In a project, the selection box in the Library list was cut off at the list's edge. It now follows the pointer, as on the Library page."),
    ],
  },
  {
    version: "0.1.35", date: "2026-11-16",
    new: [
      t("The editor fills the whole window while you edit, arranged like an editing program: the picture in the middle, the settings on the right, the library on the left in a project, the timeline along the bottom. Drag the borders, fold panels and sections, and use minimize, maximize and close without leaving the editor."),
      t("Pictures, GIFs and clips on the picture: add one with the button over the picture or by dropping the file, then move, resize, keyframe and crop it. A clip can bring its sound along as an audio row."),
      t("The timeline goes on past the end of the video: move the playhead there, and let texts, pictures and songs run on over a black picture. Everything on the picture shows as a block on its own row (V2, V3) that you can drag and trim."),
      t("Right-click the picture in the editor for a freeform crop: of a picture, GIF or clip you added, or of the clip itself."),
      t("Blur boxes can have round corners and a soft edge."),
      t("A heart button beside the bug button: give Glint one to five stars and tell us what you like or what should be better."),
    ],
    improved: [
      t("With an added audio file selected, the mouse wheel over it moves the song inside its block, so another part of it plays at the same place."),
      t("Timeline rows are named like an editor's (V1 for the picture, A1, A2 for the sound), with a timecode of the playhead and a + to add an audio file."),
      t("Turning Desktop audio or the microphone on or off no longer empties the replay buffer."),
    ],
    fixed: [
      t("With two monitors, clips could show the other monitor than the one chosen under Capture."),
      t("GAME mode with desktop sound: when the game plays on other headphones or speakers than the ones Glint records, the clip's first track held only the microphone. The game is now mixed into it."),
      t("An exported clip that took the place or the name of another file played the old file's sound until you skipped back."),
    ],
  },
  {
    version: "0.1.34", date: "2026-11-15",
    new: [
      t("Editor: an audio file you add (a song) plays on through every cut in the video. Cutting a part out no longer chops the song; the clip's own sound is still cut with the picture. A button on the file's row switches it back to being cut with the video."),
    ],
  },
  {
    version: "0.1.33", date: "2026-11-15",
    new: [
      t("GAME mode can record the desktop's sound too: the game's window with everything you hear. Turn Desktop audio on in Settings → Audio while GAME is chosen. Off, GAME records only the game's own sound and your microphone, as before."),
      t("Editor projects: rest the pointer on a clip in the Library list and it plays, without sound, in a small player beside the list."),
      t("Editor: any audio row can be deleted, the clip's own tracks too (the bin on the row). A deleted track is not in the export."),
      t("Keyframes: drag along a keyframe bar to scrub through the animation, and drag a diamond to move that keyframe to another time."),
      t("Editor shortcuts: a search box, and a right-click on a shortcut removes it."),
    ],
    improved: [
      t("With cut parts hidden, a trim bracket can be dragged back out to bring cut footage back: the cut parts show while you drag."),
      t("Waveforms are drawn twice as tall, and a button on the timeline shows or hides the names written on the audio."),
    ],
    fixed: [
      t("An added audio file lost its waveform when a project was closed and opened again."),
      t("An edit with every audio row muted was exported with its sound. It is silent now."),
    ],
  },
  {
    version: "0.1.32", date: "2026-11-15",
    new: [
      t("Settings → Audio has a mixer: a fader, a live level meter and an on / off button for Desktop audio, Microphone and Game audio. The faders set how loud each source is in your clips."),
    ],
    improved: [
      t("The categories under Settings in the sidebar are drawn as a tree."),
    ],
  },
  {
    version: "0.1.31", date: "2026-11-15",
    new: [
      t("Editor: crop the picture. Under Canvas, pick “Crop to a part of the picture” and drag the box, or its edges and corners, on the picture. The export is the box at its own size."),
      t("Custom clip sound: MP3 and other audio files work, not only WAV. The volume slider now applies to it, and you can choose the part of the file that is played by dragging the ends of its bar."),
      t("Settings has categories: Appearance, General, Audio and sounds, Clip overlay, Updates and program. They show under Settings in the sidebar."),
    ],
    improved: [
      t("The GAME / DESKTOP switch is narrower and has an arrow that opens its menu (pick the game, “This is not a game”); a right-click still opens it too."),
    ],
  },
  {
    version: "0.1.30", date: "2026-11-15",
    fixed: [
      t("The canvas preview (vertical formats) stuttered. It is now drawn once for each video frame instead of on every screen refresh, and the blurred background is done by the graphics card."),
      t("In the editor, Play at the end of the clip starts again from the beginning."),
    ],
  },
  {
    version: "0.1.29", date: "2026-11-15",
    improved: [
      t("The GAME / DESKTOP switch is a box like the other top bar buttons, and narrower."),
    ],
  },
  {
    version: "0.1.28", date: "2026-11-15",
    new: [
      t("A GAME / DESKTOP switch in the top bar. DESKTOP records the whole monitor and all the sound, as before. GAME records only the game: its window and its own sound, plus your microphone. Glint finds the game by itself; right-click the switch to pick it by hand or to say “This is not a game”."),
      t("Editor keyframes: text, shapes, drawings, blur boxes and canvas pieces can move and change size over time. Add a keyframe, move the playhead, drag the item, and it travels between the two, evenly or with Smooth."),
      t("Settings → Audio: pick which output device's sound is recorded and which microphone is used. If a chosen device is unplugged, Glint records the Windows default until it is back."),
      t("Render rules have a Canvas block: pick a layout you saved in the editor and the rule turns any clip into that vertical video in one click."),
    ],
    improved: [
      t("The editor's audio rows say what they hold: “Mic audio”, “Desktop audio” and the game's name for the game's own track."),
      t("Number boxes (Fade in, Fade out and the others) have Glint's own look, with small arrows to step the value."),
    ],
  },
  {
    version: "0.1.27", date: "2026-11-15",
    fixed: [
      t("In a window that was not maximized, the editor's player controls (play, the seek bar, volume, speed) did not react to clicks."),
      t("The hover sound no longer repeats while the pointer rests on a clip and the list is redrawn."),
    ],
  },
  {
    version: "0.1.26", date: "2026-11-15",
    new: [
      t("Editor: a Canvas for vertical videos. Pick 9:16 (TikTok, Shorts, Reels), 1:1, 4:5 or your own size, then choose which parts of the picture show and where: the gameplay in the middle, the kill feed zoomed above, the health bar below. Behind them is the clip blurred, black or a color."),
      t("Arrange the pieces on the picture: drag to move, a corner scales, an edge crops, the mouse wheel zooms. “Add a piece” lets you drag a box around what the piece should show."),
      t("Canvas layouts: ready-made ones for each shape, and your own, saved under “My layouts” and used on any clip with one click."),
    ],
    improved: [
      t("With a canvas, text, shapes, drawings and blur boxes are placed on the canvas."),
    ],
  },
  {
    version: "0.1.25", date: "2026-11-15",
    new: [
      t("Editor: put text, rectangles, ellipses, lines, arrows and freehand drawing on the picture, each for the time you choose. A blur box hides what is inside it: pick Blur and drag a box. Fade in and fade out for the start and the end of the clip."),
      t("A keybind that opens your newest clip in the editor: Keybinds → Edit the last clip."),
      t("Watching a clip fullscreen, the previous and next arrows show with the controls when the pointer moves, and the next clip opens without leaving fullscreen. Settings → Appearance sets how visible they are."),
      t("Right-click a text box for Glint's own Cut, Copy and Paste, in place of the browser's menu."),
    ],
    improved: [
      t("In the viewer, the previous and next arrows sit beside the clip instead of at the edges of the window."),
      t("Glint writes “color” everywhere."),
      t("In the import dialog, Medal's logo is the size of the others and Outplayed has its own mark."),
      t("In a render rule's Color block, “color panel” is a link that opens the editor at its color panel."),
    ],
    fixed: [
      t("A fix for the old-style window frame that could flash in for a moment while the window was resized fast."),
    ],
  },
  {
    version: "0.1.24", date: "2026-11-15",
    new: [
      t("Render rules: build a chain of steps from blocks (a color look, Blur, upscale, compress to a size, GIF, copy) and run it on a clip with one click, from the right-click menu, the viewer, or right after Export in the editor. Make them under Tools → Render rules."),
      t("Export as GIF: right-click a clip to make a GIF of it. You pick the size, the frames a second and an optional size limit; the GIF is saved beside the clip."),
      t("Upscale for YouTube: right-click a clip to save a 1080p, 1440p or 4K copy, sharp or smooth, so YouTube gives it more bitrate. It runs on your graphics card when it can."),
      t("Clip keys can save any length: choose Custom… on the Keybinds page and type minutes and seconds."),
      t("Tags on clip cards are back as a choice: under Settings → Appearance, pick which kinds (import, trim, edit, Blur, Discord and more) get a tag. None by default."),
      t("A new folder can get its color right away, in the same box as its name."),
      t("Drag the theme tiles under Settings → Appearance to put them in your own order."),
      t("In the editor, drag the bar under the tracks to make them taller. Double-click it to reset."),
    ],
    improved: [
      t("Blur shows its progress from the start. Before, Blur's percent only reached Glint near the end, so a render sat on “Starting Blur…” for most of its time. Clips that need preparing for Blur are also ready sooner."),
      t("Every dropdown opens Glint's own list, in the style of the right-click menu, with mouse and keyboard."),
      t("The key on the clip button keeps its edge at the bottom instead of fading into the button."),
      t("Discord Rich Presence is removed."),
    ],
    fixed: [
      t("The editor left empty space under the tracks, and with many tracks its toolbar scrolled away."),
    ],
  },
  {
    version: "0.1.23", date: "2026-11-15",
    new: [
      t("Export can replace the clip instead of saving a new one beside it. Tick “Replace the original clip” in the editor's Export box: the original goes to the Recycle Bin and the export takes its name, date and favourite. Glint remembers the choice."),
      t("In a project with several clips, the same option asks which of its clips the export replaces. The ticked clips go to the Recycle Bin and the others stay in your library."),
    ],
    improved: [],
    fixed: [
      t("After a project was exported, its Export button stayed greyed out until the next edit."),
    ],
  },
  {
    version: "0.1.22", date: "2026-11-15",
    new: [
      t("Editor page: pick several projects with Ctrl or Shift and a click, the tick on a card, or a box drawn over the cards. Rename or delete them together from the bar at the bottom, the right-click menu, F2 or Delete."),
    ],
    improved: [
      t("Adding clips to a project is much faster when the clips are not all alike (a capture next to an export or a Blur render). Only the clips that differ are converted; before, every clip was, and the whole project again with each add."),
      t("A 60 fps and a 120 fps capture of the same size now go into a project without any conversion."),
      t("A Blur render no longer looks stuck at 0 %: until Blur reports its first percent, Glint says “Starting Blur…” and the bar keeps moving."),
    ],
    fixed: [],
  },
  {
    version: "0.1.21", date: "2026-11-14",
    new: [],
    improved: [
      t("The title of a selection's right-click menu (“10 selected”) is set closer together."),
      t("Clip cards no longer carry a tag such as IMPORT, TRIM or DISCORD."),
    ],
    fixed: [],
  },
  {
    version: "0.1.20", date: "2026-11-14",
    new: [
      t("Theme presets: “+ New preset” saves the look you have as your own preset. Right-click a preset to rename, duplicate or delete it. Changing a color never alters the built-in themes: it makes a preset of yours instead."),
      t("Settings → Startup → When I close the window (X): keep Glint running in the tray, or quit. Quitting asks first if a render, compression or export is still running."),
    ],
    improved: [
      t("The colors and the custom CSS are in one “Customise colors” box that opens when you need it."),
      t("The clip button is simpler: your clip key as a keycap, then “Clip 30s”."),
      t("A colored folder is highlighted in its own color while it is open."),
      t("What's new: the section titles are a little bigger and sit closer to their lists."),
      t("Blur encodes on the graphics card by default on a new install, and uses the card this PC really has."),
    ],
    fixed: [
      t("Clicks that made their sound but did nothing: a button could be redrawn between the press and the release. Buttons now keep their click, and the sidebar's pages open the moment you press."),
    ],
  },
  {
    version: "0.1.19", date: "2026-11-14",
    new: [],
    improved: [
      t("Import shows Medal's own logo, used as Medal's brand page allows."),
    ],
    fixed: [],
  },
  {
    version: "0.1.18", date: "2026-11-14",
    new: [
      t("The installer is now a normal Windows setup window: choose the folder and what to install, watch the progress, and cancel at any time. Removing Glint has its own small window too."),
      t("Clip names: choose how new clips are named. Under Settings → Storage, drag Game, Date, Time, Length and your own words into any order, with a live preview."),
      t("Rename several clips at once: select them, right-click → Rename, and build the names from blocks, with numbering, a preview of every name and Undo."),
      t("Folder colors: right-click a folder → Color."),
      t("Viewer: arrows at the left and right go to the previous and the next clip (also Page Up and Page Down)."),
    ],
    improved: [
      t("Player: the volume sits on the left, as in other players. Right-click the back or forward button to set how far it jumps, from 1 to 30 seconds; Left and Right use the same step."),
      t("The clip button names its key: “F8 to clip the last 30s”."),
      t("A new Glint starts with one keybind: F8 saves the last 30 seconds. Keybinds you already have stay."),
      t("Import shows a logo in front of each program."),
      t("Settings → Match the taskbar icon to the theme has a Reload icon button, and the taskbar icon now really changes."),
    ],
    fixed: [
      t("The header of the right-click menu of a selection is lined up with its items."),
      t("Renaming a clip that was imported without copying no longer renames the other program's file."),
    ],
  },
  {
    version: "0.1.17", date: "2026-11-14",
    new: [
      t("Updates install by themselves. When a new version is out, Glint installs it and restarts in the tray, with clipping on or off as it was. It does this only while its window is closed and nothing is being rendered, exported or imported, and never while a game is running. Switch it off under Settings → Updates → Install updates automatically."),
    ],
    improved: [
      t("After any update, clipping carries on as it was: on if it was on, paused if you had paused it."),
    ],
    fixed: [],
  },
  {
    version: "0.1.16", date: "2026-11-14",
    new: [
      t("Editor: your editing sessions are now projects, kept on the new Editor page in the sidebar. Close one at any time and go on later. Add more clips from the library list beside the editor: click, Ctrl+click, Shift+click or drag a box, then double-click, press Enter or drag them onto the timeline."),
      t("Compress page in the sidebar: compress clips or any video file to the size you choose, see the resolution and bitrate each one will get before you start, and set the speed, audio, highest resolution and where the results go."),
      t("Settings → Appearance → Custom colors: set the accent, background, panels, text and more with Glint's own color picker (drag in the square, or type a hex or RGB value)."),
      t("Capture: an Apply recommended settings button picks settings from your graphics card, screen and memory, and shows each change before it is made."),
      t("Import: choose where the imported clips go: a folder for each game, one folder you name, or no folder."),
      t("A thank-you note at the bottom of Settings."),
    ],
    improved: [
      t("Library: the game and folder chips each fit on one line, with a “more” chip for the rest, and a trail beside the title shows the folder and game you are in."),
      t("Game icons: an imported game gets its real icon when the game is installed on this PC (Steam, Epic, Xbox, Riot, Ubisoft, GOG, shortcuts). Right-click a game and pick Find icon again to look once more."),
      t("Every Auto choice shows what it is using right now, such as Auto (NVIDIA NVENC)."),
    ],
    fixed: [
      t("Clips imported without copying are never moved: Sort by game and moving them to a folder now only file them in the library. Before, the other program's own files could be moved."),
      t("Imported Medal clips no longer end up in the main folder while the rest go to game folders, and a game is no longer listed twice under two spellings."),
      t("Windows' yellow capture border coming back when a game starts: Glint now waits for the new screen mode to settle before it restarts the capture, restarts it once more if the border shows, and the Capture page names the Windows setting to check."),
    ],
  },
  {
    version: "0.1.15", date: "2026-11-14",
    new: [],
    improved: [
      t("After an update Glint starts itself again directly, without a hidden PowerShell step in between. That step could make antivirus programs suspicious."),
    ],
    fixed: [],
  },
  {
    version: "0.1.14", date: "2026-11-14",
    new: [
      t("Edit together: the clips now open in the editor as separate clips, in the order you selected them. Drag a clip in the Clips strip to move it, or right-click it to move it or cut it out. Nothing is added to your library until you export."),
    ],
    improved: [
      t("Edit together keeps Desktop, Microphone and Game as separate tracks, even when the clips were recorded with different tracks."),
      t("A trim starts exactly where you set it: sound from before the start no longer comes back."),
      t("Settings → Components: ffmpeg can be installed again in one session, a failed install never leaves Glint without ffmpeg, and the replay buffer restarts only when it has to."),
    ],
    fixed: [
      t("With only the microphone and the game sound recorded, the main audio track now has both, so Discord and other players hear the game."),
      t("Settings → Components no longer stays on Installing after the window was opened again, and finds ffprobe beside an ffmpeg you chose yourself."),
      t("A clip that is waiting to be edited together can no longer be renamed or moved in the meantime."),
      t("If Blur's timing fix fails, the clip still renders."),
    ],
  },
  {
    version: "0.1.13", date: "2026-11-14",
    new: [],
    improved: [
      t("What's new: the text is set closer together."),
    ],
    fixed: [],
  },
  {
    version: "0.1.12", date: "2026-11-14",
    new: [],
    improved: [],
    fixed: [
      t("A trimmed clip that is exported again, compressed for Discord or joined no longer has its picture ahead of the sound. The trim now starts on the frame that was on screen, so it can begin a moment earlier than you asked."),
    ],
  },
  {
    version: "0.1.11", date: "2026-11-14",
    new: [
      t("Settings → Components checks for ffmpeg and Blur and installs anything missing. If ffmpeg is missing, the status bar has an Install ffmpeg button."),
      t("Edit together: select several clips, right-click, and they are joined, oldest first, into one new clip that opens in the editor with a split where each clip starts. Your original clips stay as they are."),
      t("Separate game audio track (Capture, off until you switch it on): a track with only the game's sound. In the editor, mute Desktop and keep Game to drop voice chat and music."),
      t("The taskbar icon can take your theme's color: Settings, next to the themes."),
    ],
    improved: [
      t("On Windows 10, Auto capture no longer shows the yellow border around the screen."),
      t("Clip cards: more room around the text, and the folder is shown only when it differs from the game."),
    ],
    fixed: [
      t("A trimmed clip rendered with Blur no longer has its sound out of step with the picture."),
      t("Clip thumbnails always show the whole frame, also with a custom theme and for stretched, ultrawide and portrait clips."),
    ],
  },
  {
    version: "0.1.10", date: "2026-11-13",
    new: [
      t("A keybind can switch the capture monitor: to the next monitor, or straight to one you pick. Your \"capture monitor\" clip keys, the tray and the Clip button follow it."),
    ],
    improved: [],
    fixed: [],
  },
  {
    version: "0.1.9", date: "2026-11-13",
    new: [
      t("Two keys for two monitors: in Keybinds, each clip keybind can pick its own monitor. Glint keeps a replay buffer for every monitor a keybind uses."),
    ],
    improved: [],
    fixed: [],
  },
  {
    version: "0.1.8", date: "2026-11-13",
    new: [
      t("Compress for Discord: right-click a clip and pick 10, 20, 50 or 500 MB. The smaller copy is saved next to it and put on the clipboard, ready to paste."),
      t("Interface sounds: soft sounds for clicks, switches, menus and results. They are off until you switch them on in Settings."),
      t("Trim brackets at both ends of the editor timeline: drag them to cut the start or the end of a clip."),
      t("What's new: this list. It opens once after each update, and any time from the logo or Settings → Updates."),
    ],
    improved: [
      t("The interface is drawn at a tighter size, so more fits in the window."),
    ],
    fixed: [
      t("On PCs where the graphics card's encoder could not take the capture (seen with AMD), Glint stayed at 0 fps. It now switches to a way that works by itself."),
    ],
  },
  {
    version: "0.1.7", date: "2026-11-13",
    new: [
      t("Bookmarks in the editor: right-click the timeline to add or remove one, and jump between them with N, Shift+N and Shift+P. Each clip keeps its own."),
    ],
    improved: [
      t("Every game is filed under one name (Overwatch 2 is Overwatch), and windows that are no game, such as a VPN, count as Desktop. This also applies to the clips you already have."),
      t("Minecraft Java Edition is recognised far more reliably."),
      t("The bug, update and renders buttons sit left of the search, and the renders button only shows while something is rendering."),
      t("Blur page: the comparison can be folded away, and the interpolation rates are plain lists."),
      t("The timeline's tooltips show your current shortcuts."),
    ],
    fixed: [],
  },
  {
    version: "0.1.6", date: "2026-11-13",
    new: [
      t("Report a bug from the top bar. The dialog shows everything that will be sent, and nothing leaves your PC until you press Send."),
      t("Double-click a clip's name to rename it. The file is renamed too."),
      t("A Fast render preset for Blur."),
    ],
    improved: [
      t("Folder chips run from the most clips to the fewest, and an empty Main folder is hidden unless you ask for it in Settings."),
      t("Slimmer window buttons, the sidebar toggle inside the sidebar and a clearer update button."),
      t("The status bar shows the current and highest frame rate and the memory Glint uses in total."),
    ],
    fixed: [
      t("The selection box could get stuck when the window lost the mouse."),
      t("Frames that arrived slightly early were dropped by the frame-rate limit."),
    ],
  },
  {
    version: "0.1.5", date: "2026-11-13",
    new: [
      t("Select several pieces on the editor timeline (Ctrl+click, Shift+click or drag a box) and right-click them to split, cut out, mute, solo, change the level or hide the picture."),
      t("Color correction for the selected pieces or the whole clip."),
      t("Full screen for the whole editor, and every editor shortcut can be changed in the Shortcuts panel."),
      t("\"Hide cut parts\" closes the gaps the cuts leave on the timeline."),
    ],
    improved: [
      t("The sidebar folds to icons, and Folders is one row that unfolds."),
      t("The logo moved to the top bar, and search is an icon until you use it."),
    ],
    fixed: [
      t("Esc with the timeline menu open closed the whole clip viewer."),
    ],
  },
  {
    version: "0.1.4", date: "2026-11-13",
    new: [
      t("Search the Settings page as you type. It also finds the Capture settings, which you can change right from the results."),
      t("Compact clip overlay: a saved clip shows as two stars in a small square."),
    ],
    improved: [
      t("Language is a normal Settings section with a list."),
      t("In the clip viewer, Blur links to the Blur page."),
    ],
    fixed: [],
  },
  {
    version: "0.1.3", date: "2026-11-13",
    new: [
      t("Choose where the replay buffer is kept: RAM and disk (the default), RAM only or disk only. A long buffer now needs far less memory."),
      t("Greek translation."),
    ],
    improved: [
      t("If the buffer folder cannot be written to or the drive is nearly full, the buffer stays in RAM."),
      t("The Korean translation covers the new texts."),
    ],
    fixed: [
      t("Open file location opened Documents instead of the clip's folder."),
    ],
  },
  {
    version: "0.1.2", date: "2026-11-13",
    new: [
      t("A setting to switch the daily update check off."),
    ],
    improved: [
      t("The clip overlay shows the clip length in your language."),
      t("The Korean translation was reviewed."),
    ],
    fixed: [
      t("Start with Windows now always starts the copy of Glint you are using."),
    ],
  },
  {
    version: "0.1.1", date: "2026-11-13",
    new: [],
    improved: [
      t("Clearer Korean wording in the library and the editor, and Korean text fits its buttons better."),
    ],
    fixed: [],
  },
  {
    version: "0.1.0", date: "2026-11-13",
    new: [
      t("Glint keeps the last minutes of your screen in memory and saves a clip when you press a key."),
      t("A clip library with folders, favourites and automatic game detection."),
      t("A timeline editor that exports your edit as a new clip."),
      t("Blur renders in a background queue."),
      t("Import clips from other clipping software."),
      t("Themes, a Korean translation and updates from inside the app."),
    ],
    improved: [],
    fixed: [],
  },
];

const WN_X = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6 6 18"/></svg>';
let wnCur = null;      // the open dialog: { root, back, onKey, onFocus, downOnBack }
let wnStarted = false; // wnStartup ran

/** A release date ("2026-11-13") the way the language in use writes it, e.g. "Nov 13, 2026". */
function wnDate(iso) {
  const d = new Date(`${iso}T12:00:00`);
  if (isNaN(d)) return iso;
  try { return d.toLocaleDateString(document.documentElement.lang || undefined, { year: "numeric", month: "short", day: "numeric" }); } catch { return iso; }
}

function wnHtml() {
  const group = (label, list) => (list && list.length ? `<div class="wn-grp"><div class="wn-lab">${esc(label)}</div><ul>${list.map((s) => `<li>${esc(s)}</li>`).join("")}</ul></div>` : "");
  return wnReleases().map((r) => `<section class="wn-rel">
      <div class="wn-top"><span class="wn-date">${esc(wnDate(r.date))}</span><span class="wn-ver">${esc(r.version)}</span></div>
      ${group(t("New"), r.new)}${group(t("Improved"), r.improved)}${group(t("Fixed"), r.fixed)}
    </section>`).join("");
}

function wnFocusables(root) {
  return $$('button, a[href], [tabindex]:not([tabindex="-1"])', root).filter((el) => !el.disabled && el.getClientRects().length > 0);
}

/** Open the dialog, scrolled to the newest release. Does nothing while it is already open. */
function wnOpen() {
  if (wnCur) return;
  if (typeof closeMenu === "function") closeMenu();
  const root = document.createElement("div");
  root.className = "wn-dlg";
  root.setAttribute("role", "dialog");
  root.setAttribute("aria-modal", "true");
  root.setAttribute("aria-labelledby", "wn-title");
  root.innerHTML = `<div class="wn-box">
      <div class="wn-head"><h2 id="wn-title">${esc(t("What's new"))}</h2><button type="button" class="wn-x" data-wn="close" data-sfx="close" title="${esc(t("Close"))}" aria-label="${esc(t("Close"))}">${WN_X}</button></div>
      <div class="wn-main" tabindex="0">${wnHtml()}</div>
    </div>`;
  const dlg = { root, back: document.activeElement, downOnBack: false };
  dlg.onKey = (e) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); wnClose(); }
    else if (e.key === "Tab") {
      e.preventDefault();
      const list = wnFocusables(root);
      if (!list.length) return;
      const i = list.indexOf(document.activeElement);
      list[i < 0 ? (e.shiftKey ? list.length - 1 : 0) : (i + (e.shiftKey ? -1 : 1) + list.length) % list.length].focus();
    }
  };
  // Focus that wanders out of the dialog is brought back.
  dlg.onFocus = (e) => { if (wnCur === dlg && !root.contains(e.target)) $(".wn-x", root)?.focus({ preventScroll: true }); };
  window.addEventListener("keydown", dlg.onKey, true);
  document.addEventListener("focusin", dlg.onFocus, true);
  // A click on the dimmed area closes it (only one that also started there, so a drag out of the text does not).
  root.addEventListener("mousedown", (e) => { dlg.downOnBack = e.target === root; });
  root.addEventListener("click", (e) => {
    if (e.target === root) { if (dlg.downOnBack) wnClose(); return; }
    if (e.target.closest('[data-wn="close"]')) wnClose();
  });
  // In full screen only the full-screen element is drawn, so the dialog goes inside it (a <video> cannot hold it).
  const fs = document.fullscreenElement;
  (fs && !(fs instanceof HTMLMediaElement) ? fs : document.body).appendChild(root);
  wnCur = dlg;
  $(".wn-main", root).scrollTop = 0;
  $(".wn-x", root).focus({ preventScroll: true });
}

function wnClose() {
  const dlg = wnCur;
  if (!dlg) return;
  wnCur = null;
  window.removeEventListener("keydown", dlg.onKey, true);
  document.removeEventListener("focusin", dlg.onFocus, true);
  dlg.root.remove(); // the listeners on the dialog itself go with it
  if (dlg.back && dlg.back.isConnected && dlg.back !== document.body) dlg.back.focus?.({ preventScroll: true });
}

/**
 * Once per start, after S.version is known: open What's new when Glint was updated since it was
 * last seen. A first run (nothing stored yet) only stores the version.
 */
function wnStartup() {
  if (wnStarted || typeof S === "undefined" || !S.version) return;
  wnStarted = true;
  let seen = null;
  try { seen = localStorage.getItem("glint.seenVersion"); localStorage.setItem("glint.seenVersion", S.version); } catch { return; }
  if (seen && seen !== S.version) wnOpen();
}

// The logo is a button for the keyboard too (role="button" in index.html).
document.getElementById("ver")?.addEventListener("keydown", (e) => {
  if (e.key !== "Enter" && e.key !== " ") return;
  e.preventDefault();
  wnOpen();
});
// app.js calls wnStartup() after its first refreshState; if that already happened, run it now.
wnStartup();
