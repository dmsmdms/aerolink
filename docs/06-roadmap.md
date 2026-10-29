# 06. Roadmap

## Stage 0 — skeleton ✅
Build system (`make all|clean|help`, `tools/*.sh`), firmware/Android/backend skeletons that build,
docs, CLAUDE.md.

## Stage 1 — RP2040 ↔ phone link (current)
Done: protocol v1 spec, reference vectors, C codec + control state machine + link dispatcher with host tests,
Kotlin codec with tests, simulated motor driver, AOA host transport (`usb_aoa.c`), status LED (1 s blink = no
phone, solid = phone connected, fast blink = armed), Android `RobotLink` (handshake, state, 20 Hz DRIVE stream
only while ARMED) with JVM tests against a fake base, `RobotLinkService`, root charge window (`ChargeController`),
diagnostics screen with a local joystick. Next: bench test on the SM-J710F, power baseline.

1. Finalize [03](03-usb-link.md); `protocol/vectors.txt` with reference frames.
2. Firmware: TinyUSB host + AOA handshake, COBS/CRC framing, state machine and failsafes ([05](05-safety.md)),
   hoverboard driver, status LED, UART0 debug log, unit tests for the codec on the host PC.
3. Android: `RobotLinkService` (foreground, `connectedDevice`), `UsbAccessory` transport, codec + tests
   against the same vectors, link state machine, diagnostics screen with a local joystick.
4. Power baseline measurements (`PWR-015`).

**Acceptance:** robot drives from the phone screen joystick; telemetry is visible; pulling the USB cable
stops the motors within 300 ms; killing the app stops the motors within 300 ms; idle link generates no USB
traffic; the phone charges while connected.

## Stage 2 — remote driving
Done: backend (single-threaded; operators, robot tokens, signaling over WebSocket, one operator per robot,
TURN credentials, errors → file + Telegram), `.deb` with nginx site (pre-compressed static, abuse filtering,
security log, `aerolink guard` nftables bans), hardened systemd socket/service, landing + operator console (video, keyboard and
gamepad, 20 Hz DRIVE over a DataChannel, telemetry, round trip, direct/relay indicator), `make e2e` with a fake
robot in headless Chromium. Robot app side ([08](08-backend.md) "Robot app side"): signaling client with
reconnect, operator session state machine, WebRTC peer (camera, microphone, `ctl`/`drive` DataChannels, H.264
preferred), `SAFE-010`/`SAFE-012` on the phone, camera/wake lock only during a session, provisioning on the
diagnostics screen, simulated robot base for bench tests, `make bench-server`; JVM tests incl. the real backend
(`make e2e`). Bench-tested on a Pixel 7 incl. the TURN relay path ([09](09-deployment.md) "Bench test").
Next: deployment with coturn on a real server, energy measurements on the robot phone (final build), then the
RP2040 base.

Backend (operators, robots, tokens, signaling over WebSocket), coturn, WebRTC on the phone (video, audio,
DataChannel for DRIVE), operator console (video, keyboard/gamepad, telemetry, latency indicator).

## Stage 3 — field readiness
Done: flashlight headlight (`CTL-004`), two-way audio through the loudspeaker, sessions that survive a network
change (`SIG-007`: resume after a signaling reconnect, ICE restart, continual ICE gathering), video tuning
(H.264 preferred by the console, 1.5 Mbit/s cap, frame rate kept, rotation pinned to the mount; `PWR-013`,
`ROB-007`), app updates from the server (`ROB-008`: silent install as Device Owner, `PWR-014`; tested as a
self-update on the Pixel), firmware updates through the app (3b below: done except the hardware runs).
Next: 3a; Device Owner check on the robot phone once it is available.

**Compatibility:** until the first release is declared, database schemas, settings, protocols and file formats
are changed in place (no migrations, no compatibility code; `db.rs` `MIGRATIONS` is rewritten as one step).
From the first release on, every change needs a migration and must keep deployed robots and servers working.

### Stage 3a — fleet administration and error reporting ([08](08-backend.md) "Fleet administration")
Everything here can be built and tested now (PC + Pixel bench, `make e2e`).

1. Backend: `admin` role (CLI `aerolink operator role <email> admin|operator`), fleets and admins' fleet lists
   (`ADM-005`), schema with last known robot state, `robot_events`, `audit`; hub: admin views, `admin_end`,
   `admin_take`, robot/user actions with role and fleet checks (`ADM-004/005`, `ADM-012..014`).
2. Robot app: extended `status`, `event` queue with crash capture, location policy (`ADM-001..003`).
3. Console: admin tab — fleet table, robot page with events and actions, operators page (`ADM-010`, `ADM-012`,
   `ADM-013`), EN + RU; map with OpenStreetMap tiles (`ADM-011`).
4. Telegram for robot `error` events; docs 09 (admin provisioning).

**Acceptance:** an admin sees every robot of their fleets (and only those) with battery, versions, last position and errors, live; ends an
operator's session or takes the robot over, and the robot stops in between; robot errors reach Telegram; an
operator cannot use any admin message (tested); idle robot traffic does not grow beyond messages on change.

### Stage 3b — firmware update over the link ([03](03-usb-link.md) "Firmware update over the link")
Done: loader `aerolink-boot` (boot record, install, trial, rollback), application relinked at `0x10010000`,
FW_* receiver and REBOOT, `PROTO_VERSION` 2, HELLO_ACK with `fw_crc32`/`boot`; host tests with a NOR-like fake
flash incl. a power cut after every flash operation (`LINK-040..046`, `SAFE-007`); vectors and Kotlin codec;
firmware image in the APK, `FirmwareUpdater`, `SimulatedBase` support, diagnostics (`ROB-009`). Bench-tested on
the Pixel against the simulated base (40 KB sent, reboot, trial, confirm in < 1 s).
Open: the loader and a real AOA transfer on hardware — on a bare Pico via PC (flash `aerolink-rp2040.uf2`, then
write a staged image and boot record with `picotool`: normal, corrupted, crashing → rollback, unplugging during
the copy), and the FW_* path once the phone ↔ RP2040 wiring exists (with OTG and the wheels).

**Acceptance:** a newer APK brings new firmware to a robot with no PC; a power cut at any moment leaves a
working board; a firmware that never confirms is rolled back after 3 boots; motors stay at zero throughout.

## Stage 4 — autonomy (post-MVP)
Lane keeping, traffic-light detection as additional DRIVE `source`s under phone-side arbitration.
