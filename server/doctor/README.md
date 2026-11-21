# Glint Doctor (the Worker)

Optional. This is the developer's end of the one-time support tool in
`tools/glint-doctor`. It is a Cloudflare Worker that:

- holds the Anthropic API key (the tool never has it),
- accepts one upload of diagnostics per support code, and uses the code up,
- asks Claude (`claude-opus-5-5` by default) what is wrong and how to patch it,
- keeps the report for you and posts it to your Discord.

It is its own Worker. It shares nothing with `server/share` or `server/polls`,
and nothing in `src-tauri` or `ui` depends on it.

## What it costs

Cloudflare: nothing on the free plan. SQLite-backed Durable Objects and their
alarms are included.

Anthropic: one request per report. The diagnostics and source in the prompt are
cut to roughly 125,000 tokens at the very most, and the answer to
`MAX_OUTPUT_TOKENS` (64,000, thinking included). At $4 in and $20 out per
million tokens, a request costs at most about $0.50 + $1.28 = $1.80, and a
typical one a few tens of cents. `MAX_RUNS_PER_DAY` (20) caps how many reports
a day can bring in, whatever codes exist. Give the key a workspace of its own
with a spend limit too.

## 1. Deploy the Worker (once)

You need a Cloudflare account, Node.js 18 or newer, and an Anthropic API key.

    cd server/doctor
    npx wrangler login
    npx wrangler deploy

`deploy` prints the Worker's address, such as
`https://glint-doctor.<you>.workers.dev`. Keep it for steps 2 and 3.

## 2. Set the secrets

Secrets are stored by Cloudflare, never in this repository. Each command asks
for the value.

    npx wrangler secret put ANTHROPIC_API_KEY   # the API key, from a workspace with a spend limit
    npx wrangler secret put ADMIN_TOKEN         # your key for /admin/*, at least 24 characters
    npx wrangler secret put DISCORD_WEBHOOK     # optional: a webhook of a private channel
    npx wrangler secret put GITHUB_TOKEN        # optional: only if the repository goes private or GitHub rate-limits

Make `ADMIN_TOKEN` long and random:

    node -e "console.log(require('crypto').randomBytes(32).toString('hex'))"

An `ADMIN_TOKEN` shorter than 24 characters is refused for every admin request.
To change a secret later, run the same `secret put` again; it takes effect
without a new deploy.

## 3. Point the tool and the admin script at it

Put the address in `tools/glint-doctor/endpoint.txt` (one line, no secret) and
build the tool (see `tools/glint-doctor/README.md`). The file in the repository
holds a placeholder, and a tool built with the placeholder refuses to run. Keep
your real address out of commits if you do not want it public: it is not a
secret, but there is no reason to advertise it.

For the admin script, make a file `server/doctor/.doctor-admin` (git-ignored):

    DOCTOR_URL=https://glint-doctor.<you>.workers.dev
    DOCTOR_ADMIN_TOKEN=<the ADMIN_TOKEN>

The two can also be environment variables of the same names.

## 4. Mint a code for a user

    node scripts/doctor-admin.mjs mint "a Discord user, black clips on AMD" --hours 72

The label says who the code is for; it shows in the list and in the report.
The script prints the code once, such as `7KQ2-MXW9-PD4H`. The Worker keeps only
a hash of it. A code is good for 72 hours unless you say otherwise (30 days at
most), and works once.

## 5. Give it to the user

Copy the built `glint-doctor.exe` to a name with the code in it:

    glint-doctor-7KQ2-MXW9-PD4H.exe

and send that file. The user double-clicks it; the tool reads the code from its
own name. If the name gets changed on the way, the tool asks for the code, so
you can also send the code as text.

Send it yourself, in a private message. A code is worth one report that only
you can read, so a leaked code costs one report at most. Until it is used you
can withdraw it:

    node scripts/doctor-admin.mjs revoke PD4H     # the last 4 characters

## 6. Read the report

When the user has sent their diagnostics, the analysis runs on the Worker. It
usually takes a minute or two. Then:

