# AeroLink

Teleoperation system for sidewalk delivery robots: an Android phone (camera, mic, speaker, flashlight, SIM) is the robot brain, an RP2040 bridges the phone (USB) to a hoverboard drive (GPIO/UART), operators drive via a web console. Specs live in `docs/` (start with `docs/README.md`); treat them as the source of truth and reference requirement IDs (e.g. `LINK-010`, `SAFE-002`) in commits and code comments where it helps.

## Language
- Code, comments, commit messages, docs: **English**.
- Chat replies to the user: **Russian**.

## Repo layout
- `rp2040/` — firmware, C11 + Pico SDK, CMake/Ninja. SDK at `/opt/pico-sdk` (override `PICO_SDK_PATH`), board via `PICO_BOARD` (default `pico`, the robot uses a Pico clone with USB-C). `proto`, `control`, `link`, `crc32`, `fwstore`, `fwupdate` must stay SDK-free (host-tested); hardware lives in `main.c`, `motor_*.c`, `flash_rp2040.c`, USB transport. Two images: `boot/loader.c` (`aerolink-boot`, start of flash, PC-flashed only) and the application linked into the active slot (`ld/`); the phone updates the application over the link (`LINK-040..046`). Motor driver chosen by `AEROLINK_MOTOR_DRIVER` (only `sim` so far).
- `protocol/vectors.txt` — generated reference frames shared by firmware and Android tests.
- `android/` — Kotlin, package `com.aerolink.robot`, minSdk 23 (Android 6.0; robot phone SM-J710F, bench tests also on a Pixel 7). `link/` codec + `RobotLink` (pure, JVM-tested with a fake base) + `SimulatedBase` (software RP2040 for bench/tests), `signal/` signaling WebSocket client (OkHttp), `teleop/` operator session state machine + `DriveGate` (SAFE-010/012; pure, JVM-tested with a fake peer), `rtc/` WebRTC peer (`io.github.webrtc-sdk:android`), `service/` foreground `RobotLinkService` + settings + Device Owner (`PWR-014`), `update/` app updates from the server (`ROB-008`) and robot base firmware updates (`ROB-009`), `power/` root charge window, `ui/` Compose diagnostics screen only.
- `backend/` — Rust (edition 2024) service `aerolink`: single-threaded tokio + axum + SQLite; `hub.rs` is the pure signaling state machine (unit-tested), `server.rs` HTTP/WS glue (end-to-end tested), `guard.rs` the `aerolink guard` firewall banner (nginx security log via syslog datagrams → nftables sets; no fail2ban/Python on the server). `backend/package/` is the `.deb` tree: systemd sockets/services, `/etc/aerolink/aerolink.env`, nginx site + snippets, logrotate.
- `frontend/` — landing (`/`) + operator console (`/console/`), TypeScript + Svelte 5 + Vite, builds to `build/www` with `.br`/`.gz` next to every file (nginx never compresses on the fly).
- `tools/` — per-component build/clean/flash/install scripts used by the root `Makefile`. Outputs go to `build/` (`aerolink-rp2040.uf2` loader + app for PC flashing, `aerolink-rp2040.bin` the app the APK carries for phone-driven updates, `aerolink-armeabi-v7a.apk` for the SM-J710F / `aerolink-arm64-v8a.apk` for the Pixel 7 — one APK per ABI because WebRTC is ~80 % of the app, plus `aerolink-release.json` for robot updates, `aerolink.deb`).

## Build commands
- `make` / `make all` — build everything; `make clean`; `make help` lists all targets and variables.
- Single components: `make rp2040`, `make android` (`ANDROID_VARIANT=debug` for bench: plain http + simulated base), `make backend`; `make flash` (picotool), `make install-apk` (adb). `make bench-server`: backend on the LAN + console on this PC for a phone running the debug app (`BENCH_USB=1`: over the USB cable via `adb reverse` + local coturn).
- `make frontend` (→ `build/www`), `make backend` builds the frontend first and ships it in the `.deb`.
- Tests: `make test` (= `test-rp2040`: firmware SDK-free modules built with host gcc + ASan/UBSan; `test-android`: JVM unit tests; `test-backend`: cargo tests incl. HTTP/WebSocket end-to-end; `test-frontend`: vitest + i18n parity). `make e2e`: backend + console + fake robot (WebRTC) in headless Chromium, then the Android signaling/session code against the same backend (`BackendInteropTest`).
- Protocol changes: edit `tools/gen_vectors.py` (reference encoder) first, run `make vectors`, then update `rp2040/src/{proto,messages}` and `android/.../link/` until both test suites pass. Bump `PROTO_VERSION` on incompatible changes.

## Toolchain quirks
- Pico SDK 2.3 requires picotool ≥ 2.3.0, Debian has 2.1.1: `rp2040/CMakeLists.txt` sets `PICO_NO_PICOTOOL` and builds the UF2 with the system picotool. Do not let the SDK fetch/build picotool.
- TinyUSB comes from the SDK submodule `/opt/pico-sdk/lib/tinyusb` (must be initialized).
- AGP must be ≥ 8.2.1 (8.2.0 breaks with JDK 21 in `JdkImageTransform`). Build uses the system `gradle` (8.8), no wrapper.
- Frontend deps are project-local (`npm ci` in `frontend/`); vitest/Vite need Node ≥ 20.19, `make e2e` Node ≥ 22. Package list for build machine and server: `docs/10-packages.md`.
- Never install packages yourself; tell the user which Debian package is missing.

## Hard rules
- **Safety first** (`docs/05-safety.md`): the firmware stops the motors on its own when commands stop (ARMED timeout, USB detach, watchdog). Never add a code path that drives motors without a fresh command. Limits are compile-time maxima; runtime config can only lower them.
- **Energy is a feature** (`docs/04-power.md`): no polling, no periodic USB traffic when DISARMED, nothing running while idle except the signaling socket; camera/encoder/wake locks only during an operator session.
- **USB link** is Android Open Accessory: RP2040 = USB host, phone = accessory and gets charged (`docs/03-usb-link.md`). Firmware and Android codecs must pass the shared vectors in `protocol/`.
- **Concurrency**: firmware — super-loop + IRQs, no RTOS, no malloc after init. Android — coroutines/Flow only, no custom executors/RxJava. Backend — one process, one thread (tokio current-thread), no `std::thread::spawn`, no timers/polling; std mutexes never held across `.await`.
- **Server split**: nginx does TLS, static files, caching, rate limits and bot filtering; the backend only does `/api/` + `/ws/` signaling. Operator ↔ robot media and control are peer-to-peer WebRTC, never proxied by the backend. Backend ERRORs go to the error log file and Telegram.
- **Secrets** never in git: Android signing goes in `~/.gradle/gradle.properties` (`AEROLINK_STORE_FILE`, `AEROLINK_STORE_PASSWORD`, `AEROLINK_KEY_ALIAS`, `AEROLINK_KEY_PASSWORD`); without them release is signed with the debug key.
- UI strings are localized (EN + RU), no hard-coded user-facing text.
