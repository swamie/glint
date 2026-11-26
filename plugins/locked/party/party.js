// Party: what is playing in the top bar, and listening to it together with friends.
// A Glint plugin that is not in the marketplace (plugins/unlisted/party in Glint's repository): it is installed by hand.
//
// Now playing: Glint asks Windows what is playing (ctx.media.now: Spotify, YouTube Music or any
// site in a browser, Media Player and other music players) and this shows it in the top bar.
//
// A party: the host presses Start and gets a six character code. A friend with this plugin types
// the code and hears what the host's music player plays, live.
//
//   - The music goes straight from the host's PC to each listener's (WebRTC, encrypted between the
//     two). Glint's server (server/share, the /party routes) only passes the handshake: who wants
//     in, and how the two PCs reach each other. If the server names a relay (TURN) the music may
//     pass through it, still encrypted, when the two PCs cannot reach each other directly.
//   - The host's sound is the music player's own (ctx.audio: that program only, so a voice chat or
//     a game is not sent), taken only while at least one listener is connected, and never saved.
//   - Everything lives in Glint's window. While a party is on, the X button hides the window
//     instead of closing it (ctx.keepOpen), so the party carries on from the tray.
//
// The handshake is one offer and one answer per listener, each sent whole once the PC knows its own
// addresses, so a listener costs the server two small writes. After that the two talk over their
// own data channel: the host's track, the listener count, mute, kick, the end of the party.
const { ctx, t, esc } = Glint;
const SERVER = "https://clips.glintclip.com";
const CODE_RE = /^[A-HJKMNP-Z2-9]{6}$/;
const MAX_CAP = 8;
const BITRATE = 192000;
const ICON = '<svg viewBox="0 0 24 24"><path d="M9 18V5l12-2v13M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM21 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z"/></svg>';
const DEFAULTS = { name: "", show_np: true, source: "auto", cap: 5, volume: 80, agreed: false, peer: "", server: "" };

const P = {
  np: null,          // what this PC plays: { found, title, artist, album, app, exe, playing, source }
  art: "", artKey: "",
  small: "", smallKey: "", // the cover made small, for listeners
  role: "",          // "", "host" or "listener"
  busy: "",          // "start" or "join" while one is under way
  // host
  code: "", key: "", cap: 5, ice: null, iceAt: 0,
  peers: new Map(),  // peer id -> { id, name, pc, dc, sender, state, muted, seen, since }
  banned: new Set(),
  answering: new Set(),
  muteAll: false,
  au: null,          // { ctx, node, dest, track }
  tap: { source: "", state: "", error: "", level: 0, at: 0 },
  // listener
  j: null,           // { code, state, host, np, art, count, pc, dc, el, mutedByHost, muted, seen, tries, retryAt, error, gen }
  apps: null,        // the programs that play sound, for the "what friends hear" list
  last: {},          // when each regular job last ran
  tickAt: 0, waiters: [],
};

/* ---------- small things ---------- */

const clip = (v, n) => String(v ?? "").replace(/[\u0000-\u001f\u007f]/g, " ").trim().slice(0, n);
const hex = (n) => [...crypto.getRandomValues(new Uint8Array(n))].map((b) => b.toString(16).padStart(2, "0")).join("");
const server = (c) => String(c.s.server || SERVER).replace(/\/+$/, "");
const myName = (c) => clip(c.s.name, 24) || clip(c.glint.settings?.share?.name, 24) || t("A friend");
const showCode = (code) => `${code.slice(0, 3)} ${code.slice(3)}`;
const artOk = (v) => typeof v === "string" && v.length < 300000 && /^data:image\/(jpeg|png|webp|gif|svg\+xml);base64,[A-Za-z0-9+/=]+$/.test(v);
const due = (what, ms) => { const now = Date.now(); if (now - (P.last[what] || 0) < ms) return false; P.last[what] = now; return true; };
/** Wait for `n` ticks of the clock (about a second each). Unlike a timer it keeps its pace while the window is hidden. */
const ticks = (n) => new Promise((done) => P.waiters.push({ n, done }));

async function api(c, method, path, body) {
  let r;
  try {
    r = await fetch(server(c) + path, { method, body: body === undefined ? undefined : JSON.stringify(body), headers: body === undefined ? {} : { "Content-Type": "application/json" }, cache: "no-store" });
  } catch {
    const e = new Error(t("The party server could not be reached. Check the internet connection."));
    e.status = 0;
    throw e;
  }
  let j = null;
  try { j = await r.json(); } catch {}
  if (!r.ok) {
    const e = new Error(r.status === 404 ? t("There is no party with this code. It may have ended.") : t((j && j.error) || "The party server answered with an error."));
    e.status = r.status;
    throw e;
  }
  return j || {};
}

/** Opus as music: stereo and a higher bitrate than a voice call's. */
function tuneOpus(sdp) {
  const m = /a=rtpmap:(\d+) opus\/48000\/2/i.exec(sdp);
  if (!m) return sdp;
  const pt = m[1], extra = ["stereo=1", "sprop-stereo=1", `maxaveragebitrate=${BITRATE}`, "maxplaybackrate=48000", "usedtx=0"];
  const re = new RegExp(`a=fmtp:${pt} ([^\\r\\n]*)`);
  if (re.test(sdp)) return sdp.replace(re, (_, cur) => `a=fmtp:${pt} ${cur.split(";").map((x) => x.trim()).filter((x) => x && !/^(stereo|sprop-stereo|maxaveragebitrate|maxplaybackrate|usedtx)=/.test(x)).concat(extra).join(";")}`);
  return sdp.replace(m[0], `${m[0]}\r\na=fmtp:${pt} ${extra.join(";")}`);
}

/** Resolves when the connection knows its own addresses, or after `ms`. */
function gathered(pc, ms = 2500) {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((done) => {
    const end = () => { pc.removeEventListener("icegatheringstatechange", on); clearTimeout(timer); done(); };
    const on = () => { if (pc.iceGatheringState === "complete") end(); };
    const timer = setTimeout(end, ms);
    pc.addEventListener("icegatheringstatechange", on);
  });
}

async function iceServers(c, code) {
  if (P.ice && Date.now() - P.iceAt < 3600e3) return P.ice;
  const j = await api(c, "GET", `/party/${code}/ice`);
  P.ice = Array.isArray(j.iceServers) ? j.iceServers : [];
  P.iceAt = Date.now();
  return P.ice;
}

const send = (dc, msg) => { try { if (dc && dc.readyState === "open") dc.send(JSON.stringify(msg)); } catch {} };

/* ---------- what is playing ---------- */

function pollNow(c) {
  if (P.npBusy) return;
  P.npBusy = true;
  c.media.now(P.artKey).then((n) => {
    const before = npKey(P.np);
    if (typeof n.art === "string") { P.art = artOk(n.art) ? n.art : ""; P.artKey = n.art_key || ""; }
    else if (!n.art_key) { P.art = ""; P.artKey = ""; }
    P.np = n.found ? { title: clip(n.title, 200), artist: clip(n.artist, 200), album: clip(n.album, 200), app: clip(n.app, 40), exe: clip(n.exe, 80).toLowerCase(), playing: !!n.playing, source: n.source, ...place(n.pos_ms, n.dur_ms) } : null;
    if (npKey(P.np) !== before || P.smallKey !== P.artKey) { smallArt().then(() => { if (P.role === "host") tellNow(); }); changed(c); }
    else if (P.role === "host" && jumped()) tellNow(); // a seek, or the clocks drifted apart
    progress();
    if (P.role === "host") syncTap(c);
  }).catch(() => {}).finally(() => { P.npBusy = false; });
}
const npKey = (n) => (n ? `${n.title}|${n.artist}|${n.app}|${n.playing}` : "");