- In Discord (if `DISCORD_WEBHOOK` is set): a message with the label, the
  summary, each finding with its confidence, and the files to patch, with the
  whole report attached as Markdown.
- From a terminal:

      node scripts/doctor-admin.mjs reports                  # the list, newest first
      node scripts/doctor-admin.mjs show <ticket>            # the report as Markdown
      node scripts/doctor-admin.mjs show <ticket> --bundle   # as JSON, with the diagnostics it was written from

A report has: a summary for you and the plain one the user was shown, the
findings (each with evidence quoted from the diagnostics and a confidence),
patch instructions per file and function, workarounds you can pass on, and
what information is missing. A patch marked "Claude did not see this
function's source" is described by intent only. Read every report as written by
a model: check the evidence it quotes before you change code.

If the analysis failed (the API was down three times, or the model declined),
the report says why and the diagnostics are kept:

    node scripts/doctor-admin.mjs retry <ticket>     # analyse the stored diagnostics again
    node scripts/doctor-admin.mjs delete <ticket>
    node scripts/doctor-admin.mjs codes              # every code's state (open, used, expired, revoked)

The same things are plain HTTP under `/admin/*` with
`Authorization: Bearer <ADMIN_TOKEN>`; the list is at the top of `src/index.js`.
Reports and their diagnostics are deleted after `RETAIN_DAYS` (30).

## Settings (`wrangler.toml`, `[vars]`)

Change them in `wrangler.toml` and run `npx wrangler deploy` again.

- `MODEL`: the Claude model, `claude-opus-5-5` by default.
- `EFFORT`: how hard it thinks: `low`, `medium`, `high` (the default here),
  `xhigh` or `max`. Without it, `claude-opus-5-5` would use `medium`.
- `MAX_OUTPUT_TOKENS`: the most tokens of one answer, thinking included. An
  answer cut off at this limit is marked failed and not tried again on its own;
  raise the limit (or lower `EFFORT`), deploy, and `retry` the report.
- `FALLBACKS`: `on` sends `fallbacks: "default"` with the beta header
  `server-side-fallback-2026-07-01`. If a safety classifier declines the request
  (crash logs and driver talk can look like security work), Anthropic runs it
  again on the model it recommends for that case, in the same call. `off` leaves
  both out, and a declined request becomes a failed report.
- `REPO`: where Glint's source is read from, at tag `v<version>`, else `main`.
- `MAX_UPLOAD_KB`, `CODE_HOURS`, `MAX_RUNS_PER_DAY`, `RETAIN_DAYS`.
- `USER_SUMMARY`: `off` if the user should only ever see "Sent. Thank you!".
- `DISCORD_PINGS`: Discord user ids to mention in the post, separated by commas.

## How it works

- `src/index.js`: the routes, the size cap, minting, the admin key check.
- `src/store.js`: the Durable Object. Codes (hashes only), reports, and the
  alarm that runs the analysis after the user has been answered. Marking a code
  used and storing the upload happen in one atomic step.
- `src/analyze.js`: picks source files from the faults, fetches them from
  GitHub, builds the prompt, and makes the streamed Messages API call.
- `src/prompt.js`: the system prompt with Glint's module map, and the report's
  JSON schema.
- `src/scrub.js`: the second scrubbing pass, for an old or altered tool.
- `src/deliver.js`: the report as Markdown, and the Discord post.

If you add a module to Glint or change a log prefix, add it to `MODULES` in
`src/prompt.js`. That table is both what Claude knows about the code and what
picks the files it is shown.

## Testing

    cd server/doctor
    npm install --no-save wrangler     # once; node_modules is git-ignored
    bash test/run.sh

This runs the unit tests (`node --test test/*.test.mjs`: the scrubbing, the
API request and the reading of its stream), then the whole flow against
`wrangler dev` with fresh local storage and `test/mock.mjs` standing in for
Claude, GitHub and Discord. Nothing real is called and nothing is billed.
