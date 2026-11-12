"use strict";
// Translation hook, loaded before every other script.
//
//   t("Clip saved")                                  -> the text in the current language
//   t("Moved {n} clips to {folder}", { n, folder })  -> placeholders filled after translating
//
// The English text is the key. A language is a dictionary from English text to
// its translation (ui/lang/<code>.js calls glintAddLanguage). A text with no
// entry is shown in English, so a missing translation never breaks the page.

window.GLINT_LANGS = { en: { name: "English", dict: {} } };
let glintLang = "en";

/** Register a language (or add more texts to it): glintAddLanguage("ko", "한국어", { "Clip saved": "클립 저장됨", ... }). */
function glintAddLanguage(code, name, dict) {
  // A language may be split over several files: later calls add to the same dictionary.
  const lang = window.GLINT_LANGS[code] || (window.GLINT_LANGS[code] = { name, dict: {} });
  lang.name = name;
  Object.assign(lang.dict, dict);
}

/** Switch language. Returns the code now in use ("en" if `code` is unknown). */
function glintSetLanguage(code) {
  glintLang = window.GLINT_LANGS[code] ? code : "en";
  document.documentElement.lang = glintLang;
  try { localStorage.setItem("glint.lang", glintLang); } catch {}
  return glintLang;
}

function t(text, vars) {
  const dict = window.GLINT_LANGS[glintLang]?.dict;
  let s = (dict && dict[text]) || text;
  if (vars) s = s.replace(/\{(\w+)\}/g, (whole, key) => (key in vars ? String(vars[key]) : whole));
  return s;
}

// Remembered so the first paint is already in the right language.
try { glintLang = localStorage.getItem("glint.lang") || "en"; } catch {}
