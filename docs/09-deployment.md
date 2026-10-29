# 09. Deployment

One Debian server (13 "trixie" or newer) runs nginx, the `aerolink` service and, when robots sit behind
cellular NAT, coturn. `make backend` produces `build/aerolink.deb` with everything below.

## Package contents

| Path | |
|------|--|
| `/usr/bin/aerolink` | backend + management CLI ([08](08-backend.md)) |
| `/usr/share/aerolink/www/` | landing (`/`) and operator console (`/console/`), each file with `.br` and `.gz` next to it |
| `/usr/lib/systemd/system/aerolink.{socket,service}` | socket activation on `/run/aerolink.sock` (group `www-data`), hardened service |
| `/usr/lib/systemd/system/aerolink-guard.{socket,service}` | `aerolink guard`: datagram socket `/run/aerolink-guard.sock` for nginx's security log, nftables bans |
| `/etc/aerolink/aerolink.env` | configuration and secrets (conffile, root:aerolink 0640) |
| `/etc/nginx/sites-available/aerolink`, `/etc/nginx/snippets/aerolink-*.conf` | nginx site (not enabled automatically) |
| `/etc/logrotate.d/aerolink` | `/var/log/aerolink/*.log` |
| `/usr/share/bash-completion/completions/aerolink` | bash completion for the `aerolink` command (`aerolink completion bash` prints the same) |

Dependencies: `nginx (≥ 1.25.1)`, `libnginx-mod-http-brotli-static`, `nftables (≥ 1.0.8)`; recommended: `coturn`
(full list for build machine and server: [10](10-packages.md)).
No Python on the server: bans are done by the backend binary itself (`aerolink guard`), not fail2ban.

## Install

```sh
sudo apt install ./aerolink.deb
sudoedit /etc/aerolink/aerolink.env               # AEROLINK_ORIGIN, Telegram, TURN, AEROLINK_GUARD_ALLOW
sudo sed -i 's/aerolink.example.com/robots.example.org/g' /etc/nginx/sites-available/aerolink
sudo certbot certonly --webroot -w /var/www/letsencrypt -d robots.example.org
sudo ln -s ../sites-available/aerolink /etc/nginx/sites-enabled/aerolink
sudo rm /etc/nginx/sites-enabled/default            # the AeroLink site brings its own default_server
sudo nginx -t && sudo systemctl reload nginx
sudo -u aerolink aerolink operator add alice@example.org Alice
sudo -u aerolink aerolink robot add robot-01        # token goes into the robot app
sudo -u aerolink aerolink telegram-test
```