/** Where a track is: { pos, dur } in ms and `at`, the moment `pos` was true. dur 0 means the player does not say. */
function place(pos, dur) {
  pos = Number(pos); dur = Number(dur);
  if (!(dur > 0) || dur > 864e5 || !(pos >= 0)) return { pos: 0, dur: 0, at: Date.now() };
  return { pos: Math.min(pos, dur), dur, at: Date.now() };
}
/** Where it is right now, or -1 when that is not known. */
const posNow = (n) => (n && n.dur > 0 ? Math.min(n.dur, n.pos + (n.playing ? Date.now() - n.at : 0)) : -1);
/** Is the host's track somewhere else than the listeners were last told? */
function jumped() {
  const a = P.told, b = P.np;
  if (!a || !b) return false;
  return a.dur !== b.dur || Math.abs(posNow(a) - posNow(b)) > 1500;
}
const clock = (ms) => { const s = Math.max(0, Math.floor(ms / 1000)), h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60; return `${h ? `${h}:${String(m).padStart(2, "0")}` : m}:${String(s % 60).padStart(2, "0")}`; };

/** The cover as a small JPEG: what listeners get. */
async function smallArt() {
  if (P.smallKey === P.artKey) return;
  const key = P.artKey, src = P.art;
  let out = "";
  if (src) {
    try {
      const img = new Image();
      await new Promise((ok, bad) => { img.onload = ok; img.onerror = bad; img.src = src; });
      const cv = document.createElement("canvas");
      cv.width = cv.height = 128;
      const g = cv.getContext("2d"), s = Math.min(img.naturalWidth, img.naturalHeight) || 1;
      g.drawImage(img, (img.naturalWidth - s) / 2, (img.naturalHeight - s) / 2, s, s, 0, 0, 128, 128);
      out = cv.toDataURL("image/jpeg", 0.82);
    } catch { out = ""; }
  }
  if (key === P.artKey) { P.small = out; P.smallKey = key; }
}

const nowMsg = () => ({ t: "np", np: P.np ? { title: P.np.title, artist: P.np.artist, album: P.np.album, app: P.np.app, playing: P.np.playing, pos: Math.max(0, Math.round(posNow(P.np))), dur: P.np.dur } : null, art: P.np ? P.small : "" });
function tellNow(only) {
  const msg = nowMsg();
  P.told = P.np ? { ...P.np } : null;
  for (const p of only ? [only] : P.peers.values()) send(p.dc, msg);
}

/* ---------- hosting ---------- */

const WORKLET = `
class PartyFeed extends AudioWorkletProcessor {
  constructor() {
    super();
    this.size = 48000; this.l = new Float32Array(this.size); this.r = new Float32Array(this.size);
    this.w = 0; this.rd = 0; this.fill = 0; this.going = false;
    this.port.onmessage = (e) => {
      const pcm = new Int16Array(e.data), n = pcm.length >> 1;
      for (let i = 0; i < n; i++) {
        this.l[this.w] = pcm[i * 2] / 32768; this.r[this.w] = pcm[i * 2 + 1] / 32768;
        this.w = (this.w + 1) % this.size;
      }
      this.fill += n;
      // More than a quarter second waiting: the oldest goes, so the delay never grows.
      if (this.fill > 12000) { const drop = this.fill - 4800; this.rd = (this.rd + drop) % this.size; this.fill -= drop; }
    };
  }
  process(_, outputs) {
    const out = outputs[0], n = out[0].length;
    if (!this.going && this.fill >= 3360) this.going = true; // 70 ms in hand before it starts
    if (!this.going) return true;
    const take = Math.min(n, this.fill);
    for (let i = 0; i < take; i++) {
      out[0][i] = this.l[this.rd]; if (out[1]) out[1][i] = this.r[this.rd];
      this.rd = (this.rd + 1) % this.size;
    }
    this.fill -= take;
    if (take < n) this.going = false; // ran dry (the music stopped): silence until there is enough again
    return true;
  }
}
registerProcessor("party-feed", PartyFeed);`;

async function audioOpen() {
  if (P.au) return P.au;
  const ac = new AudioContext({ sampleRate: 48000, latencyHint: "interactive" });
  const url = URL.createObjectURL(new Blob([WORKLET], { type: "text/javascript" }));
  try { await ac.audioWorklet.addModule(url); } finally { URL.revokeObjectURL(url); }
  const node = new AudioWorkletNode(ac, "party-feed", { numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [2] });
  const dest = new MediaStreamAudioDestinationNode(ac, { channelCount: 2, channelCountMode: "explicit" });
  node.connect(dest);
  const track = dest.stream.getAudioTracks()[0];
  try { track.contentHint = "music"; } catch {}
  ac.resume().catch(() => {});
  P.au = { ctx: ac, node, dest, track };
  return P.au;
}
function audioClose() {
  if (!P.au) return;
  try { P.au.track.stop(); P.au.node.disconnect(); P.au.ctx.close(); } catch {}
  P.au = null;
}

/** The program whose sound friends hear: the setting, or the player of the track that is on. "" = none yet. */
function wantedSource(c) {
  const s = String(c.s.source || "auto");
  if (s === "all") return "all";
  if (s !== "auto") return s;
  return (P.np && P.np.exe) || P.tap.auto || "";
}
const listening = () => [...P.peers.values()].filter((p) => p.state === "connected").length;

/** The sound is taken only while someone is there to hear it. */
function syncTap(c) {
  if (P.role !== "host") return;
  if (c.s.source === "auto" && P.np && P.np.exe && P.np.playing) P.tap.auto = P.np.exe;
  const want = listening() > 0 && !P.muteAll ? wantedSource(c) : "";
  if (want === P.tap.source) return;
  P.tap.source = want;
  P.tap.error = "";
  if (!want) { P.tap.state = ""; c.audio.stop().catch(() => {}); return changed(c); }
  P.tap.state = "starting";
  c.audio.start(want, (pcm) => {
    if (!P.au || P.tap.source !== want) return;
    let peak = 0;
    for (let i = 0; i < pcm.length; i += 16) { const v = pcm[i] < 0 ? -pcm[i] : pcm[i]; if (v > peak) peak = v; }
    P.tap.level = peak / 32768;
    P.tap.at = Date.now();
    const copy = pcm.slice(); // its own buffer, handed over to the audio thread
    P.au.node.port.postMessage(copy.buffer, [copy.buffer]);
  }).catch((e) => { if (P.tap.source === want) { P.tap.state = "failed"; P.tap.error = String(e?.message ?? e); changed(c); } });
  changed(c);
}

async function startParty(c) {
  if (P.role || P.busy) return;
  if (!c.s.agreed) {
    const yes = await c.confirm({
      title: t("Before you start a party"),
      text: t("Friends who join hear what your music player plays, live. The sound goes straight from your PC to theirs, so they can see your IP address, as in any direct call. Nothing is recorded. Music services do not allow sharing their music with others this way, so keep parties to a few friends."),
      ok: t("Start party"),
    });
    if (yes !== "yes") return;
    c.s.agreed = true;
    c.save();
  }
  P.busy = "start";
  changed(c);
  try {
    await audioOpen();
    const cap = Math.min(MAX_CAP, Math.max(1, Number(c.s.cap) || 5));
    const j = await api(c, "POST", "/party", { name: myName(c), cap });
    if (!CODE_RE.test(String(j.code || "")) || !j.key) throw new Error(t("The party server answered with an error."));
    Object.assign(P, { role: "host", code: j.code, key: j.key, cap, ice: null, muteAll: false, codeDead: false, pollFails: 0 });
    P.peers.clear(); P.banned.clear(); P.answering.clear();
    P.tap = { source: "", state: "", error: "", level: 0, at: 0, auto: (P.np && P.np.exe) || "" };
    c.keepOpen(true).catch(() => {});
    iceServers(c, P.code).catch(() => {});
  } catch (e) {
    audioClose();
    c.fail(e.message || e);
  }
  P.busy = "";
  changed(c);
}

