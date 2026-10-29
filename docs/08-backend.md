# 08. Backend and signaling

The server (`backend/`, binary `aerolink`) authenticates operators and robots, keeps the robot list,
and relays WebRTC signaling. **It never carries media or control**: the console and the robot talk
peer-to-peer over WebRTC (via a TURN relay only when NAT leaves no direct path). Static files, TLS,
compression and abuse filtering belong to nginx ([09](09-deployment.md)).

```
console ──HTTPS /api/──►┐                         ┌◄──WSS /ws/robot── robot (phone)
console ──WSS /ws/operator──► nginx ──unix──► aerolink
console ◄════════════ WebRTC: video, audio, DataChannels "ctl" + "drive" ════════════► robot
```

## Process model

- `SRV-001` One process, **one thread**: tokio current-thread runtime. The load is a few WebSocket messages
  per session; shared state sits behind std mutexes that are never held across `.await`.
- `SRV-002` Listens on a Unix socket passed by systemd (socket activation, `aerolink.socket`): restarts never
  refuse connections, no TCP port is opened. `AEROLINK_LISTEN=unix:/path` or `host:port` for development.
- `SRV-003` Storage: SQLite (WAL), queries run inline on the runtime thread. Schema steps are appended to
  `db.rs` `MIGRATIONS`, tracked by `PRAGMA user_version` (until the first release the schema is edited in place, see [06](06-roadmap.md)). Tables: `users` (with `role`, `disabled`, ready for
  registration), `robots` (token hash), `sessions` (login sessions).
- `SRV-004` No timers and no polling on the server: login-session cleanup piggybacks on logins, robots and
  consoles keep their own WebSocket alive.
- `SRV-005` Argon2id (19 MiB, t=2) costs ~30 ms of the only thread per login; nginx limits `/api/v1/login`
  to 6/min per IP so this cannot be used to stall signaling.

## Configuration

`/etc/aerolink/aerolink.env` (`KEY=value`, root:aerolink 0640, never in git), path overridable with
`AEROLINK_CONFIG`; real environment variables win. The service and the CLI read the same file.

| Key | Default | Meaning |
|-----|---------|---------|
| `AEROLINK_ORIGIN` | empty (no check) | Public origin of the console, e.g. `https://aerolink.example.com`; operator WebSockets with another `Origin` are refused (CSWSH) |
| `AEROLINK_TELEGRAM_TOKEN`, `AEROLINK_TELEGRAM_CHAT_ID` | empty (off) | Bot for error reports |
| `AEROLINK_STUN_URLS`, `AEROLINK_TURN_URLS` | empty | Comma-separated ICE server URLs given to both peers |
| `AEROLINK_TURN_SECRET` | empty | coturn `static-auth-secret`; credentials are `<expiry>:<op42 or robot7>` + HMAC-SHA1, valid 24 h |
| `AEROLINK_LISTEN` | `systemd` under systemd, else `127.0.0.1:8080` | `systemd`, `unix:/path` or `host:port` |
| `AEROLINK_DATABASE` | `/var/lib/aerolink/aerolink.db` | SQLite file |
| `AEROLINK_ERROR_LOG` | `/var/log/aerolink/error.log` | Error log file |
| `AEROLINK_SECURE_COOKIES` | `true` | `false` only for plain-http development |
| `AEROLINK_LOG` | `info` | tracing filter for journald output |
| `AEROLINK_GUARD_ALLOW` | empty | Addresses/networks `aerolink guard` never bans ([09](09-deployment.md) OPS-013) |
| `AEROLINK_GUARD_LISTEN` | `systemd` under systemd, else `unix:/run/aerolink-guard.sock` | Datagram socket for nginx's security log |
| `AEROLINK_GUARD_NFT` | `nft` | nftables binary |
| `AEROLINK_APK_DIR` | `/var/lib/aerolink-apk` | Robot app release: APKs + `aerolink-release.json` (`ROB-008`) |
| `AEROLINK_APK_ACCEL` | empty; `/_apk/` in `aerolink.service` | nginx internal location for `X-Accel-Redirect`; empty = the backend sends the APK itself (bench) |

## Errors

- `SRV-010` Every log line goes to stderr (journald). Every **ERROR** (including panics) is also appended to
  `AEROLINK_ERROR_LOG` and sent to Telegram.
