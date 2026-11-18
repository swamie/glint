# Glint Doctor (the Worker)

Optional. The developer's end of the one-time support tool in
`tools/glint-doctor`. A Cloudflare Worker that

- holds the Anthropic API key (the tool never has it),
- accepts one upload of diagnostics per support code, and uses the code up,
- asks Claude (`claude-opus-5-5`) what is wrong and how to patch it,
- keeps the report for you and posts it to your Discord.

It is its own Worker. It shares nothing with `server/share` or `server/polls`.

## What it costs

Cloudflare: nothing on the free plan (SQLite-backed Durable Objects and their
alarms are included). Anthropic: one request per report. The prompt is cut to
roughly 125,000 tokens at the very most and the answer to 32,000, so a report
costs about $0.20 to $0.50 and cannot pass about $1.15 at today's prices.
`MAX_RUNS_PER_DAY` (20) is a ceiling on any one day.

## Setting it up (once)

You need a Cloudflare account, Node.js and an Anthropic API key. Give the key
its own workspace with a spend limit in the Anthropic Console.

    cd server/doctor
    npx wrangler login
    npx wrangler secret put ANTHROPIC_API_KEY
    npx wrangler secret put ADMIN_TOKEN        # a long random string, see below
    npx wrangler secret put DISCORD_WEBHOOK    # optional: a webhook of a private channel
    npx wrangler deploy

For `ADMIN_TOKEN`:
`node -e "console.log(require('crypto').randomBytes(32).toString('hex'))"`.
`deploy` prints the address, such as `https://glint-doctor.<you>.workers.dev`.
Put it in `tools/glint-doctor/endpoint.txt` and build the tool, and in a file
`.doctor-admin` here (git-ignored) for the admin script:

    DOCTOR_URL=https://glint-doctor.<you>.workers.dev
    DOCTOR_ADMIN_TOKEN=<the ADMIN_TOKEN>

## Helping a user

    node scripts/doctor-admin.mjs mint "Rubust, black clips on AMD" --hours 72

prints a code such as `7KQ2-MXW9-PD4H`, once. Send the user
`glint-doctor.exe` renamed to `glint-doctor-7KQ2-MXW9-PD4H.exe`. When they
have run it, the report arrives in Discord with the full text attached, or:

    node scripts/doctor-admin.mjs reports             # the list
    node scripts/doctor-admin.mjs show <ticket>       # the report, as Markdown
    node scripts/doctor-admin.mjs show <ticket> --bundle   # with the diagnostics it was written from
    node scripts/doctor-admin.mjs retry <ticket>      # analyse again (after a failure or a prompt change)
    node scripts/doctor-admin.mjs codes               # every code's state
    node scripts/doctor-admin.mjs revoke PD4H         # withdraw an unused code
    node scripts/doctor-admin.mjs delete <ticket>

The same things are plain HTTP under `/admin/*` with
`Authorization: Bearer <ADMIN_TOKEN>`; the list is at the top of `src/index.js`.

## Settings (`wrangler.toml`, `[vars]`)

- `MODEL`, `EFFORT`: the Claude model and how hard it thinks.
- `FALLBACKS`: `on` sends `fallbacks: "default"` (a beta), so a request a
  safety classifier declines is re-run on another model in the same call. Set
  `off` if you would rather get a failed report.
- `REPO`: where Glint's source is read from, at tag `v<version>`, else `main`.
- `MAX_UPLOAD_KB`, `CODE_HOURS`, `MAX_RUNS_PER_DAY`, `RETAIN_DAYS`.
- `USER_SUMMARY`: `off` if the user should only ever see "sent, thanks".
- `DISCORD_PINGS`: Discord user ids to ping.

## How it works

- `src/index.js`: the routes, the size cap, minting.
- `src/store.js`: the Durable Object. Codes (hashes only), reports, and the
  alarm that runs the analysis after the user has been answered.
- `src/analyze.js`: picks source files from the faults, fetches them from
  GitHub, builds the prompt, calls the Messages API (streamed).
- `src/prompt.js`: the system prompt with Glint's module map, and the report's JSON schema.
- `src/scrub.js`: the second scrubbing pass.
- `src/deliver.js`: the report as Markdown, and the Discord post.

If you add a module to Glint or change a log prefix, add it to `MODULES` in
`src/prompt.js`: that table is both what Claude knows about the code and what
picks the files it is shown.

## Testing

`test/smoke.sh` runs the whole flow against `npx wrangler dev` with
`test/mock.mjs` standing in for Claude, GitHub and Discord. Nothing real is
called. The commands are at the top of the script.