function endParty(c, quiet) {
  if (P.role !== "host") return;
  for (const p of P.peers.values()) { send(p.dc, { t: "end" }); const pc = p.pc; setTimeout(() => { try { pc.close(); } catch {} }, 300); }
  P.peers.clear();
  const { code, key } = P;
  fetch(`${server(c)}/party/${code}?key=${key}`, { method: "DELETE", keepalive: true }).catch(() => {});
  P.role = ""; P.code = ""; P.key = "";
  P.tap.source = "";
  c.audio.stop().catch(() => {});
  audioClose();
  c.keepOpen(false).catch(() => {});
  if (!quiet) c.flash(t("The party is over"));
  changed(c);
}

function hostPoll(c) {
  if (P.pollBusy) return;
  P.pollBusy = true;
  const code = P.code;
  api(c, "GET", `/party/${code}?key=${P.key}`).then((j) => {
    P.pollFails = 0;
    if (P.role !== "host" || P.code !== code) return;
    for (const o of j.offers || []) if (o && /^[0-9a-f]{32}$/.test(o.peer) && !P.answering.has(o.peer)) answer(c, o);
  }).catch((e) => {
    // The server forgot the party (it was idle too long, or ended elsewhere): listeners already in stay, nobody new can join.
    if (e.status === 404 || e.status === 403) { c.flash(t("The party's code has expired. Start a new party to let more friends in."), true); P.codeDead = true; }
    P.pollFails = (P.pollFails || 0) + 1;
  }).finally(() => { P.pollBusy = false; });
}

async function answer(c, o) {
  const id = o.peer;
  P.answering.add(id);
  const refuse = (why) => api(c, "POST", `/party/${P.code}/answer?key=${P.key}`, { peer: id, refused: why }).catch(() => {});
  try {
    if (P.banned.has(id)) return await refuse("kicked");
    const others = [...P.peers.keys()].filter((k) => k !== id).length;
    if (others >= P.cap) return await refuse("full");
    dropPeer(c, id, true); // the same friend joining again replaces the old connection
    const au = await audioOpen();
    const pc = new RTCPeerConnection({ iceServers: await iceServers(c, P.code).catch(() => []) });
    const p = { id, name: clip(o.name, 24) || t("A friend"), pc, dc: null, sender: null, state: "joining", muted: false, seen: Date.now(), since: Date.now() };
    P.peers.set(id, p);
    pc.ondatachannel = (e) => {
      p.dc = e.channel;
      p.dc.onopen = () => { send(p.dc, { t: "hello", host: myName(c), n: listening(), cap: P.cap, muted: p.muted || P.muteAll }); tellNow(p); };
      p.dc.onmessage = (m) => {
        p.seen = Date.now();
        let msg = null;
        try { msg = typeof m.data === "string" && m.data.length < 2000 ? JSON.parse(m.data) : null; } catch {}
        if (!msg) return;
        if (msg.t === "ping") send(p.dc, { t: "pong" });
        if (msg.t === "bye") dropPeer(c, id);
      };
    };
    pc.onconnectionstatechange = () => {
      if (P.peers.get(id) !== p) return;
      const s = pc.connectionState;
      if (s === "connected") { p.state = "connected"; p.seen = Date.now(); peersChanged(c); }
      else if (s === "disconnected") { p.state = "unsteady"; peersChanged(c); }
      else if (s === "failed" || s === "closed") dropPeer(c, id);
    };
    await pc.setRemoteDescription({ type: "offer", sdp: String(o.sdp) });
    p.sender = pc.addTrack(au.track, au.dest.stream);
    const ans = await pc.createAnswer();
    await pc.setLocalDescription({ type: "answer", sdp: tuneOpus(ans.sdp) });
    try {
      const prm = p.sender.getParameters();
      if (prm.encodings && prm.encodings[0]) { prm.encodings[0].maxBitrate = BITRATE; await p.sender.setParameters(prm); }
    } catch {}
    await gathered(pc);
    if (P.peers.get(id) !== p) return;
    await api(c, "POST", `/party/${P.code}/answer?key=${P.key}`, { peer: id, sdp: pc.localDescription.sdp });
  } catch (e) {
    console.warn("Glint party: could not answer a listener", e);
    dropPeer(c, id, true);
  } finally {
    P.answering.delete(id);
    changed(c);
  }
}

function dropPeer(c, id, quiet) {
  const p = P.peers.get(id);
  if (!p) return;
  P.peers.delete(id);
  try { p.pc.close(); } catch {}
  if (!quiet) peersChanged(c);
}

/** The listeners changed: everyone hears the new count, and the sound starts or stops with the first and last one. */
function peersChanged(c) {
  const n = listening();
  for (const p of P.peers.values()) send(p.dc, { t: "count", n });
  syncTap(c);
  changed(c);
}

function setMuted(c, p, on) {
  p.muted = on;
  const quiet = on || P.muteAll;
  try { p.sender && p.sender.replaceTrack(quiet ? null : P.au.track); } catch {}
  send(p.dc, { t: "muted", on: quiet });
}

function hostTick(c) {
  if (!P.codeDead && due("poll", 2000)) hostPoll(c);
  if (P.tap.source && due("keep", 5000)) c.audio.keep().catch(() => {});
  const now = Date.now();
  for (const p of [...P.peers.values()]) {
    // A listener that never connected, or that fell silent (its pings stop when it is gone).
    if ((p.state === "joining" && now - p.since > 30000) || (p.state !== "joining" && now - p.seen > 25000)) dropPeer(c, p.id);
  }
  if (P.tap.at && now - P.tap.at > 1500) P.tap.level = 0;
}

/* ---------- listening ---------- */

function joinParty(c, raw) {
  if (P.role || P.busy) return;
  const code = String(raw || "").toUpperCase().replace(/[^A-Z0-9]/g, "");
  if (!CODE_RE.test(code)) return c.flash(t("A party code is 6 letters and numbers, like K7M 2QX"), true);
  if (!c.s.peer || !/^[0-9a-f]{32}$/.test(c.s.peer)) { c.s.peer = hex(16); c.save(); }
  P.role = "listener";
  P.ice = null;
  P.j = { code, state: "joining", host: "", np: null, art: "", count: 0, pc: null, dc: null, el: null, mutedByHost: false, muted: false, seen: 0, tries: 0, retryAt: 0, error: "", gen: 0 };
  c.keepOpen(true).catch(() => {});
  changed(c);
  connect(c);
}

function closeLink(j) {
  j.gen++;
  try { if (j.dc) j.dc.close(); } catch {}
  try { if (j.pc) j.pc.close(); } catch {}
  if (j.el) { try { j.el.pause(); j.el.srcObject = null; } catch {} }
  j.pc = j.dc = j.el = null;
}

/** The party is over for this listener: `why` stays on the page until it is dismissed. */
function ended(c, why) {
  const j = P.j;
  if (!j) return;
  closeLink(j);
  j.state = "ended";
  j.error = why;
  c.keepOpen(false).catch(() => {});
  changed(c);
}

function leaveParty(c) {
  const j = P.j;
  if (!j) return;
  send(j.dc, { t: "bye" });
  closeLink(j);
  P.j = null;
  P.role = "";
  c.keepOpen(false).catch(() => {});
  changed(c);
}

/** The connection was lost: try again, a little later each time, for about two minutes. */
function retry(c, j) {
  if (P.j !== j || j.state === "ended") return;
  closeLink(j);
  if (j.tries >= 9) return ended(c, t("The connection to the party was lost."));
  j.state = "reconnecting";
  j.retryAt = Date.now() + Math.min(15000, 1000 * 2 ** j.tries);
  j.tries++;
  changed(c);
}