- `SRV-011` Telegram messages are batched (2 s window, ≥ 5 s apart, ≤ 3900 chars, `retry_after` on 429), so an
  error storm costs a few requests. The queue is bounded; overflow is dropped from Telegram only, never from
  the file. On shutdown (or a failed start) the queue is flushed before exit. Telegram's own failures are
  logged to the file only (no loops) and never contain the bot token.
- `SRV-012` `aerolink telegram-test` sends a test message with the configured bot.

## Management CLI

Run as the service user so the database stays owned by it: `sudo -u aerolink aerolink …`.

```
aerolink operator add <email> <name>      password from stdin (≥ 10 chars; no echo on a terminal)
aerolink operator passwd|disable|enable <email>   passwd/disable log out every session
aerolink operator list
aerolink robot add <name>                 prints the robot token once: "<id>.<secret>"
aerolink robot token <id>                 new token, the old one stops working
aerolink robot disable|enable <id>
aerolink robot list
aerolink telegram-test
aerolink guard [serve]                    firewall bans from nginx's security log (aerolink-guard.service)
aerolink guard list | unban <ip>          as root
aerolink completion bash                  bash completion script (the .deb installs it)
aerolink help [command]                   overview, or the details of a command (= aerolink <command> --help)
```

`help`, `version` and `completion` work without reading the configuration, so for any user. Bash completion
covers commands and subcommands; robot ids, operator e-mails and banned addresses are completed from the
`list` commands, i.e. only for users who can read the database (root, `aerolink`) or run `nft` (root).

Only hashes are stored: Argon2id for passwords, SHA-256 for the 256-bit session and robot tokens.

## HTTP API

JSON, under `/api/v1/`, body ≤ 16 KiB. Errors are `{"error": "<code>"}` with the HTTP status.

| Method, path | Auth | Result |
|--------------|------|--------|
| `POST /api/v1/login` `{email, password}` | — | `200 {id, email, name, role}` + session cookie; `401` |
| `POST /api/v1/logout` | cookie | `204`, cookie cleared |
| `GET /api/v1/me` | cookie | `200` user, `401` |
| `GET /api/v1/health` | localhost only (nginx) | `{status, version, operators, robots_online}` |
| `GET /api/v1/robot/apk/<abi>` | robot token (`Bearer`) | APK of the current release (`ROB-008`); `404` for an unknown ABI |

- `SRV-020` Session cookie `__Host-aerolink`: `HttpOnly; Secure; SameSite=Strict; Path=/`, 30 days.
  CSRF: `SameSite=Strict`, JSON-only bodies (a cross-site form cannot send `application/json`) and the Origin
  check on the operator WebSocket.
- `SRV-021` Unknown e-mails take as long as wrong passwords (a dummy Argon2 check).

## Signaling

Two WebSocket endpoints, JSON text messages with a `type` field. Limits per connection: message ≤ 64 KiB,
100 messages burst / 20 per s sustained, outgoing queue 128 (a client that far behind is dropped);
violations close the connection with code 1008.

- `/ws/operator` — session cookie + `Origin` check. Silent for 2 min → closed; the console pings every 30 s.
- `/ws/robot[?session=<id>]` — header `Authorization: Bearer <id>.<secret>`. Silent for 10 min → closed; the
  robot pings every 2–4 min (`PWR-012`). A new connection of the same robot replaces the old one; `session` is the
  session the robot is still in (`SIG-007`).

Rules:

- `SIG-001` A robot is controlled by at most one operator (`ARCH-001`): `take` on a robot held by another
  operator answers `error busy`. Taking a robot you already hold re-announces the same session.
- `SIG-002` SDP and ICE are relayed only between the two ends of a session; anything else is dropped
  (operators get `error no_session`). The server does not parse SDP.
- `SIG-003` A session ends on `release`, on robot `end`, or when either WebSocket closes (for the robot: unless
  it is replaced first, `SIG-007`). Both ends hear `end` (except the side that is gone). The robot then closes
  the peer connection and stops (`SAFE-010`).
- `SIG-004` Robot status (`status`) is stored and broadcast to every operator only when it changes; robots
  send it on change only (no periodic chatter, `PWR-012`).
