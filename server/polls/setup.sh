#!/usr/bin/env bash
# Glint polls: set up (or update) the polls server on Ubuntu 22.04 / 24.04.
#
#   sudo bash setup.sh polls.example.com
#
# Run it from the folder that holds glint_polls.py. The domain must already
# point at this server (a DNS A or AAAA record, "DNS only": no Cloudflare proxy),
# so Caddy can get a certificate.
#
# What it does, in order:
#   1. installs Python 3
#   2. makes a system user "glintpolls" that runs the program and owns nothing else
#   3. copies glint_polls.py to /opt/glint-polls; the database lives in /var/lib/glint-polls
#   4. makes the admin key once, in /etc/glint-polls/admin.key (readable by root and the service only)
#   5. installs a systemd service that keeps the program running, and checks that it answers
#   6. Caddy (HTTPS in front of the program): first checks that no other web server holds
#      ports 80 / 443, then installs Caddy from its own apt repository, and adds this domain.
#      The new Caddy configuration is checked as a copy first; your Caddyfile is only
#      replaced when the copy is valid.
#   7. opens ports 80 and 443 in ufw, only if ufw is active (and Caddy is set up)
#   8. prints the admin page's address, and the key the first time only
#
# Running it again updates the program and keeps the key and the database.

set -euo pipefail

DOMAIN="${1:-}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR=/opt/glint-polls
DATA_DIR=/var/lib/glint-polls
KEY_DIR=/etc/glint-polls
KEY_FILE="$KEY_DIR/admin.key"
SERVICE=glint-polls
USER_NAME=glintpolls
CADDY_DIR=/etc/caddy
CADDYFILE="$CADDY_DIR/Caddyfile"
CADDY_SITE="$CADDY_DIR/glint-polls.caddy"
OURS_MARK="# Written by Glint polls setup.sh."

# --- 0. Checks: run as root, with a plain domain name, next to glint_polls.py.
if [ "$(id -u)" -ne 0 ]; then
    echo "Run this as root: sudo bash setup.sh your.domain" >&2
    exit 1
fi
if ! [[ "$DOMAIN" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)+$ ]]; then
    echo "Give the domain as the only argument, for example: sudo bash setup.sh polls.example.com" >&2
    exit 1
fi
if [ ! -f "$HERE/glint_polls.py" ]; then
    echo "glint_polls.py was not found next to setup.sh ($HERE)." >&2
    exit 1
fi

echo "==> 1. Installing Python 3"
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q python3 curl iproute2

echo "==> 2. The system user $USER_NAME"
if ! id "$USER_NAME" >/dev/null 2>&1; then
    useradd --system --home-dir "$DATA_DIR" --no-create-home --shell /usr/sbin/nologin "$USER_NAME"
fi

echo "==> 3. The program in $APP_DIR, the database in $DATA_DIR"
install -d -o root -g root -m 755 "$APP_DIR"
install -o root -g root -m 644 "$HERE/glint_polls.py" "$APP_DIR/glint_polls.py"
install -d -o "$USER_NAME" -g "$USER_NAME" -m 750 "$DATA_DIR"

echo "==> 4. The admin key"
install -d -o root -g "$USER_NAME" -m 750 "$KEY_DIR"
NEW_KEY=0
if [ ! -s "$KEY_FILE" ]; then
    # 32 random bytes, URL-safe. Written with a strict umask so it is never readable by others.
    ( umask 077; python3 -c 'import secrets; print(secrets.token_urlsafe(32))' > "$KEY_FILE" )
    NEW_KEY=1
fi
chown root:"$USER_NAME" "$KEY_FILE"
chmod 640 "$KEY_FILE"

echo "==> 5. The systemd service $SERVICE"
cat > "/etc/systemd/system/$SERVICE.service" <<EOF
[Unit]
Description=Glint polls server
After=network.target

[Service]
User=$USER_NAME
Group=$USER_NAME
ExecStart=/usr/bin/python3 $APP_DIR/glint_polls.py --host 127.0.0.1 --port 8787 --db $DATA_DIR/polls.db --key-file $KEY_FILE
Restart=always
RestartSec=3
# Lock the program in: no new privileges, the whole system read-only but its data folder.
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=$DATA_DIR
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
LockPersonality=true
# Hard ceilings: at most 128 threads (one per open connection) and 256 MB of memory.
TasksMax=128
MemoryMax=256M

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable "$SERVICE" >/dev/null
systemctl restart "$SERVICE"
ok=0
for _ in 1 2 3 4 5 6 7 8 9 10; do
    if curl -fsS http://127.0.0.1:8787/v1/health >/dev/null 2>&1; then ok=1; break; fi
    sleep 1
done
if [ "$ok" -ne 1 ]; then
    echo "The program did not start. See: journalctl -u $SERVICE -n 50" >&2
    exit 1
fi

# --- 6. Caddy. Runs as a function so that a failure here still lets step 8 print
# the admin key note (the program itself is running). CADDY_ERR says what went wrong.
CADDY_ERR=""

# Programs other than Caddy that listen on port 80 or 443 ("nginx", "apache2", ...).
others_on_web_ports() {
    ss -ltnpH 2>/dev/null | awk '$4 ~ /:(80|443)$/' | grep -v '"caddy"' | grep -o 'users:(("[^"]*"' | sed 's/users:(("//; s/"$//' | sort -u | tr '\n' ' '
}

# True when the Caddyfile is still the example page the caddy package installs (one ":80" site).
stock_caddyfile() {
    grep -q '^:80 {' "$CADDYFILE" && grep -q 'root \* /usr/share/caddy' "$CADDYFILE" && [ "$(grep -c '{' "$CADDYFILE")" -le 1 ]
}