async function connect(c) {
  const j = P.j;
  if (!j) return;
  const gen = ++j.gen;
  const mine = () => P.j === j && j.gen === gen;
  try {
    const servers = await iceServers(c, j.code);
    if (!mine()) return;
    const pc = new RTCPeerConnection({ iceServers: servers });
    j.pc = pc;
    pc.addTransceiver("audio", { direction: "recvonly" });
    const dc = pc.createDataChannel("party");
    j.dc = dc;
    dc.onmessage = (m) => { if (mine()) fromHost(c, j, m.data); };
    dc.onclose = () => { if (mine() && j.state === "connected") retry(c, j); };
    pc.ontrack = (e) => {
      if (!mine()) return;
      const el = new Audio();
      el.autoplay = true;
      el.srcObject = e.streams[0] || new MediaStream([e.track]);
      j.el = el;
      applyVolume(c);
      el.play().catch(() => {});
    };
    pc.onconnectionstatechange = () => {
      if (!mine()) return;
      const s = pc.connectionState;
      if (s === "connected") { j.state = "connected"; j.tries = 0; j.seen = Date.now(); changed(c); }
      else if (s === "failed" || s === "closed") retry(c, j);
    };
    const offer = await pc.createOffer();
    await pc.setLocalDescription({ type: "offer", sdp: tuneOpus(offer.sdp) });
    await gathered(pc);
    if (!mine()) return;
    const hi = await api(c, "POST", `/party/${j.code}/join`, { peer: c.s.peer, name: myName(c), sdp: pc.localDescription.sdp });
    if (!mine()) return;
    j.host = clip(hi.host, 24);
    changed(c);
    for (let i = 0; i < 25; i++) {
      await ticks(1);
      if (!mine()) return;
      const a = await api(c, "GET", `/party/${j.code}/answer?peer=${c.s.peer}`);
      if (!mine()) return;
      if (a.refused) return ended(c, a.refused === "kicked" ? t("The host removed you from this party.") : t("This party is full."));
      if (a.sdp) { await pc.setRemoteDescription({ type: "answer", sdp: String(a.sdp) }); return; }
    }
    throw new Error(t("The host did not answer. They may have closed Glint."));
  } catch (e) {
    if (!mine()) return;
    if (e.status === 404) return ended(c, j.host ? t("The party has ended.") : t("There is no party with this code. It may have ended."));
    if (e.status === 429 || e.status === 409 || (j.tries === 0 && !j.host && e.status)) return ended(c, e.message);
    retry(c, j);
  }
}

function fromHost(c, j, data) {
  j.seen = Date.now();
  let m = null;
  try { m = typeof data === "string" && data.length < 400000 ? JSON.parse(data) : null; } catch {}
  if (!m) return;
  if (m.t === "hello") { j.host = clip(m.host, 24) || j.host; j.count = Number(m.n) || 0; j.mutedByHost = !!m.muted; }
  else if (m.t === "np") {
    j.np = m.np && typeof m.np === "object" ? { title: clip(m.np.title, 200), artist: clip(m.np.artist, 200), album: clip(m.np.album, 200), app: clip(m.np.app, 40), playing: !!m.np.playing, ...place(m.np.pos, m.np.dur) } : null;
    j.art = artOk(m.art) ? m.art : "";
  }
  else if (m.t === "count") j.count = Number(m.n) || 0;
  else if (m.t === "muted") j.mutedByHost = !!m.on;
  else if (m.t === "end") return ended(c, t("The host ended the party."));
  else if (m.t === "kick") return ended(c, t("The host removed you from this party."));
  else return;
  changed(c);
}

function applyVolume(c) {
  const j = P.j;
  if (j && j.el) { j.el.volume = Math.min(1, Math.max(0, (Number(c.s.volume) || 0) / 100)); j.el.muted = !!j.muted; }
}

function listenTick(c) {
  const j = P.j;
  if (!j) return;
  const now = Date.now();
  if (j.state === "reconnecting" && now >= j.retryAt) { j.state = "joining"; changed(c); connect(c); }
  if (j.state === "connected") {
    if (due("ping", 5000)) send(j.dc, { t: "ping" });
    if (now - j.seen > 16000) retry(c, j); // three pings went unanswered
    if (j.el && j.el.paused) j.el.play().catch(() => {});
  }
}

/* ---------- the clock ---------- */

function tick(c) {
  const now = Date.now();
  if (now - P.tickAt < 400) return; // the page's own timer and Glint's tick both call this
  P.tickAt = now;
  const ws = P.waiters;
  P.waiters = [];
  for (const w of ws) { if (--w.n <= 0) w.done(); else P.waiters.push(w); }
  if ((c.s.show_np !== false || P.role || onPage()) && due("np", 2000)) pollNow(c);
  if (P.role === "host") hostTick(c);
  if (P.role === "listener") listenTick(c);
  patch(c);
  progress();
}

/* ---------- what it shows ---------- */

const onPage = () => !!document.querySelector('.plug-page[data-plugin="party"]');
/** Something changed: the top bar and, when it is open, the page follow. */
function changed(c) { c.chrome(); patch(c); progress(); }

/** The bars that show where a track is. Their place is set here and not in the page's HTML, so a second passing redraws nothing else. */
const barHtml = (n, who, cls) => (n && n.dur > 0 ? `<span class="pt-bar ${cls}" data-pt-bar="${who}"><i></i></span>` : "");
function progress() {
  const j = P.j, theirs = P.role === "listener" && j && j.state !== "ended" ? j.np : null;
  for (const el of document.querySelectorAll("[data-pt-bar], [data-pt-time]")) {
    const n = (el.dataset.ptBar || el.dataset.ptTime) === "host" ? theirs : P.np, at = posNow(n);
    if (at < 0) continue;
    if (el.dataset.ptTime) { const s = `${clock(at)} / ${clock(n.dur)}`; if (el.textContent !== s) el.textContent = s; }
    else {
      // A bar that was just drawn starts where the track is, without sweeping there from the left.
      const i = el.firstElementChild, fresh = !el._on;
      if (fresh) { el._on = true; i.style.transition = "none"; requestAnimationFrame(() => { i.style.transition = ""; }); }
      i.style.width = `${((at / n.dur) * 100).toFixed(2)}%`;
    }
  }
}

const artHtml = (src, cls) => (src ? `<img class="${cls}" src="${esc(src)}" alt="">` : `<span class="${cls} pt-noart">${ICON}</span>`);
const trackLine = (n) => `${esc(n.title)}${n.artist ? `<span class="top-muted"> · ${esc(n.artist)}</span>` : ""}`;
const STOP = '<svg viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="1.500"/></svg>';

/** The top bar: the track that is on (this PC's, or the host's while listening), and the party's state with a red dot. */
function topbar(c) {
  const j = P.j, hosting = P.role === "host", inParty = P.role === "listener" && j && j.state !== "ended";
  const n = inParty ? j.np : P.np;
  const art = inParty ? j.art : P.art;
  let track = "";
  if (n && (c.s.show_np !== false || P.role)) {
    const tip = [n.title, n.artist, n.album].filter(Boolean).join(" · ") + (n.app ? ` (${n.app})` : "");
    track = `<button class="btn pt-top" data-pact="open" title="${esc(tip)}">${artHtml(art, "pt-top-art")}<span class="pt-top-text top-label${n.playing ? "" : " pt-paused"}">${trackLine(n)}</span>${barHtml(n, inParty ? "host" : "own", "pt-top-bar")}</button>`;
  }
  if (hosting) {
    const k = listening(), live = !!P.tap.source;
    const tip = live ? t("Your music is being shared with {n} of {cap} listeners. Nothing is recorded.", { n: k, cap: P.cap }) : t("Party {code} is open. Nothing is shared until a friend joins.", { code: showCode(P.code) });
    return `<span class="top-group${live ? " live" : ""}"><button class="btn" data-pact="open" title="${esc(tip)}"><i class="top-dot${live ? "" : " idle"}"></i><span class="top-label">${esc(t("Party"))}</span><span class="top-muted top-label">${k}</span></button>${track}<button class="btn top-icon" data-pact="end" title="${esc(t("End party"))}" aria-label="${esc(t("End party"))}">${STOP}</button></span>`;
  }
  if (inParty) {
    const label = j.state === "connected" ? (j.host ? t("Listening with {name}", { name: j.host }) : t("Listening")) : j.state === "reconnecting" ? t("Reconnecting…") : t("Joining…");
    return `<span class="top-group"><button class="btn" data-pact="open"><i class="top-dot${j.state === "connected" ? "" : " idle"}"></i><span class="top-label">${esc(label)}</span></button>${track}<button class="btn top-icon" data-pact="leave" title="${esc(t("Leave party"))}" aria-label="${esc(t("Leave party"))}">${STOP}</button></span>`;
  }
  return track;
}