- `SIG-005` The console makes the offer; the robot answers. ICE is trickled both ways.
- `SIG-006` All robots are visible to all operators for now; rental/ownership will add an access table.
- `SIG-007` Network change (Wi-Fi ↔ cellular, cell handover): the robot's old connection usually goes silent
  rather than closing, so the robot reconnects with `?session=<id>` and replaces it. If that session is still
  the robot's, it carries on: the robot hears `session` again (fresh TURN credentials), the operator hears
  `resumed` and restarts ICE with a new offer. Otherwise the robot hears `end` for it (`robot_offline`). The
  server keeps no timers for this; if it noticed the old connection close first, the session is already over.
  The console also restarts ICE by itself after 3 s of `disconnected` or on `failed` (backing off 5 … 30 s).

Robot → server:

| `type` | Fields | |
|--------|--------|--|
| `status` | `mode` (`IDLE`, `TELEOP`, `AUTO`, `NEEDS_HELP`), `phone_battery` %, `robot_battery_mv`, `charge_control` (bool), `app`, `firmware` — all optional | on change |
| `answer` | `session`, `sdp` (RTCSessionDescriptionInit) | |
| `ice` | `session`, `candidate` (RTCIceCandidateInit) | |
| `end` | `session` | robot ends the session |
| `ping` | | answered with `pong` |

Operator → server: `take {robot}`, `offer {session, sdp}`, `ice {session, candidate}`, `release {session}`, `ping`.

Server → both:

| `type` | Fields | |
|--------|--------|--|
| `welcome` | `you {id, name}` | first message |
| `robots` | `robots: [RobotView]` | operators, after `welcome` |
| `robot` | `robot: RobotView` | operators, on any change |
| `session` | `session`, `robot`, `peer {id, name}`, `ice_servers` (RTCIceServer[] with TURN credentials) | to both ends |
| `offer` / `answer` / `ice` | as sent by the peer | |
| `end` | `session`, `reason` (`released`, `robot_ended`, `robot_offline`, `operator_offline`) | |
| `resumed` | `session` | operators: the robot reconnected and kept the session, restart ICE (`SIG-007`) |
| `update` | `version_code`, `version_name`, `apks: {<abi>: {size, sha256}}` | robots, after `welcome` and on `systemctl reload` when the release changed (`ROB-008`) |
| `error` | `code` (`not_found`, `offline`, `busy`, `no_session`), `robot`? | operators |
| `pong` | | |

`RobotView = {id, name, online, status, operator}` (`operator` = user id holding the robot, or `null`).

## Control channel (console ↔ robot, peer-to-peer)

The console's offer carries receive-only video (H.264 first in its codec preferences: the sender takes the
receiver's first codec it can encode), two-way audio (the operator's microphone is attached only while talking,
by replacing the track, so talking needs no renegotiation) and two DataChannels, JSON messages:

- `CTL-001` **`ctl`** (reliable, ordered). Console → robot: `{"t":"arm"}`, `{"t":"disarm"}`,
  `{"t":"ping","ts":…}` every second. Robot → console: `{"t":"pong","ts":…}` (echo, gives the round trip),
  `{"t":"telemetry","base":true,"state":"DISARMED|ARMED|STOPPING|FAULT","faults":…,"speed_l":…,"speed_r":…,
  "battery_mv":…,"phone_battery":…,"light":…}` at ~5 Hz during a session and at once when `base`, `state`,
  `faults` or `light` change (the console waits for `ARMED` before it streams DRIVE); `light` is absent when the
  camera has no flashlight. `state`/`faults` as in [03](03-usb-link.md);
  `base: false` = no usable robot base on the phone (video only, the console does not offer ARM).
- `CTL-002` **`drive`** (unordered, `maxRetransmits: 0`: a late DRIVE is worse than a lost one).
  `{"seq":n,"ts":<ms epoch>,"speed":‰,"steer":‰}`, speed + = forward, steer + = right, sent at 20 Hz only
  while telemetry says `ARMED`. The robot drops `seq` ≤ last seen (`SAFE-012`) and treats > 500 ms without
  DRIVE as link loss (`SAFE-010`). The phone forwards it to the RP2040 as DRIVE with `source = OPERATOR`.
- `CTL-003` Console safety: `Space` / gamepad B → `disarm`; a hidden tab → `disarm`; losing focus forgets held
  keys; keyboard speed is 50 % unless Shift is held.
- `CTL-004` Light: `{"t":"light","on":true|false}` (console key `L`) switches the flashlight of the camera in
  use; it goes off with the session. Talk: key `T` toggles the operator's microphone; the robot plays it on the
  loudspeaker.

Bench/diagnostics: `?relay` in the console URL forces the media path through TURN (`iceTransportPolicy: relay`).

