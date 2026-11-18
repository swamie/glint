// A finished report, for the developer: as Markdown (what /admin/reports/<ticket>?format=md
// returns and what is attached in Discord) and as a Discord message.

const cut = (text, max) => { const t = String(text ?? ""); return t.length <= max ? t : t.slice(0, max - 1) + "…"; };
const fence = (text, lang = "") => "```" + lang + "\n" + String(text).replace(/```/g, "'''") + "\n```";

/** The report as a Markdown document. `row` is the stored report row. */
export function markdown(row) {
  const r = row.report || {};
  const out = [`# Glint Doctor report ${row.ticket}`, "", `- For: ${row.label || "(no label)"}`, `- Glint ${row.glint_version || "version unknown"}, received ${new Date(row.created).toISOString()}`, `- Model ${row.model || "?"}, ${row.input_tokens || 0} tokens in, ${row.output_tokens || 0} out`];
  if (row.meta?.source_files?.length) out.push(`- Source read at ${row.meta.source_ref}: ${row.meta.source_files.join(", ")}`);
  if (row.status !== "done") out.push("", `**Status: ${row.status}.** ${row.error || ""}`);
  if (r.developer_summary) out.push("", `## Summary (${r.severity})`, "", r.developer_summary);
  if (r.user_summary) out.push("", "## What the user was told", "", r.user_summary);
  (r.findings || []).forEach((f, i) => {
    out.push("", `## Finding ${i + 1}: ${f.title}`, "", `Confidence **${f.confidence}**, ${f.category}`, "", f.explanation);
    if (f.evidence?.length) out.push("", "Evidence:", "", fence(f.evidence.join("\n")));
  });
  (r.patches || []).forEach((p, i) => {
    out.push("", `## Patch ${i + 1}: ${p.file}, \`${p.function}\``, "", `For: ${p.finding}`, "", p.change);
    if (p.code) out.push("", fence(p.code, p.file.endsWith(".rs") ? "rust" : ""));
    if (!p.source_seen) out.push("", "_Claude did not see this function's source; the change is described by intent._");
    out.push("", `Risk: ${p.risk}`, "", `Verify: ${p.verify}`);
  });
  if (r.workarounds?.length) out.push("", "## Workarounds for the user", "", ...r.workarounds.map((w) => `- ${w}`));
  if (r.missing_information?.length) out.push("", "## Missing information", "", ...r.missing_information.map((w) => `- ${w}`));
  return out.join("\n") + "\n";
}

/** Post the report to the developer's Discord webhook, the full text attached as a file. True if Discord took it. */
export async function postToDiscord(env, row) {
  if (!env.DISCORD_WEBHOOK) return false;
  const r = row.report || {};
  const ids = String(env.DISCORD_PINGS || "").split(",").map((s) => s.trim()).filter((s) => /^\d+$/.test(s));
  const done = row.status === "done";
  const title = `Glint Doctor · ${row.label || "no label"} · Glint ${row.glint_version || "?"}`;
  const fields = [];
  for (const f of (r.findings || []).slice(0, 8)) fields.push({ name: cut(`${f.confidence.toUpperCase()} · ${f.title}`, 256), value: cut(f.explanation, 1000), inline: false });
  if (r.patches?.length) fields.push({ name: "Patches", value: cut(r.patches.map((p) => `\`${p.file}\` \`${p.function}\``).join("\n"), 1000), inline: false });
  const embed = done
    ? { title: cut(`${r.severity}: ${(r.findings || []).length} finding(s), ${(r.patches || []).length} patch(es)`, 256), description: cut(r.developer_summary, 4000), color: 0x3b82f6, fields, footer: { text: `ticket ${row.ticket} · ${row.model} · ${row.input_tokens} in / ${row.output_tokens} out` } }
    : { title: "The analysis failed", description: cut(`${row.error}\n\nThe diagnostics are stored. Retry: POST /admin/reports/${row.ticket}/retry`, 4000), color: 0xe5484d, footer: { text: `ticket ${row.ticket}` } };
  const payload = { content: cut((ids.length ? ids.map((id) => `<@${id}>`).join(" ") + "\n" : "") + title, 2000), allowed_mentions: { parse: [], users: ids }, embeds: [embed] };
  const form = new FormData();
  form.append("payload_json", JSON.stringify(payload));
  if (done) form.append("files[0]", new Blob([markdown(row)], { type: "text/markdown" }), `glint-doctor-${row.ticket}.md`);
  try {
    const res = await fetch(env.DISCORD_WEBHOOK, { method: "POST", body: form });
    return res.ok;
  } catch { return false; }
}