function nowCard(n, art, empty, who = "own") {
  if (!n) return `<div class="pt-card pt-empty">${artHtml("", "pt-art")}<div class="pt-card-text"><b>${esc(t("Nothing is playing"))}</b><span>${esc(empty)}</span></div></div>`;
  return `<div class="pt-card">${artHtml(art, "pt-art")}<div class="pt-card-text"><b>${esc(n.title)}</b><span>${esc([n.artist, n.album].filter(Boolean).join(" · "))}</span><span class="pt-from">${esc(n.playing ? t("Playing in {app}", { app: n.app || t("a music player") }) : t("Paused in {app}", { app: n.app || t("a music player") }))}</span>${n.dur > 0 ? `<span class="pt-prog">${barHtml(n, who, "")}<span class="pt-time" data-pt-time="${who}"></span></span>` : ""}</div></div>`;
}

function sourceNote(c) {
  if (!listening()) return t("Nothing is shared yet. Your music starts going out when the first friend joins.");
  if (P.muteAll) return t("Muted for everyone. Nothing is shared.");
  const src = P.tap.source;
  if (!src) return c.s.source === "auto" ? t("Start a song. Glint shares the player it finds, such as Spotify or YouTube Music.") : "";
  if (P.tap.state === "failed") return t("The sound could not be captured: {error}", { error: P.tap.error });
  if (P.tap.state === "waiting") return t("Waiting for {app} to start.", { app: src });
  const name = src === "all" ? t("everything this PC plays") : (P.np && P.np.exe === src && P.np.app) || src.replace(/\.exe$/, "");
  return P.tap.level > 0.003 ? t("Sharing the sound of {app}.", { app: name }) : t("Sharing {app}, which is silent right now.", { app: name });
}

function hostHtml(c) {
  const list = [...P.peers.values()];
  const rows = list.map((p) => `<div class="pt-peer"><span class="pt-peer-name">${esc(p.name)}</span><span class="pt-peer-state">${esc(p.state === "connected" ? (p.muted ? t("Muted") : t("Listening")) : p.state === "unsteady" ? t("Unsteady connection") : t("Joining…"))}</span>
      <button class="btn sm" data-pact="mute" data-peer="${p.id}">${esc(p.muted ? t("Unmute") : t("Mute"))}</button><button class="btn sm danger" data-pact="kick" data-peer="${p.id}">${esc(t("Kick"))}</button></div>`).join("");
  return `<div class="section"><h2>${esc(t("Your party"))}</h2>
    <div class="pt-host">
      <div class="pt-code-box"><span class="pt-code-label">${esc(t("Party code"))}</span><span class="pt-code">${esc(showCode(P.code))}</span>
        <button class="btn sm" data-pact="copy">${esc(t("Copy"))}</button></div>
      <div class="pt-host-text"><b>${esc(t("Friends open Tools, Party in Glint and type this code."))}</b>
        <span class="pt-live${P.tap.source && !P.muteAll ? " on" : ""}"><i class="top-dot${P.tap.source ? "" : " idle"}"></i>${esc(sourceNote(c))}</span></div>
      <div class="pt-host-actions"><button class="btn" data-pact="mute-all">${esc(P.muteAll ? t("Unmute everyone") : t("Mute everyone"))}</button><button class="btn danger" data-pact="end">${esc(t("End party"))}</button></div>
    </div>
    <div class="pt-peers"><div class="pt-peers-head">${esc(t("Listeners"))}<span>${listening()} / ${P.cap}</span></div>${rows || `<div class="pt-none">${esc(t("Nobody has joined yet."))}</div>`}</div>
  </div>`;
}

function listenHtml(c) {
  const j = P.j;
  if (j.state === "ended") return `<div class="section"><h2>${esc(t("Party"))}</h2><div class="pt-ended"><b>${esc(j.error)}</b><button class="btn" data-pact="leave">${esc(t("OK"))}</button></div></div>`;
  const state = j.state === "connected" ? (j.mutedByHost ? t("The host muted the music for you.") : t("{n} listening", { n: Math.max(1, j.count) })) : j.state === "reconnecting" ? t("The connection dropped. Joining again…") : t("Joining…");
  const vol = Math.round(Number(c.s.volume) || 0);
  return `<div class="section"><h2>${esc(j.host ? t("{name}'s party", { name: j.host }) : t("Party"))}</h2>
    ${j.state === "connected" ? nowCard(j.np, j.art, t("The host is not playing anything right now."), "host") : ""}
    ${j.host ? `<div class="pt-hostrow"><span class="pt-face">${esc([...j.host][0].toUpperCase())}</span><span class="pt-hostname"><b>${esc(j.host)}</b><span>${esc(t("Host"))}</span></span><span class="pt-codechip" title="${esc(t("Party code"))}">${esc(showCode(j.code))}</span></div>` : ""}
    <div class="pt-listen">
      <span class="pt-live${j.state === "connected" ? " on" : ""}"><i class="top-dot${j.state === "connected" ? "" : " idle"}"></i>${esc(state)}</span>
      <label class="pt-vol"><span>${esc(t("Volume"))}</span><input type="range" class="range" min="0" max="100" step="1" value="${vol}" data-pt-vol aria-label="${esc(t("Volume"))}"><output>${vol}%</output></label>
      <button class="btn" data-pact="mute-me">${esc(j.muted ? t("Unmute") : t("Mute"))}</button>
      <button class="btn danger" data-pact="leave">${esc(t("Leave party"))}</button>
    </div></div>`;
}

function idleHtml(c) {
  return `<div class="section"><h2>${esc(t("Listen together"))}</h2>
    <div class="pt-idle">
      <div class="pt-col"><b>${esc(t("Start a party"))}</b><span>${esc(t("You get a code. Friends who type it hear what your music player plays."))}</span>
        <button class="btn primary" data-pact="start"${P.busy ? " disabled" : ""}>${esc(P.busy === "start" ? t("Starting…") : t("Start party"))}</button></div>
      <div class="pt-col"><b>${esc(t("Join a party"))}</b><span>${esc(t("Type the code a friend gave you."))}</span>
        <div class="pt-join"><input class="field pt-code-in" type="text" maxlength="9" placeholder="K7M 2QX" spellcheck="false" autocomplete="off" data-pt-code aria-label="${esc(t("Party code"))}"><button class="btn" data-pact="join"${P.busy ? " disabled" : ""}>${esc(t("Join"))}</button></div></div>
    </div></div>`;
}

const mainHtml = (c) => (P.role === "host" ? hostHtml(c) : P.role === "listener" && P.j ? listenHtml(c) : idleHtml(c));
const npHtml = () => `<div class="section"><h2>${esc(t("Now playing on this PC"))}</h2>${nowCard(P.np, P.art, t("Start a song in Spotify, YouTube Music or a music player and it shows here."))}</div>`;

