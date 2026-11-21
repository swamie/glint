// Builds the marketplace's list of official plugins: plugins/index.json.
//   node tools/plugin-index.js            write plugins/index.json
//   node tools/plugin-index.js --check    exit 1 if plugins/index.json is not up to date (nothing is written)
//
// Every folder in plugins/official/ with a plugin.json is one plugin. Its entry carries what the
// marketplace shows (from plugin.json, plus "icon" and "tag" from the optional market.json beside
// it) and every file of the folder with its size and SHA-256. Glint installs exactly those files and
// checks each one against this list (src-tauri/src/plugins.rs), so run this after any change to a
// plugin and commit the result with it. Raise the plugin's "version" too: that is what tells an
// installed copy there is an update.
const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const root = path.join(__dirname, "..", "plugins");
const official = path.join(root, "official");
const out = path.join(root, "index.json");
const ENDINGS = [".js", ".css", ".json", ".svg", ".png", ".md", ".txt", ".html"]; // what plugins.rs will install

function files(dir, rel = "") {
  return fs.readdirSync(path.join(dir, rel), { withFileTypes: true }).flatMap((e) => {
    const r = rel ? `${rel}/${e.name}` : e.name;
    if (e.name.startsWith(".")) return [];
    if (e.isDirectory()) return files(dir, r);
    return r === "market.json" ? [] : [r];
  }).sort();
}

const plugins = fs.readdirSync(official, { withFileTypes: true }).filter((e) => e.isDirectory()).map((e) => e.name).sort().flatMap((id) => {
  const dir = path.join(official, id);
  if (!fs.existsSync(path.join(dir, "plugin.json"))) return [];
  const m = JSON.parse(fs.readFileSync(path.join(dir, "plugin.json"), "utf8"));
  const market = fs.existsSync(path.join(dir, "market.json")) ? JSON.parse(fs.readFileSync(path.join(dir, "market.json"), "utf8")) : {};
  if (m.id !== id) throw new Error(`${id}/plugin.json: "id" must be the folder's name`);
  if (!/^[a-z0-9][a-z0-9-]{0,39}$/.test(id)) throw new Error(`${id}: a plugin's id is lower-case letters, digits and dashes`);
  if (!/^\d+\.\d+\.\d+$/.test(m.version || "")) throw new Error(`${id}/plugin.json: "version" must look like 1.2.3`);
  const list = files(dir).map((rel) => {
    if (!ENDINGS.some((x) => rel.endsWith(x)) || !/^[A-Za-z0-9._\/-]+$/.test(rel)) throw new Error(`${id}/${rel}: Glint does not install this kind of file`);
    // Hashed as the bytes git serves: a checkout that turned line ends into CRLF would give other sums.
    const data = fs.readFileSync(path.join(dir, rel));
    if (data.includes(13)) throw new Error(`${id}/${rel}: has CRLF line ends; plugin files must use LF (see .gitattributes)`);
    if (!data.length) throw new Error(`${id}/${rel}: is empty`);
    return { path: rel, sha256: crypto.createHash("sha256").update(data).digest("hex"), size: data.length };
  });
  return [{ id, name: m.name || id, description: m.description || "", author: m.author || "Glint developers", version: m.version, min_glint: m.min_glint || "", icon: market.icon || "", tag: market.tag || "", files: list }];
});

const text = JSON.stringify({ schema: 1, plugins }, null, 2) + "\n";
if (process.argv.includes("--check")) {
  const same = fs.existsSync(out) && fs.readFileSync(out, "utf8") === text;
  console.log(same ? "plugins/index.json is up to date" : "plugins/index.json is OUT OF DATE: run node tools/plugin-index.js");
  process.exit(same ? 0 : 1);
}
fs.writeFileSync(out, text);
console.log(`plugins/index.json: ${plugins.map((p) => `${p.id} ${p.version} (${p.files.length} files)`).join(", ") || "no plugins"}`);