`tools/e2e_console.mjs` contains a fake robot implementing this side (used by `make e2e`).

## Robot app side

The phone's implementation (`android/`, packages `signal`, `teleop`, `rtc`, `service`):

- `ROB-001` Provisioning: server origin and robot token are entered on the diagnostics screen and kept in the
  app's private preferences. A provisioned phone runs `RobotLinkService` all the time (started from the app, on
  boot and on accessory attach; `START_STICKY`), with or without the robot base attached.
- `ROB-002` Signaling: one WebSocket to `/ws/robot` (OkHttp), keepalive = WebSocket ping every 3 min (`PWR-012`;
  the server counts pings as activity). Reconnect with exponential backoff 1 s … 60 s with jitter, at once when a
  network comes up (Wi-Fi ↔ cellular); a rejected token (401) waits 10 min. `status` is sent on change and again
  after every reconnect (robot battery in 0.1 V steps so telemetry does not cause chatter).
- `ROB-003` Session: `session` → offer → answer with the back camera (Camera1 on phones with LEGACY Camera2,
  e.g. the SM-J710F) and the microphone; the operator's voice plays on the loudspeaker (communication mode, so
  hardware echo cancellation applies; volume as set on the phone). ICE that arrives before the answer is
  buffered. Later offers in the session are ICE restarts: same camera and channels, new ICE (`SIG-007`); ICE
  gathers continually, so a new network often takes over even before that. Peer connection `failed` → the
  robot sends `end`. Losing the signaling WebSocket keeps a running peer and reconnects with `?session=`; a
  session without a peer yet ends at once. Each session logs one summary line (bytes sent/received, ICE path).
- `ROB-007` Video: 1280×720 at 30 fps, bitrate as `PWR-013`. The frame rotation is pinned to the camera sensor
  orientation plus a mount rotation set on the diagnostics screen (0° = phone upright): WebRTC would otherwise
  follow the display rotation, which on a robot depends on whatever is on the screen. Light (`CTL-004`): Android
  cannot switch the torch of a camera in use, so the app changes the flash mode in WebRTC's own camera session
  (private fields, kept by ProGuard; Camera1 and Camera2); if that fails the light is reported off.
- `ROB-004` Resources: WebRTC factory, EGL, encoder, camera and microphone are created on the offer and released
  on `end`; the partial wake lock (≤ 4 h) and the camera/microphone foreground-service types are held only during
  a session (`PWR-012/013`).
- `ROB-005` Commands: `arm`/`disarm`/DRIVE go to `RobotLink` with `source = OPERATOR` through `DriveGate`:
  `SAFE-012` drops `seq` ≤ last (per session), `SAFE-010` stops (zero DRIVE, DISARM) after 500 ms without DRIVE;
  right after `arm` the console first needs the `ARMED` telemetry, so the first DRIVE may take up to 1.5 s.
  Session end and peer failure stop the robot the same way.
- `ROB-008` App updates: on `update` with a higher `version_code` the robot waits until no operator session runs,
  streams its ABI's APK into a `PackageInstaller` session while checking size and SHA-256, and commits it; a
  session starting meanwhile cancels the download (it restarts afterwards). Android checks the signature and
  that the version is higher. The system replaces the app and restarts the service (`MY_PACKAGE_REPLACED`);
  the robot reconnects with the new `app` version in `status`. Installs must be silent: Device Owner (`PWR-014`,
  [09](09-deployment.md) `OPS-030`), or on Android 12+ a self-update allowed to install unknown apps with a
  targetSdk the Android version accepts. Otherwise the diagnostics screen shows the update as blocked and nothing
  is downloaded. At most 3 attempts per release. An install takes a few seconds; an operator taking the robot in
  exactly that window loses the session (the base stops as on any link loss, `SAFE-010`).
- `ROB-009` Firmware updates ([03](03-usb-link.md) "Firmware update over the link"): the APK carries the RP2040
  image built with it. On every HELLO_ACK the robot first confirms an image on trial (`LINK-044`); when `fw_crc32`
  differs from the carried image (older or newer), it waits until no operator session runs and the base is
  DISARMED, sends the image (~1 s for 40 KB; not cancelled by a session starting, ARM is refused only meanwhile),
  waits for any session to end and sends REBOOT. The base comes back with the new image on trial and is confirmed.
  At most 3 attempts per image (a rollback counts as a failed one); progress and errors on the diagnostics
  screen, log tag `AeroLinkFirmware`. Works also when the protocol versions differ (that is what the update is
  for). The simulated base takes updates too (its "flash" lives as long as the app process, so a bench phone
  updates it after every app start).