function sourceOptions(c) {
  const opts = [["auto", t("Only my music player (recommended)")], ["all", t("Everything this PC plays")]];
  const seen = new Set();
  for (const a of P.apps || []) if (a && a.exe && !seen.has(a.exe)) { seen.add(a.exe); opts.push([a.exe, t("Only {app}", { app: a.name || a.exe })]); }
  const cur = String(c.s.source || "auto");
  if (!opts.some(([v]) => v === cur)) opts.push([cur, t("Only {app}", { app: cur.replace(/\.exe$/, "") })]);
  return opts;
}

function settingsRows(c) {
  return c.row(t("Your name"), t("What friends see in a party"), c.text("name", { placeholder: myName(c), width: 200 }))
    + c.row(t("Now playing in the top bar"), t("Shows the track that is on, from Spotify, YouTube Music and other players"), c.toggle("show_np"))
    + c.row(t("What friends hear"), t("Your music player alone keeps voice chat and game sound out of the party"), c.select("source", sourceOptions(c)))
    + c.row(t("Most listeners"), t("Each listener uses a little of your upload speed"), c.select("cap", [2, 3, 4, 5, 6, 8].map((n) => [n, String(n)]), true));
}

function pageHtml(c) {
  if (!P.apps) { P.apps = []; c.media.apps().then((a) => { P.apps = Array.isArray(a) ? a : []; if (onPage()) c.render(); }).catch(() => {}); }
  setTimeout(progress, 0);
  return `<div class="page-head"><div><h1>${esc(t("Party"))}</h1><div class="sub">${esc(t("See what is playing, and listen to it together with friends"))}</div></div></div>
    <div id="pt-np">${npHtml()}</div>
    <div id="pt-main">${mainHtml(c)}</div>
    <div class="section"><h2>${esc(t("Settings"))}</h2>${settingsRows(c)}</div>
    <p class="pt-foot">${esc(t("Your sound is shared only while a friend is in your party, and only your music player's. It goes straight from your PC to theirs and is never recorded. People in a party can see each other's IP address. Music services do not allow sharing their music with others, so keep parties small and among friends."))}</p>`;
}

/** Redraw the parts of the open page that changed, and leave a box that is being typed in alone. */
function patch(c) {
  if (!onPage()) return;
  for (const [id, html] of [["pt-np", npHtml()], ["pt-main", mainHtml(c)]]) {
    const el = document.getElementById(id);
    if (!el || el._html === html) continue;
    if (el._html !== undefined && el.contains(document.activeElement) && document.activeElement.matches("input[type=text], input[type=range]:active")) continue;
    el._html = html;
    el.innerHTML = html;
  }
}

/* ---------- start ---------- */