Robots update their app from this server ([App updates](#app-updates)); `/var/lib/aerolink-apk/` is created
by the package and stays empty until the first release.

The first certificate needs port 80 served by nginx; until the certificate exists comment out the 443
server (or use `certbot --standalone` with nginx stopped). Renewals reuse the webroot.
`certbot` pulls in Python; `uacme` (C) and `lego` (Go) are Python-free alternatives in Debian that use the same
webroot.

## nginx

- `OPS-001` **Static files are pre-compressed at build time** (`frontend/scripts/compress.mjs`: brotli q11 and
  gzip -9). nginx serves them with `brotli_static`/`gzip_static` and has `gzip off`, so it never compresses
  on the fly. Responses from the backend are small JSON and are not compressed either.
- `OPS-002` Caching: `/assets/*` (content-hashed) `immutable`, one year; HTML `no-cache` (revalidated by ETag);
  API `no-store`. `open_file_cache` keeps file descriptors and metadata.
- `OPS-003` Only `/api/` and `/ws/` reach the backend, over a keepalive Unix-socket upstream. WebSocket read
  timeout is 660 s (robots ping every 2–4 min). `/api/v1/health` answers only from localhost.
- `OPS-004` TLS 1.2/1.3, hybrid post-quantum key exchange first (`X25519MLKEM768`), shared session cache, no
  session tickets, 4 KiB records, HTTP/2. HSTS, CSP (`'self'` only, no inline scripts), `X-Frame-Options`,
  `nosniff`, `Referrer-Policy`, `Permissions-Policy` in `snippets/aerolink-headers.conf`.
- `OPS-005` Request hygiene: body ≤ 16 KiB, short header/body timeouts, `reset_timedout_connection`,
  ≤ 40 connections per IP.

### Abuse filtering and logs

- `OPS-010` Requests are refused with **444** (connection closed, no response) when:
  - the path is a classic exploit/recon target (`.php`, `.env`, `.git`, `wp-admin`, `phpmyadmin`, `cgi-bin`,
    `actuator`, backups/archives/keys, traversal, `${jndi:`, SQL injection markers, …) → reason `probe`;
  - the User-Agent is empty or a known scanner / abusive crawler (sqlmap, nikto, nmap, masscan, zgrab, nuclei,
    …) → `agent`;
  - the method is not GET/HEAD/POST/PUT/PATCH/DELETE → `method`;
  - the Host is not ours (bare-IP scans) → `nohost`. TLS without our SNI is refused during the handshake
    (`ssl_reject_handshake`), so the certificate does not reveal the domain to IP scanners.
- `OPS-011` Rate limits per IP (`429`): static 30 r/s (burst 100–200), API 10 r/s (burst 40),
  login 6/min (burst 5), WebSocket connects 20/min (burst 10).
- `OPS-012` Logs in `/var/log/nginx/` (rotated by Debian's nginx logrotate):
  - `aerolink-access.log` — normal traffic, buffered (64 KiB / 1 min); hashed assets are not logged;
  - `aerolink-security.log` — JSON lines (the same lines also go to `aerolink guard`, see OPS-013) `{t, ip, reason, status, method, host, uri, ua, ref, …}` for every
    refused request and every 400/401/403/404/405/408/413/414/429/444/49x/5xx: this is where bots and attacks
    show up (`jq -r .uri aerolink-security.log | sort | uniq -c | sort -rn | head` lists what they hunt);
  - `aerolink-error.log` — nginx errors and `limit_req` rejections.
- `OPS-013` **Bans** (`aerolink guard`, `backend/src/guard.rs`): nginx sends every security-log line as a
  syslog datagram to `/run/aerolink-guard.sock` (`snippets/aerolink-security-log.conf`); systemd owns the
  socket and starts the guard on the first line, so nothing tails or polls log files. A **strike** is a refused
  request (`probe`, `agent`, `method`, `nohost`), a 429/444, or a 401 on `/api/v1/login` or `/ws/robot`
  (password / token guessing; other 401s such as `/api/v1/me` are the console checking its login and do not
  count; 404s are logged only). **5 strikes within 10 min** put the client into nftables for **1 h**, doubled
  for each repeat (2 h, 4 h, … up to a week; a week without bans resets it). The ban unit is one IPv4 address
  or one IPv6 /64.
  - Firewall: table `inet aerolink_guard`, sets `ban4`/`ban6` with per-element timeouts, chain `input`
    (priority filter − 10) dropping TCP 80/443 from banned sources. The kernel expires bans; SSH and other
    ports are not affected. The table is independent of `/etc/nftables.conf`; if `systemctl restart nftables`
    flushes it, the next ban recreates it (current bans are lost then).
  - `AEROLINK_GUARD_ALLOW` lists addresses/networks never banned (loopback always is). A real operator
    mistyping the password 5 times is banned like anyone else.
  - Admin (as root): `aerolink guard list` (bans with remaining time), `aerolink guard unban <ip>`. Bans are
    logged as warnings to the journal (`journalctl -u aerolink-guard`); guard failures (e.g. `nft` errors) are
    errors: error log + Telegram like the backend's.
  - The guard runs as `aerolink` with `CAP_NET_ADMIN` only (inherited by the `nft` it spawns per ban), one
    thread, ~1–2 MiB. Strike counters live in memory; a restart forgets them but not the bans in the kernel.

### Recommended `nginx.conf` tweaks

These live outside `sites-available` (main/events context), so the package does not touch them:

```nginx
worker_rlimit_nofile 65535;
events { worker_connections 8192; }
```

Debian's defaults (`worker_processes auto`, `sendfile on`, `server_tokens off`) are kept. HTTP/3 (QUIC) is
possible with Debian's nginx (`listen 443 quic reuseport;` + UDP 443 open + `Alt-Svc`); not enabled because
the console is used from desktops and WebSockets do not run over HTTP/3 in nginx.

## systemd

- `OPS-020` `aerolink.socket` owns `/run/aerolink.sock` (0660 aerolink:www-data); the service starts on the
  first request and restarts without dropping new connections.
- `OPS-021` The service runs as `aerolink` with an empty capability set, read-only system
  (`ProtectSystem=strict`), private `/tmp` and devices, syscall filter `@system-service`, `MemoryMax=256M`,
  `TasksMax=8`. Writable: `/var/lib/aerolink` (state), `/var/log/aerolink` (error log). `systemctl reload
  aerolink` (SIGHUP) re-reads the app release without dropping any connection.
- `OPS-022` `aerolink-guard.socket` (datagram, 0660 aerolink:www-data, 1 MiB receive buffer) and
  `aerolink-guard.service` (see OPS-013), hardened the same way plus `CAP_NET_ADMIN` and `AF_NETLINK`.

## Admins

- `OPS-040` The first admin comes from the CLI; afterwards admins of all fleets manage users and fleets on the
  console's admin pages (`ADM-012/013`), and every change there is in the audit log:
  ```sh
  sudo -u aerolink aerolink operator add boss@example.com "Boss"     # password from stdin
  sudo -u aerolink aerolink operator role boss@example.com admin
  sudo -u aerolink aerolink operator fleets boss@example.com all
  sudo -u aerolink aerolink fleet add north                          # robots: aerolink robot fleet <id> north
  ```
  An admin of some fleets: `aerolink operator fleets <email> north,south`. The map loads public OpenStreetMap tiles
  in the admin's browser (Q9); the CSP in `snippets/aerolink-headers.conf` allows exactly that host.

## Robot phone

- `OPS-030` Provisioning (`PWR-014`), once per phone, **after a factory reset and before any Google or Samsung
  account is added** (Android refuses a Device Owner on a phone with accounts):
  ```sh
  adb install aerolink-armeabi-v7a.apk
  adb shell dpm set-device-owner com.aerolink.robot/.service.AdminReceiver
  ```
  Then open the app, enter the server address and robot token. As Device Owner the app grants itself camera,
  microphone, location (`ADM-002`) and notification permissions, switches off the lock screen (only without a PIN/pattern) and installs
  updates silently. "Remove Device Owner" on the diagnostics screen gives the phone back (decommissioning).
  Bench phones that cannot be reset keep working without it, minus silent updates.
- `OPS-033` Robot base (RP2040), once per board with a PC: hold BOOTSEL while plugging it into the PC, then
  `make flash` (`build/aerolink-rp2040.uf2`: loader, erased boot record, application). `picotool load -f` cannot
  reboot a running board into BOOTSEL (its USB port is the AOA host), so the button is always needed. From then
  on the robot app keeps the firmware up to date over the robot's own USB cable (`ROB-009`); a board that cannot
  start any image falls back to BOOTSEL by itself (`LINK-045`).

## App updates

- `OPS-031` Publishing a release (`ROB-008`): `make android` writes `build/aerolink-<abi>.apk` and
  `build/aerolink-release.json` (versionCode, versionName, size and SHA-256 of each APK). Raise `versionCode` in
  `android/app/build.gradle.kts` for every release; robots ignore anything not newer.
  ```sh
  scp build/aerolink-*.apk build/aerolink-release.json server:/tmp/
  sudo install -m 0644 -t /var/lib/aerolink-apk /tmp/aerolink-*.apk /tmp/aerolink-release.json
  sudo systemctl reload aerolink      # announces it to every connected robot
  ```
  A release that does not match its files (size) is rejected on reload and the previous one stays (error log +
  Telegram). Robots download with their token; nginx sends the file (`/_apk/`, internal).
- `OPS-032` Every APK must be signed with the same release key (`~/.gradle/gradle.properties`): Android refuses
  an update signed with another key, and the robot gives up on that release after 3 attempts.

## TURN (coturn)

Needed when a peer is behind symmetric NAT (common on cellular). Minimal `/etc/turnserver.conf`:

```
listening-port=3478
tls-listening-port=5349
fingerprint
use-auth-secret
static-auth-secret=<same as AEROLINK_TURN_SECRET>
realm=robots.example.org
cert=/etc/letsencrypt/live/robots.example.org/fullchain.pem
pkey=/etc/letsencrypt/live/robots.example.org/privkey.pem
no-cli
no-multicast-peers
denied-peer-ip=10.0.0.0-10.255.255.255
denied-peer-ip=172.16.0.0-172.31.255.255
denied-peer-ip=192.168.0.0-192.168.255.255
total-quota=200
```

The console shows whether a session goes `direct` or `via TURN relay`.

## Development

```sh
cd backend && AEROLINK_CONFIG=/dev/null AEROLINK_DATABASE=/tmp/al.db AEROLINK_ERROR_LOG=/tmp/al-error.log \
  AEROLINK_SECURE_COOKIES=false AEROLINK_ORIGIN=http://localhost:5173 cargo run
cd frontend && npm run dev        # http://localhost:5173/console/, proxies /api and /ws to 127.0.0.1:8080
```

`make e2e` runs the whole chain (backend, console build, fake robot with WebRTC in headless Chromium), then the
Android robot code (signaling, session control, simulated base, fake WebRTC peer) against the same backend.

### Bench test with a phone

```sh
make android ANDROID_VARIANT=debug install-apk   # debug: plain http/ws allowed, simulated robot base offered
make bench-server                                # backend on 0.0.0.0:8080 + console on http://localhost:5173
```

`make bench-server` prints the server address (this PC's LAN IP), the robot token and the console login; state
stays in `build/bench/`. On the phone: diagnostics screen → server address + token → Save; switch on the
simulated base when no RP2040 is attached. Phone and PC on the same LAN connect directly (no STUN/TURN needed).
Phone without Wi-Fi: `BENCH_USB=1 make bench-server` forwards the backend and a local coturn over the USB cable
(`adb reverse`), server address `http://127.0.0.1:8080`. coturn listens on 127.0.0.1 but relays on the PC's LAN
address (WebRTC does not pair loopback candidates). Media still goes direct when it can;
`http://localhost:5173/console/?relay` forces the TURN path.

App updates on the bench: copy `build/aerolink-*.apk` and `build/aerolink-release.json` (built with
`ANDROID_VERSION_CODE=<n>` higher than the installed one) into `build/bench/apk/`, then `pkill -HUP -f "aerolink
serve"`. A bench phone that is not Device Owner updates silently only on Android 12+ with "install unknown apps"
allowed (`adb shell appops set com.aerolink.robot REQUEST_INSTALL_PACKAGES allow`) and only to an APK whose
targetSdk meets that Android version's minimum (Android 17: 35; the app targets 34, so the test build raised it).

Measured on the Pixel 7 (Android 17):

- LAN: video plays ~1.2 s after "Take", round trip 12–19 ms; `SAFE-010` disarms ~0.6 s after DRIVE stops (as seen
  by the console); wake lock and camera are held only during the session.
- `?relay` (USB bench): video after ~0.9 s, 1280×720 @ 30 fps from the first seconds, hardware H.264
  (constrained baseline), ~1.45–1.5 Mbit/s (the cap); round trip 5–9 ms.
- Phone Wi-Fi switched off mid-session: the connection never left `connected` (continual gathering moved it to
  relay ↔ relay), and back when Wi-Fi returned; the console's ICE restart was not even needed. The signaling
  reconnect with `resumed` + ICE restart is covered by `make e2e` (fake robot) and the JVM tests.
- Talk (silent test file): the phone received the operator's audio (session summary in the log) and routed it to
  the loudspeaker in communication mode; mode and camera were back to idle after Release. Light: the camera's
  capture request had `flash.mode = TORCH`.
- App update (self-update, see above): announcement → 14 MB download and SHA-256 check → silent install → service
  back online ~25 s after the announcement (`MY_PACKAGE_REPLACED`). Announced during a session, the update waited
  and started 30 ms after Release. Device Owner itself is untested here (the Pixel is a personal phone).
Logs: `adb logcat -s AeroLinkService AeroLinkTeleop AeroLinkRtc AeroLinkUpdate AeroLinkFirmware`.
