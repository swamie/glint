# Glint share links

Optional. A Cloudflare Worker that stores clips in your own R2 bucket and gives
each one a link. Pasted in Discord, the link unfurls as an embed with a blue
side bar, the clip's title in bold, an "uploader · game" line and an inline
video player.

Glint only uses it after you put the Worker's address and upload key into
Settings → Share links. Without them, Glint never uploads anything.

## What it costs

Nothing on Cloudflare's free tier, as long as you stay inside it: R2 gives
10 GB of storage and free downloads, Workers give 100,000 requests a day. A
Discord view is a few requests. `MAX_MB` in `wrangler.toml` (9000 by default)
makes the Worker refuse uploads before the bucket passes it; lower it if you
want a smaller cap. Settings → Share links → Check shows how much is used.

## Setting it up (once)

You need a free Cloudflare account and Node.js.

1. In the Cloudflare dashboard, open **R2** and enable it (it asks for a card
   even on the free plan; nothing is charged within the free limits).
2. In this folder:

       npx wrangler login
       npx wrangler r2 bucket create glint-clips
       npx wrangler secret put UPLOAD_TOKEN
       npx wrangler deploy

   For `UPLOAD_TOKEN`, paste a long random string, for example the output of
   `node -e "console.log(require('crypto').randomBytes(32).toString('hex'))"`.
   `deploy` prints the address, such as `https://glint-share.<you>.workers.dev`.
3. In Glint: Settings → Share links. Paste the address and the same upload key,
   set your name, press **Check**.
4. Right-click a clip → **Share link**. The link is copied when the upload is done.

## Settings (`wrangler.toml`, `[vars]`)

- `EMBED_COLOR`: the embed's side bar, `#3B82F6` (blue).
- `MAX_MB`: most the bucket may hold, in MB.
- `EMBED_LINE`: where the "uploader · game" line goes. `description` puts it
  under the title. If Discord does not show it there, set `site_name`: Discord
  then shows it as a small grey line above the title.
- `PUBLIC_BASE` (optional): links use this address instead of the workers.dev
  one, if you add a custom domain to the Worker.

## Testing the embed

Discord caches each link's embed. After changing a setting and running
`npx wrangler deploy` again, paste the link with something new on the end,
such as `?v=2`, to see the new version.

## What matches Medal's embed and what does not

Matches: the colored side bar (blue here), the bold title, the "uploader ·
game" line, the large inline player with the clip's picture.

Does not: the two buttons under Medal's embed ("Follow on Medal", "Download for
PC"). Discord draws those only for its own partner apps; a web page cannot ask
for them. The share page itself has a Download button for people who open the
link in a browser.

## Limits worth knowing

- Discord may not play very large videos inline. Glint warns above 100 MB;
  trim or compress such clips first.
- Deleting a link (right-click → Delete from cloud) removes the video, but
  Discord may keep showing the picture in old messages for a while.
- An upload that Glint abandons (closed mid-upload) leaves parts that R2
  removes on its own after about a week.

## How it works

`src/index.js` is the whole Worker. Clips live under `c/<id>/` as
`video.mp4`, `thumb.jpg` and `meta.json`. Glint prepares an H.264/AAC MP4 with
the index at the front (copying H.264 clips as they are, encoding HEVC, AV1 or
VP9 ones), then uploads it in 16 MiB pieces through the Worker's multipart API,
so clips of any size fit under the Worker's 100 MB request limit.

`test/smoke.sh` checks the whole flow against `npx wrangler dev` (local, no
Cloudflare account needed):

    echo 'UPLOAD_TOKEN=test-key' > .dev.vars
    npx wrangler dev --port 8799 &
    bash test/smoke.sh http://127.0.0.1:8799 test-key some-video.mp4