- `ROB-006` Bench: debug builds allow plain `http://`/`ws://` and offer a simulated robot base
  (`SimulatedBase`: firmware state machine incl. the command timeout, telemetry, wheel speeds from DRIVE)
  when no RP2040 is attached. Release builds are TLS-only and never simulate.

## Fleet administration (planned, Stage 3a)

An **admin** (`users.role = 'admin'`) sees the robots of the fleets assigned to them and can intervene; operators
keep the console as it is.
Everything is pushed over the existing WebSockets on change: no polling by the browser, no timers on the server,
and the robot only sends when something changed (`PWR-012`).

Robot data:

- `ADM-001` `status` grows: phone model and Android version, network (`wifi`/`cellular`, operator, signal level
  in 4 steps), app and firmware versions, update states (`ROB-008/009`), Device Owner, uptime of the service.
  The battery is sent in 5 % steps outside a session (1 % in a session).
- `ADM-002` Location: robot → server `location {lat, lon, accuracy_m, at}`. In a session from the GPS provider,
  sent when moved ≥ 10 m; outside a session only the passive provider (fixes other apps already paid for) and one
  last fix when the session ends — the GPS is never switched on just for the map. Permission granted by Device
  Owner (`PWR-014`); the service adds the `location` foreground type only during a session.
- `ADM-003` Errors: robot → server `event {level, code, message, at}` for app crashes (written on the next start),
  base faults (`SAFE-006`), USB link errors, failed updates, low batteries, `NEEDS_HELP`. Events are queued on the
  phone while offline (bounded, oldest dropped) and sent after `welcome`. `error`-level events also go to Telegram
  (`SRV-010`), batched like server errors.
- `ADM-004` Storage: last known `status`, `location` and `last_seen` per robot in the database (written only when a
  message arrives), so offline robots show where and how they were last; events in `robot_events` (last 500 per
  robot, trimmed on insert); admin actions in `audit` (who, what, when).
- `ADM-005` Fleets: every robot belongs to one fleet (`fleets`, `robots.fleet_id`); an admin has a list of fleets
  (`user_fleets`) or the `all_fleets` flag, which also covers fleets created later. Every admin view, event,
  REST answer and action is limited to those fleets, checked by the server. CLI: `aerolink fleet add|list`,
  `aerolink robot fleet <id> <fleet>`, `aerolink operator fleets <email> <fleet,…|all>`. Operators' access to
  robots (today: all, `SIG-006`) moves to the same fleets with Q8.

Admin console (`/console/`, same login; the tab exists only for admins, the server checks the role on every
admin message):

- `ADM-010` Fleet table: online / last seen, mode, operator holding it, phone and robot battery, charge control,
  network, app / firmware version and update state, unread errors; sortable, filter by text.
- `ADM-011` Map of last known positions: Leaflet with public OpenStreetMap tiles (Q9, attribution shown); the
  console CSP gets `img-src https://tile.openstreetmap.org`. The tile URL is one setting, so a paid or
  self-hosted source replaces it later.
- `ADM-012` Robot page: all of the above, the event log, and actions:
  - **End session**: the operator hears `end` with reason `admin`; the robot stops (`SAFE-010`).
  - **Take over**: ends the operator's session the same way and opens one for the admin in the normal console
    view; the robot is DISARMED in between, the admin must ARM.
  - **Disable / enable**, **new token** (shown once), **rename**, **add robot** — the CLI stays as well.
- `ADM-013` Operators page: online, robots held right now; add, disable, reset password, change role.
- `ADM-014` Signaling: admin → server `admin_end {session}`, `admin_take {robot}` (as `take`, but a held robot is
  taken over), `admin_robot {robot, action, …}`, `admin_user {user, action, …}`; server → admin
  `fleet {robots: [RobotAdminView]}` after `welcome`, `fleet_robot` on change, `robot_event` on each new event.
  REST for history: `GET /api/v1/admin/robots/<id>/events?before=<id>`, `GET /api/v1/admin/audit`.
- `ADM-015` Tests: hub unit tests (role and fleet checks, take-over, end), end-to-end with an admin and an operator in
  `make e2e`, i18n parity for the new strings.

