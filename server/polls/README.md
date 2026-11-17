# Glint polls server

A small server for asking Glint's users questions. You create a poll on a
private admin page; Glint shows it to its users as a small card, and you watch
the votes come in live. Users can turn polls off in Glint's settings
("Polls from the developer"); then Glint never contacts this server.

- `glint_polls.py`: the whole program (Python 3, standard library only). It
  listens on 127.0.0.1:8787 and keeps everything in one SQLite file.
- `setup.sh`: installs it on Ubuntu 22.04 or 24.04, with Caddy in front for HTTPS.
- `address.txt`: the server's address for Glint (one line, `https://...`).
  Empty means "no polls": Glint shows nothing.
- `test_polls.py`: tests (`python3 -m unittest -v test_polls`, in this folder).

What is stored about a voter: the random number Glint made for that install,
the answers picked, the comment (if the poll allows one) and the time. No IP
address is stored and no requests are logged. The abuse limits (60 requests a
minute and 10 votes a day per poll from one IP, 5 wrong admin keys lock an IP
out for 10 minutes) are counted in memory only.

## Setting it up

1. Point a domain (or subdomain) at the VPS: a DNS `A` record such as
   `polls.example.com -> <the VPS's IP>`. Wait until it resolves.
   If the domain is on Cloudflare, set that record to "DNS only" (grey cloud).
   Do not put Cloudflare's proxy (orange cloud) in front, or every voter shares
   a few Cloudflare IPs and the per-IP limits block real people.
2. Copy this folder to the VPS and run, in it:

       sudo bash setup.sh polls.example.com

   It installs Caddy, a service user `glintpolls`, the program under
   `/opt/glint-polls`, the database under `/var/lib/glint-polls`, a systemd
   service `glint-polls`, and opens ports 80 and 443 if `ufw` is active.
   At the end it prints the admin page's address and the admin key (once).
3. Open `https://polls.example.com/admin`, paste the key, create a poll.
4. Put `https://polls.example.com` into `server/polls/address.txt` in the Glint
   repository (main branch). Glint reads that file at most once a day.

Running `setup.sh` again (for example after updating `glint_polls.py`)
updates the program and keeps the key and the database.

If you already have a Caddyfile of your own, setup.sh keeps it, adds one
`import /etc/caddy/glint-polls.caddy` line, and checks the result with
`caddy validate` before anything is replaced (your old file is kept as
`Caddyfile.before-glint-polls`). If the check fails, nothing of Caddy's changes
and the script prints Caddy's reason.

If another web server (nginx, Apache) already uses ports 80 and 443, setup.sh
stops before touching Caddy and says which program it is. Either stop that
program and run setup.sh again, or let it pass `https://polls.example.com` to
`http://127.0.0.1:8787` itself (it must set `X-Forwarded-For` to the visitor's
address and limit request bodies to 4 KB).

Time limits: the program drops a client that stays silent for 15 s; when
setup.sh writes the whole Caddyfile it also gives Caddy 10 s limits for reading
headers and bodies. The service may use at most 128 threads and 256 MB.

## What it does not prevent

There are no accounts, so someone who wants to see the results before voting
can cast a throwaway vote from a made-up install number. That is accepted: the
votes are a gauge, not an election, and the per-IP limits keep it small.

## The admin key

To see it again:

    sudo cat /etc/glint-polls/admin.key

To make a new one (the old one stops working):

    sudo rm /etc/glint-polls/admin.key && sudo bash setup.sh polls.example.com

## Backup

Everything is in one file: `/var/lib/glint-polls/polls.db`. Stop the service
for a moment, copy the file, start it again (Glint simply finds no polls for
those few seconds):

    sudo systemctl stop glint-polls
    sudo cp /var/lib/glint-polls/polls.db /root/polls-backup-$(date +%F).db
    sudo systemctl start glint-polls

To restore, stop the service, copy the backup back to
`/var/lib/glint-polls/polls.db`, `sudo chown glintpolls:glintpolls` it, and start
the service.

## Checking on it

    systemctl status glint-polls
    journalctl -u glint-polls -n 50
    curl -s https://polls.example.com/v1/health

## Removing it all

    sudo systemctl disable --now glint-polls
    sudo rm /etc/systemd/system/glint-polls.service && sudo systemctl daemon-reload
    sudo rm -r /opt/glint-polls /var/lib/glint-polls /etc/glint-polls
    sudo userdel glintpolls
    sudo rm /etc/caddy/glint-polls.caddy
    # then delete the "import /etc/caddy/glint-polls.caddy" line from /etc/caddy/Caddyfile
    sudo systemctl reload caddy
    # Caddy itself, if nothing else uses it:  sudo apt remove caddy

Empty `address.txt` in the repository as well, so Glint stops asking.
