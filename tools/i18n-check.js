// Translation coverage check for Glint's UI.
//   node tools/i18n-check.js <ui folder> [language code, default ko] [file.js ...]
// Lists every text passed to t("...") in the given UI scripts (default: all of
// them), and for one language's dictionary (ui/lang/<code>-*.js) reports:
//   MISSING   a text used in the code with no Korean entry
//   EMPTY     an entry whose Korean text is empty
//   PLACEHOLDER  an entry whose {placeholders} differ from the English text
//   SAME      an entry identical to the English (fine for names, suspicious for sentences)
//   UNUSED    an entry no script uses (stale, or a typo in the key)
//   DYNAMIC   a t(...) call whose argument is not a plain string (cannot be checked)
//   SHADOW    a local variable or parameter named t, which hides the t() function
const fs = require("fs");
const path = require("path");
const vm = require("vm");
const ui = process.argv[2];
const code = /^[a-z]{2}$/.test(process.argv[3] || "") ? process.argv[3] : "ko";
const only = process.argv.slice(/^[a-z]{2}$/.test(process.argv[3] || "") ? 4 : 3);
const scripts = (only.length ? only : fs.readdirSync(ui).filter((f) => f.endsWith(".js") && !["mock.js", "i18n.js", "editmodel.js"].includes(f))).map((f) => path.join(ui, f));

// Korean dictionary: run the lang files with a stub glintAddLanguage.
const dict = {};
const dups = [];
const langDir = path.join(ui, "lang");
for (const f of fs.existsSync(langDir) ? fs.readdirSync(langDir).filter((n) => n.startsWith(code + "-") && n.endsWith(".js")) : []) {
  const ctx = { glintAddLanguage: (code, name, d) => { for (const [k, v] of Object.entries(d)) { if (k in dict && dict[k] !== v) dups.push(`${k}  =>  "${dict[k]}"  vs  "${v}" (${f})`); dict[k] = v; } } };
  vm.runInNewContext(fs.readFileSync(path.join(langDir, f), "utf8"), ctx, { filename: f });
}

const used = new Map(); // text -> first place
const dynamic = [];
const shadow = [];
for (const file of scripts) {
  const src = fs.readFileSync(file, "utf8");
  const name = path.basename(file);
  const lineOf = (i) => src.slice(0, i).split("\n").length;
  // t( followed by a string literal, then , or )
  const re = /(?<![\w$.])t\(\s*(?:"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|`((?:[^`\\$]|\\.)*)`)\s*[,)]/g;
  let m;
  while ((m = re.exec(src))) {
    const raw = m[1] ?? m[2] ?? m[3];
    let text;
    try { text = JSON.parse('"' + raw.replace(/\\'/g, "'").replace(/(?<!\\)"/g, '\\"') + '"'); } catch { text = raw; }
    if (!used.has(text)) used.set(text, `${name}:${lineOf(m.index)}`);
  }
  const reAny = /(?<![\w$.])t\(\s*([^"'`\s)])/g;
  while ((m = reAny.exec(src))) dynamic.push(`${name}:${lineOf(m.index)}  t(${src.slice(m.index + 2, m.index + 42).split("\n")[0]}`);
  const reShadow = /\b(?:const|let|var)\s+t\b|\(\s*t\s*[,)=]|,\s*t\s*[,)=]|\bt\s*=>|\bfunction\s+\w*\s*\([^)]*\bt\b[^)]*\)/g;
  while ((m = reShadow.exec(src))) shadow.push(`${name}:${lineOf(m.index)}  ${src.slice(m.index, m.index + 50).split("\n")[0]}`);
}

const holders = (s) => (s.match(/\{\w+\}/g) || []).sort().join(",");
const out = { MISSING: [], EMPTY: [], PLACEHOLDER: [], SAME: [], UNUSED: [], DYNAMIC: dynamic, SHADOW: shadow, CONFLICT: dups };
for (const [text, where] of used) {
  if (!(text in dict)) out.MISSING.push(`${where}  ${JSON.stringify(text)}`);
  else if (!dict[text]) out.EMPTY.push(JSON.stringify(text));
  else if (holders(text) !== holders(dict[text])) out.PLACEHOLDER.push(`${JSON.stringify(text)}  =>  ${JSON.stringify(dict[text])}`);
  else if (dict[text] === text && /[a-z]{3,} [a-z]{3,}/i.test(text)) out.SAME.push(JSON.stringify(text));
}
if (!only.length) for (const k of Object.keys(dict)) if (!used.has(k)) out.UNUSED.push(JSON.stringify(k));
console.log(`texts in code: ${used.size}   ${code} entries: ${Object.keys(dict).length}`);
for (const [kind, list] of Object.entries(out)) {
  console.log(`\n${kind}: ${list.length}`);
  for (const l of list.slice(0, 400)) console.log("  " + l);
}
