"use strict";
// Which files hold each language's texts, and the loading of them.
//
// Every language used to be loaded at every start (138 scripts, about 1 MB, before the first
// paint). Now only the language in use is: its files are written into the page right here, so
// the first paint is still in the right language, and another language is fetched when it is
// chosen (glintLoadLanguage). A new lang file must be added to its list below.
window.GLINT_LANG_FILES = {
  ko: [
    "ko-app.js",
    "ko-editor.js",
    "ko-update.js",
    "ko-buffer.js",
    "ko-editor2.js",
    "ko-timeline2.js",
    "ko-shell.js",
    "ko-bug.js",
    "ko-marks.js",
    "ko-ui3.js",
    "ko-trim.js",
    "ko-mon.js",
    "ko-rel11.js",
    "ko-gameaudio.js",
    "ko-deps.js",
    "ko-ui4.js",
    "ko-join.js",
    "ko-join2b.js",
    "ko-join2.js",
    "ko-parts.js",
    "ko-lib2.js",
    "ko-icons.js",
    "ko-cap2.js",
    "ko-proj.js",
    "ko-projb.js",
    "ko-comp.js",
    "ko-color.js",
    "ko-name2.js",
    "ko-ui5.js",
    "ko-preset.js",
    "ko-ui6.js",
    "ko-rules.js",
    "ko-gif.js",
    "ko-share.js",
    "ko-upscale.js",
    "ko-drop.js",
    "ko-tl3.js",
    "ko-ovl.js",
    "ko-cv.js",
    "ko-rc.js",
    "ko-kf.js",
    "ko-aud.js",
    "ko-cap.js",
    "ko-tl4.js",
    "ko-tl5.js",
    "ko-tl6.js",
    "ko-pj2.js",
    "ko-tl7.js",
    "ko-tl8.js",
    "ko-pj3.js",
    "ko-tl9.js",
    "ko-zm1.js",
    "ko-sp1.js",
    "ko-ex2.js",
    "ko-rw1.js",
    "ko-lk1.js",
    "ko-ws2.js",
    "ko-med2.js",
    "ko-med.js",
    "ko-ws.js",
    "ko-fb.js",
    "ko-gd1.js",
    "ko-gd2.js",
    "ko-gd3.js",
    "ko-pl1.js",
    "ko-plg.js",
    "ko-th1.js",
    "ko-bd1.js",
    "ko-nw1.js",
    "ko-lw1.js",
  ],
  el: [
    "el-app.js",
    "el-editor.js",
    "el-update.js",
    "el-buffer.js",
    "el-editor2.js",
    "el-timeline2.js",
    "el-shell.js",
    "el-bug.js",
    "el-marks.js",
    "el-ui3.js",
    "el-trim.js",
    "el-mon.js",
    "el-rel11.js",
    "el-gameaudio.js",
    "el-deps.js",
    "el-ui4.js",
    "el-join.js",
    "el-join2b.js",
    "el-join2.js",
    "el-parts.js",
    "el-lib2.js",
    "el-icons.js",
    "el-cap2.js",
    "el-proj.js",
    "el-projb.js",
    "el-comp.js",
    "el-color.js",
    "el-name2.js",
    "el-ui5.js",
    "el-preset.js",
    "el-ui6.js",
    "el-rules.js",
    "el-gif.js",
    "el-share.js",
    "el-upscale.js",
    "el-drop.js",
    "el-tl3.js",
    "el-ovl.js",
    "el-cv.js",
    "el-rc.js",
    "el-kf.js",
    "el-aud.js",
    "el-cap.js",
    "el-tl4.js",
    "el-tl5.js",
    "el-tl6.js",
    "el-pj2.js",
    "el-tl7.js",
    "el-tl8.js",
    "el-pj3.js",
    "el-tl9.js",
    "el-zm1.js",
    "el-sp1.js",
    "el-ex2.js",
    "el-rw1.js",
    "el-lk1.js",
    "el-ws2.js",
    "el-med2.js",
    "el-med.js",
    "el-ws.js",
    "el-fb.js",
    "el-gd1.js",
    "el-gd2.js",
    "el-gd3.js",
    "el-pl1.js",
    "el-plg.js",
    "el-th1.js",
    "el-bd1.js",
    "el-nw1.js",
    "el-lw1.js",
  ],
};
// The names are known without the texts: Settings → Language lists every language.
glintAddLanguage("ko", "한국어", {});
glintAddLanguage("el", "Ελληνικά", {});

/** code -> a promise that settles once that language's files have run. */
const GLINT_LANG_LOADS = { en: Promise.resolve() };

/** Make sure the texts of language `code` are loaded. Resolves at once for one that is. */
function glintLoadLanguage(code) {
  if (GLINT_LANG_LOADS[code]) return GLINT_LANG_LOADS[code];
  const files = window.GLINT_LANG_FILES[code];
  if (!files) return Promise.resolve();
  return (GLINT_LANG_LOADS[code] = Promise.all(files.map((f) => new Promise((done) => {
    const s = document.createElement("script");
    s.src = "lang/" + f;
    s.async = false; // in the order of the list: a later file may add to an earlier one's texts
    s.onload = s.onerror = () => done(); // a file that is missing leaves its texts in English
    document.head.appendChild(s);
  }))));
}

// The language remembered from last time (i18n.js), in time for the first paint.
if (window.GLINT_LANG_FILES[glintLang]) {
  document.write(window.GLINT_LANG_FILES[glintLang].map((f) => `<script src="lang/${f}"><\/script>`).join(""));
  GLINT_LANG_LOADS[glintLang] = Promise.resolve();
}