setup_caddy() {
    local others
    others="$(others_on_web_ports)"
    if [ -n "$others" ]; then
        CADDY_ERR="ports 80/443 are already used by: ${others}. Caddy was not installed or changed.
    Either stop that program (for example: sudo systemctl disable --now nginx) and run setup.sh again,
    or let that program pass https://$DOMAIN to http://127.0.0.1:8787 itself (see README.md)."
        return 1
    fi

    if ! command -v caddy >/dev/null 2>&1; then
        echo "    Installing Caddy from its official repository"
        # https://caddyserver.com/docs/install#debian-ubuntu-raspbian
        apt-get install -y -q debian-keyring debian-archive-keyring apt-transport-https gnupg || { CADDY_ERR="apt could not install Caddy's prerequisites"; return 1; }
        curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg || { CADDY_ERR="could not fetch Caddy's signing key"; return 1; }
        curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list || { CADDY_ERR="could not fetch Caddy's repository list"; return 1; }
        chmod o+r /usr/share/keyrings/caddy-stable-archive-keyring.gpg /etc/apt/sources.list.d/caddy-stable.list
        apt-get update -q && apt-get install -y -q caddy || { CADDY_ERR="apt could not install Caddy"; return 1; }
    fi

    # Build the new configuration as copies next to the real files, check the copies, and only
    # then put them in place. Your own Caddyfile is not touched unless the check passes.
    local new_site="$CADDY_DIR/.glint-polls.caddy.new" new_file="$CADDY_DIR/.Caddyfile.glint-polls.new" own=0
    cat > "$new_site" <<EOF
# Glint polls (written by setup.sh; running setup.sh again rewrites it).
$DOMAIN {
	request_body {
		max_size 4KB
	}
	reverse_proxy 127.0.0.1:8787
}
EOF
    if [ ! -f "$CADDYFILE" ] || stock_caddyfile || [ "$(head -n 1 "$CADDYFILE")" = "$OURS_MARK" ]; then
        # The Caddyfile is missing, the package's example, or ours: write it whole, with time
        # limits for slow clients (they belong in Caddy's global options, the file's first block).
        own=1
        cat > "$new_file" <<EOF
$OURS_MARK
{
	servers {
		timeouts {
			read_header 10s
			read_body 10s
		}
	}
}
import $new_site
EOF
    else
        # Your own Caddyfile: keep it as it is and add one import line at its end. Its global
        # options are yours, so no time limits are added there (glint_polls.py has its own: 15 s).
        cp -p "$CADDYFILE" "$new_file"
        if ! grep -qF "import $CADDY_SITE" "$CADDYFILE"; then
            printf '\n# Glint polls (added by setup.sh)\nimport %s\n' "$new_site" >> "$new_file"
        else
            sed -i "s#import $CADDY_SITE#import $new_site#" "$new_file"
        fi
    fi
    local why
    if ! why="$(caddy validate --config "$new_file" --adapter caddyfile 2>&1)"; then
        rm -f "$new_site" "$new_file"
        CADDY_ERR="the new Caddy configuration did not pass 'caddy validate', so nothing of Caddy's was changed.
    Caddy said: $(printf '%s' "$why" | grep -i 'error' | tail -n 3)
    (A common cause: your Caddyfile already has a site for $DOMAIN.)"
        return 1
    fi
    # Valid: put the files in place (the import points at the final name).
    sed -i "s#import $new_site#import $CADDY_SITE#" "$new_file"
    if [ "$own" -eq 0 ] && [ ! -f "$CADDYFILE.before-glint-polls" ]; then
        cp -p "$CADDYFILE" "$CADDYFILE.before-glint-polls"
    fi
    mv -f "$new_site" "$CADDY_SITE"
    mv -f "$new_file" "$CADDYFILE"
    systemctl enable caddy >/dev/null 2>&1 || true
    if ! { systemctl reload caddy 2>/dev/null || systemctl restart caddy; }; then
        CADDY_ERR="Caddy did not start with the new configuration. See: journalctl -u caddy -n 50"
        return 1
    fi
    return 0
}

echo "==> 6. Caddy: HTTPS for $DOMAIN, passed on to 127.0.0.1:8787"
CADDY_OK=1
setup_caddy || CADDY_OK=0

echo "==> 7. Firewall"
if [ "$CADDY_OK" -ne 1 ]; then
    echo "    Skipped: Caddy is not set up."
elif command -v ufw >/dev/null 2>&1 && ufw status | grep -q '^Status: active'; then
    ufw allow 80/tcp >/dev/null
    ufw allow 443/tcp >/dev/null
    echo "    ufw is active: opened ports 80 and 443."
else
    echo "    ufw is not active: nothing changed. (A firewall at your VPS provider must allow ports 80 and 443.)"
fi

echo
if [ "$CADDY_OK" -eq 1 ]; then
    echo "Glint polls is running."
else
    echo "The polls program is running on 127.0.0.1:8787, but HTTPS is NOT set up:"
    echo "    $CADDY_ERR"
    echo "Until that is fixed the addresses below do not answer from outside."
fi
echo "  Admin page:  https://$DOMAIN/admin"
echo "  For Glint:   https://$DOMAIN   (this line goes into server/polls/address.txt in the repository)"
if [ "$NEW_KEY" -eq 1 ]; then
    echo
    echo "  Admin key (shown this once; keep it somewhere safe):"
    echo
    echo "      $(cat "$KEY_FILE")"
    echo
    echo "  To see it again later: sudo cat $KEY_FILE"
else
    echo "  The admin key is unchanged. To see it: sudo cat $KEY_FILE"
fi
[ "$CADDY_OK" -eq 1 ]