Glint.register({
  defaults: DEFAULTS,
  pages: [{ id: "party", title: "Party", icon: ICON, render: pageHtml, badge: () => (P.role === "host" && listening() ? String(listening()) : "") }],
  topbar,
  start(c) {
    setInterval(() => tick(c), 1000);
    tick(c);
    // The volume slider and the code box are the page's own controls: they are not Glint settings rows.
    document.addEventListener("input", (e) => {
      if (e.target.matches && e.target.matches("[data-pt-vol]")) {
        c.s.volume = Number(e.target.value) || 0;
        const out = e.target.parentElement.querySelector("output");
        if (out) out.textContent = `${Math.round(c.s.volume)}%`;
        applyVolume(c);
        c.save();
      }
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && e.target.matches && e.target.matches("[data-pt-code]")) { e.target.blur(); joinParty(c, e.target.value); }
    });
    // Glint is quitting or the page is going: the party ends for everyone, not after a timeout.
    window.addEventListener("pagehide", () => {
      if (P.role === "host") endParty(c, true);
      if (P.role === "listener" && P.j) send(P.j.dc, { t: "bye" });
    });
  },
  onTick: (c) => tick(c),
  onAudio(st, c) {
    if (P.role !== "host" || st.source !== P.tap.source) return;
    if (st.state === "stopped") { if (P.tap.source) { P.tap.source = ""; syncTap(c); } return; } // it ended by itself: asked for again while someone listens
    P.tap.state = st.state;
    P.tap.error = st.error || "";
    changed(c);
  },
  onSave(c) {
    c.s.name = clip(c.s.name, 24);
    if (P.role === "host") syncTap(c); // "what friends hear" changed
  },
  onAct(name, el, c) {
    if (name === "open") return c.go("p:party:party");
    if (name === "start") return startParty(c);
    if (name === "join") return joinParty(c, document.querySelector("[data-pt-code]")?.value);
    if (name === "leave") return leaveParty(c);
    if (name === "copy") return Promise.resolve(c.copy(P.code)).then(() => c.flash(t("Party code copied"))).catch(c.fail);
    if (name === "end") {
      if (!listening()) return endParty(c);
      return c.confirm({ title: t("End the party?"), text: t("The music stops for everyone who is listening."), ok: t("End party") }).then((a) => { if (a === "yes") endParty(c); });
    }
    if (name === "mute-all") {
      P.muteAll = !P.muteAll;
      for (const p of P.peers.values()) setMuted(c, p, p.muted);
      syncTap(c);
      return changed(c);
    }
    if (name === "mute-me" && P.j) { P.j.muted = !P.j.muted; applyVolume(c); return changed(c); }
    const p = P.peers.get(el.dataset.peer || "");
    if (name === "mute" && p) { setMuted(c, p, !p.muted); return changed(c); }
    if (name === "kick" && p) {
      P.banned.add(p.id);
      send(p.dc, { t: "kick" });
      const pc = p.pc;
      P.peers.delete(p.id);
      setTimeout(() => { try { pc.close(); } catch {} }, 300);
      return peersChanged(c);
    }
  },
  lang: {
    ko: {
      "Party": "파티",
      "Shows what is playing in the top bar, from Spotify, YouTube Music and other music players. Start a party and friends who type your code hear your music live, straight from your PC to theirs.": "Spotify, YouTube Music 등 음악 플레이어에서 재생 중인 곡을 상단 바에 보여 줘요. 파티를 시작하면 코드를 입력한 친구가 내 음악을 실시간으로 함께 들어요. 소리는 내 PC에서 친구 PC로 바로 전달돼요.",
      "A friend": "친구",
      "The party server could not be reached. Check the internet connection.": "파티 서버에 연결할 수 없어요. 인터넷 연결을 확인해 주세요.",
      "There is no party with this code. It may have ended.": "이 코드의 파티가 없어요. 이미 끝났을 수 있어요.",
      "Before you start a party": "파티를 시작하기 전에",
      "Friends who join hear what your music player plays, live. The sound goes straight from your PC to theirs, so they can see your IP address, as in any direct call. Nothing is recorded. Music services do not allow sharing their music with others this way, so keep parties to a few friends.": "참가한 친구는 내 음악 플레이어에서 나오는 소리를 실시간으로 들어요. 소리는 내 PC에서 친구 PC로 바로 전달되므로, 일반적인 직접 통화처럼 친구가 내 IP 주소를 볼 수 있어요. 아무것도 녹음되지 않아요. 음악 서비스는 이런 방식으로 음악을 다른 사람과 공유하는 것을 허용하지 않으니, 파티는 친한 친구 몇 명과만 하세요.",
      "Start party": "파티 시작",
      "The party server answered with an error.": "파티 서버에서 오류가 발생했어요.",
      "The party is over": "파티가 끝났어요",
      "The party's code has expired. Start a new party to let more friends in.": "파티 코드가 만료됐어요. 친구를 더 초대하려면 새 파티를 시작하세요.",
      "A party code is 6 letters and numbers, like K7M 2QX": "파티 코드는 K7M 2QX처럼 문자와 숫자 6자리예요",
      "The connection to the party was lost.": "파티와의 연결이 끊어졌어요.",
      "The host removed you from this party.": "호스트가 나를 이 파티에서 내보냈어요.",
      "This party is full.": "이 파티는 가득 찼어요.",
      "The host did not answer. They may have closed Glint.": "호스트가 응답하지 않아요. Glint를 닫았을 수 있어요.",
      "The party has ended.": "파티가 끝났어요.",
      "The host ended the party.": "호스트가 파티를 끝냈어요.",
      "Your music is being shared with {n} of {cap} listeners. Nothing is recorded.": "내 음악을 청취자 {n}명(최대 {cap}명)과 공유하고 있어요. 아무것도 녹음되지 않아요.",
      "Party {code} is open. Nothing is shared until a friend joins.": "파티 {code}이(가) 열려 있어요. 친구가 참가하기 전에는 아무것도 공유되지 않아요.",
      "End party": "파티 끝내기",
      "Listening with {name}": "{name} 님과 듣는 중",
      "Host": "호스트",
      "Listening": "듣는 중",
      "Reconnecting…": "다시 연결하는 중…",
      "Joining…": "참가하는 중…",
      "Leave party": "파티 나가기",
      "Nothing is playing": "재생 중인 곡이 없어요",
      "Playing in {app}": "{app}에서 재생 중",
      "a music player": "음악 플레이어",
      "Paused in {app}": "{app}에서 일시 정지됨",
      "Nothing is shared yet. Your music starts going out when the first friend joins.": "아직 아무것도 공유되지 않아요. 첫 친구가 참가하면 음악이 전달되기 시작해요.",
      "Muted for everyone. Nothing is shared.": "모두에게 음소거됨. 아무것도 공유되지 않아요.",
      "Start a song. Glint shares the player it finds, such as Spotify or YouTube Music.": "노래를 재생하세요. Glint가 Spotify나 YouTube Music 같은 플레이어를 찾아 공유해요.",
      "The sound could not be captured: {error}": "소리를 캡처할 수 없어요: {error}",
      "Waiting for {app} to start.": "{app}이(가) 시작되기를 기다리는 중이에요.",
      "everything this PC plays": "이 PC에서 나오는 모든 소리",
      "Sharing the sound of {app}.": "{app}의 소리를 공유하는 중이에요.",
      "Sharing {app}, which is silent right now.": "{app}을(를) 공유 중인데 지금은 소리가 없어요.",
      "Muted": "음소거됨",
      "Unsteady connection": "연결 불안정",
      "Unmute": "음소거 해제",
      "Mute": "음소거",
      "Kick": "내보내기",
      "Your party": "내 파티",
      "Party code": "파티 코드",
      "Copy": "복사",
      "Friends open Tools, Party in Glint and type this code.": "친구가 Glint의 도구, 파티를 열고 이 코드를 입력하면 돼요.",
      "Unmute everyone": "모두 음소거 해제",
      "Mute everyone": "모두 음소거",
      "Listeners": "청취자",
      "Nobody has joined yet.": "아직 아무도 참가하지 않았어요.",
      "OK": "확인",
      "The host muted the music for you.": "호스트가 내 음악을 음소거했어요.",
      "{n} listening": "{n}명 듣는 중",
      "The connection dropped. Joining again…": "연결이 끊어졌어요. 다시 참가하는 중…",
      "{name}'s party": "{name} 님의 파티",
      "The host is not playing anything right now.": "호스트가 지금은 아무것도 재생하지 않고 있어요.",
      "Volume": "음량",
      "Listen together": "함께 듣기",
      "Start a party": "파티 시작하기",
      "You get a code. Friends who type it hear what your music player plays.": "코드를 받아요. 코드를 입력한 친구는 내 음악 플레이어의 소리를 들어요.",
      "Starting…": "시작하는 중…",
      "Join a party": "파티 참가하기",
      "Type the code a friend gave you.": "친구에게 받은 코드를 입력하세요.",
      "Join": "참가",
      "Now playing on this PC": "이 PC에서 재생 중",
      "Start a song in Spotify, YouTube Music or a music player and it shows here.": "Spotify, YouTube Music 또는 음악 플레이어에서 노래를 재생하면 여기에 표시돼요.",
      "Only my music player (recommended)": "내 음악 플레이어만 (권장)",
      "Everything this PC plays": "이 PC에서 나오는 모든 소리",
      "Only {app}": "{app}만",
      "Your name": "내 이름",
      "What friends see in a party": "파티에서 친구에게 보이는 이름",
      "Now playing in the top bar": "상단 바에 재생 중인 곡 표시",
      "Shows the track that is on, from Spotify, YouTube Music and other players": "Spotify, YouTube Music 등 플레이어에서 재생 중인 곡을 보여 줘요",
      "What friends hear": "친구가 듣는 소리",
      "Your music player alone keeps voice chat and game sound out of the party": "음악 플레이어만 선택하면 음성 채팅과 게임 소리는 파티에 들어가지 않아요",
      "Most listeners": "최대 청취자 수",
      "Each listener uses a little of your upload speed": "청취자 한 명마다 업로드 속도를 조금씩 사용해요",
      "See what is playing, and listen to it together with friends": "재생 중인 곡을 보고, 친구와 함께 들어요",
      "Your sound is shared only while a friend is in your party, and only your music player's. It goes straight from your PC to theirs and is never recorded. People in a party can see each other's IP address. Music services do not allow sharing their music with others, so keep parties small and among friends.": "소리는 친구가 파티에 있는 동안에만, 그리고 음악 플레이어의 소리만 공유돼요. 내 PC에서 친구 PC로 바로 전달되고 녹음되지 않아요. 파티에 있는 사람들은 서로의 IP 주소를 볼 수 있어요. 음악 서비스는 음악을 다른 사람과 공유하는 것을 허용하지 않으니, 파티는 작게, 친구끼리만 하세요.",
      "Party code copied": "파티 코드를 복사했어요",
      "End the party?": "파티를 끝낼까요?",
      "The music stops for everyone who is listening.": "듣고 있는 모든 사람의 음악이 멈춰요."
    },
    el: {
      "Party": "Πάρτι",
      "Shows what is playing in the top bar, from Spotify, YouTube Music and other music players. Start a party and friends who type your code hear your music live, straight from your PC to theirs.": "Δείχνει στην πάνω μπάρα τι παίζει στο Spotify, στο YouTube Music και σε άλλα προγράμματα μουσικής. Ξεκίνα ένα πάρτι και οι φίλοι που γράφουν τον κωδικό σου ακούν τη μουσική σου ζωντανά, απευθείας από τον υπολογιστή σου στον δικό τους.",
      "A friend": "Φίλος",
      "The party server could not be reached. Check the internet connection.": "Δεν ήταν δυνατή η σύνδεση με τον διακομιστή του πάρτι. Έλεγξε τη σύνδεση στο διαδίκτυο.",
      "There is no party with this code. It may have ended.": "Δεν υπάρχει πάρτι με αυτόν τον κωδικό. Ίσως έχει τελειώσει.",
      "Before you start a party": "Πριν ξεκινήσεις ένα πάρτι",
      "Friends who join hear what your music player plays, live. The sound goes straight from your PC to theirs, so they can see your IP address, as in any direct call. Nothing is recorded. Music services do not allow sharing their music with others this way, so keep parties to a few friends.": "Οι φίλοι που μπαίνουν ακούν ζωντανά ό,τι παίζει το πρόγραμμα μουσικής σου. Ο ήχος πηγαίνει απευθείας από τον υπολογιστή σου στον δικό τους, άρα μπορούν να δουν τη διεύθυνση IP σου, όπως σε κάθε απευθείας κλήση. Τίποτα δεν ηχογραφείται. Οι υπηρεσίες μουσικής δεν επιτρέπουν να μοιράζεσαι έτσι τη μουσική τους με άλλους, γι' αυτό κράτα τα πάρτι για λίγους φίλους.",
      "Start party": "Έναρξη πάρτι",
      "The party server answered with an error.": "Ο διακομιστής του πάρτι απάντησε με σφάλμα.",
      "The party is over": "Το πάρτι τελείωσε",
      "The party's code has expired. Start a new party to let more friends in.": "Ο κωδικός του πάρτι έληξε. Ξεκίνα νέο πάρτι για να μπουν κι άλλοι φίλοι.",
      "A party code is 6 letters and numbers, like K7M 2QX": "Ο κωδικός πάρτι έχει 6 γράμματα και αριθμούς, όπως K7M 2QX",
      "The connection to the party was lost.": "Η σύνδεση με το πάρτι χάθηκε.",
      "The host removed you from this party.": "Ο οικοδεσπότης σε αφαίρεσε από αυτό το πάρτι.",
      "This party is full.": "Αυτό το πάρτι είναι γεμάτο.",
      "The host did not answer. They may have closed Glint.": "Ο οικοδεσπότης δεν απάντησε. Ίσως έκλεισε το Glint.",
      "The party has ended.": "Το πάρτι τελείωσε.",
      "The host ended the party.": "Ο οικοδεσπότης τερμάτισε το πάρτι.",
      "Your music is being shared with {n} of {cap} listeners. Nothing is recorded.": "Η μουσική σου μοιράζεται με {n} από {cap} ακροατές. Τίποτα δεν ηχογραφείται.",
      "Party {code} is open. Nothing is shared until a friend joins.": "Το πάρτι {code} είναι ανοιχτό. Τίποτα δεν μοιράζεται μέχρι να μπει ένας φίλος.",
      "End party": "Τέλος πάρτι",
      "Listening with {name}": "Ακούς με {name}",
      "Host": "Οικοδεσπότης",
      "Listening": "Ακούει",
      "Reconnecting…": "Επανασύνδεση…",
      "Joining…": "Σύνδεση…",
      "Leave party": "Αποχώρηση",
      "Nothing is playing": "Δεν παίζει τίποτα",
      "Playing in {app}": "Παίζει στο {app}",
      "a music player": "πρόγραμμα μουσικής",
      "Paused in {app}": "Σε παύση στο {app}",
      "Nothing is shared yet. Your music starts going out when the first friend joins.": "Τίποτα δεν μοιράζεται ακόμα. Η μουσική σου αρχίζει να στέλνεται όταν μπει ο πρώτος φίλος.",
      "Muted for everyone. Nothing is shared.": "Σε σίγαση για όλους. Τίποτα δεν μοιράζεται.",
      "Start a song. Glint shares the player it finds, such as Spotify or YouTube Music.": "Βάλε ένα τραγούδι. Το Glint μοιράζεται το πρόγραμμα που βρίσκει, όπως το Spotify ή το YouTube Music.",
      "The sound could not be captured: {error}": "Δεν ήταν δυνατή η λήψη του ήχου: {error}",
      "Waiting for {app} to start.": "Αναμονή να ξεκινήσει το {app}.",
      "everything this PC plays": "ό,τι παίζει αυτός ο υπολογιστής",
      "Sharing the sound of {app}.": "Μοιράζεται ο ήχος του {app}.",
      "Sharing {app}, which is silent right now.": "Μοιράζεται το {app}, που τώρα είναι σιωπηλό.",
      "Muted": "Σε σίγαση",
      "Unsteady connection": "Ασταθής σύνδεση",
      "Unmute": "Άρση σίγασης",
      "Mute": "Σίγαση",
      "Kick": "Αποβολή",
      "Your party": "Το πάρτι σου",
      "Party code": "Κωδικός πάρτι",
      "Copy": "Αντιγραφή",
      "Friends open Tools, Party in Glint and type this code.": "Οι φίλοι ανοίγουν Εργαλεία, Πάρτι στο Glint και γράφουν αυτόν τον κωδικό.",
      "Unmute everyone": "Άρση σίγασης για όλους",
      "Mute everyone": "Σίγαση για όλους",
      "Listeners": "Ακροατές",
      "Nobody has joined yet.": "Δεν έχει μπει κανείς ακόμα.",
      "The host muted the music for you.": "Ο οικοδεσπότης έβαλε τη μουσική σε σίγαση για εσένα.",
      "{n} listening": "{n} ακούν",
      "The connection dropped. Joining again…": "Η σύνδεση έπεσε. Γίνεται ξανά σύνδεση…",
      "{name}'s party": "Το πάρτι του χρήστη {name}",
      "The host is not playing anything right now.": "Ο οικοδεσπότης δεν παίζει κάτι αυτή τη στιγμή.",
      "Volume": "Ένταση",
      "Listen together": "Ακούστε μαζί",
      "Start a party": "Ξεκίνα ένα πάρτι",
      "You get a code. Friends who type it hear what your music player plays.": "Παίρνεις έναν κωδικό. Οι φίλοι που τον γράφουν ακούν ό,τι παίζει το πρόγραμμα μουσικής σου.",
      "Starting…": "Εκκίνηση…",
      "Join a party": "Μπες σε πάρτι",
      "Type the code a friend gave you.": "Γράψε τον κωδικό που σου έδωσε ένας φίλος.",
      "Join": "Είσοδος",
      "Now playing on this PC": "Παίζει τώρα σε αυτόν τον υπολογιστή",
      "Start a song in Spotify, YouTube Music or a music player and it shows here.": "Βάλε ένα τραγούδι στο Spotify, στο YouTube Music ή σε ένα πρόγραμμα μουσικής και θα φανεί εδώ.",
      "Only my music player (recommended)": "Μόνο το πρόγραμμα μουσικής μου (προτείνεται)",
      "Everything this PC plays": "Ό,τι παίζει αυτός ο υπολογιστής",
      "Only {app}": "Μόνο {app}",
      "Your name": "Το όνομά σου",
      "What friends see in a party": "Αυτό που βλέπουν οι φίλοι στο πάρτι",
      "Now playing in the top bar": "Τι παίζει, στην πάνω μπάρα",
      "Shows the track that is on, from Spotify, YouTube Music and other players": "Δείχνει το κομμάτι που παίζει στο Spotify, στο YouTube Music και σε άλλα προγράμματα",
      "What friends hear": "Τι ακούν οι φίλοι",
      "Your music player alone keeps voice chat and game sound out of the party": "Με μόνο το πρόγραμμα μουσικής, η φωνητική συνομιλία και ο ήχος του παιχνιδιού μένουν έξω από το πάρτι",
      "Most listeners": "Μέγιστοι ακροατές",
      "Each listener uses a little of your upload speed": "Κάθε ακροατής χρησιμοποιεί λίγη από την ταχύτητα αποστολής σου",
      "See what is playing, and listen to it together with friends": "Δες τι παίζει και άκουσέ το μαζί με φίλους",
      "Your sound is shared only while a friend is in your party, and only your music player's. It goes straight from your PC to theirs and is never recorded. People in a party can see each other's IP address. Music services do not allow sharing their music with others, so keep parties small and among friends.": "Ο ήχος σου μοιράζεται μόνο όσο ένας φίλος είναι στο πάρτι σου, και μόνο του προγράμματος μουσικής σου. Πηγαίνει απευθείας από τον υπολογιστή σου στον δικό τους και δεν ηχογραφείται ποτέ. Όσοι είναι σε ένα πάρτι μπορούν να δουν ο ένας τη διεύθυνση IP του άλλου. Οι υπηρεσίες μουσικής δεν επιτρέπουν να μοιράζεσαι τη μουσική τους με άλλους, γι' αυτό κράτα τα πάρτι μικρά και ανάμεσα σε φίλους.",
      "Party code copied": "Ο κωδικός πάρτι αντιγράφηκε",
      "End the party?": "Τέλος του πάρτι;",
      "The music stops for everyone who is listening.": "Η μουσική σταματά για όλους όσοι ακούν."
    },
  },
});
