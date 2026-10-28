# AeroLink

Teleoperation system for sidewalk delivery robots: an Android phone (camera, mic, speaker, flashlight, SIM) is the robot brain, an RP2040 bridges the phone (USB) to a hoverboard drive (GPIO/UART), operators drive via a web console. Specs live in `docs/` (start with `docs/README.md`); treat them as the source of truth and reference requirement IDs (e.g. `LINK-010`, `SAFE-002`) in commits and code comments where it helps.

## Language
- Code, comments, commit messages, docs: **English**.
- Chat replies to the user: **Russian**.

## Repo layout
- `rp2040/` — firmware, C11 + Pico SDK, CMake/Ninja. SDK at `/opt/pico-sdk` (override `PICO_SDK_PATH`), board via `PICO_BOARD` (default `pico`, the robot uses a Pico clone with USB-C). `proto`, `control`, `link` must stay SDK-free (host-tested); hardware lives in `main.c`, `motor_*.c`, USB transport. Motor driver chosen by `AEROLINK_MOTOR_DRIVER` (only `sim` so far).
- `protocol/vectors.txt` — generated reference frames shared by firmware and Android tests.
- `android/` — Kotlin, package `com.aerolink.robot`, minSdk 23 (Android 6.0). Compose only for the bench diagnostics screen.
- `backend/` — Rust (edition 2024) service `aerolink`, packaged as `.deb` + systemd (`backend/package/`).
- `frontend/` — operator console, TypeScript + Svelte + Vite (stage 2, not created yet).
- `tools/` — per-component build/clean/flash/install scripts used by the root `Makefile`. Outputs go to `build/` (`aerolink-rp2040.uf2`, `aerolink.apk`, `aerolink.deb`).

## Build commands
- `make` / `make all` — build everything; `make clean`; `make help` lists all targets and variables.
- Single components: `make rp2040`, `make android`, `make backend`; `make flash` (picotool), `make install-apk` (adb).
- Tests: `make test` (= `test-rp2040`: firmware SDK-free modules built with host gcc + ASan/UBSan; `test-android`: JVM unit tests).
- Protocol changes: edit `tools/gen_vectors.py` (reference encoder) first, run `make vectors`, then update `rp2040/src/{proto,messages}` and `android/.../link/` until both test suites pass. Bump `PROTO_VERSION` on incompatible changes.

## Toolchain quirks
- Pico SDK 2.3 requires picotool ≥ 2.3.0, Debian has 2.1.1: `rp2040/CMakeLists.txt` sets `PICO_NO_PICOTOOL` and builds the UF2 with the system picotool. Do not let the SDK fetch/build picotool.
- TinyUSB comes from the SDK submodule `/opt/pico-sdk/lib/tinyusb` (must be initialized).
- AGP must be ≥ 8.2.1 (8.2.0 breaks with JDK 21 in `JdkImageTransform`). Build uses the system `gradle` (8.8), no wrapper.
- Never install packages yourself; tell the user which Debian package is missing.

## Hard rules
- **Safety first** (`docs/05-safety.md`): the firmware stops the motors on its own when commands stop (ARMED timeout, USB detach, watchdog). Never add a code path that drives motors without a fresh command. Limits are compile-time maxima; runtime config can only lower them.
- **Energy is a feature** (`docs/04-power.md`): no polling, no periodic USB traffic when DISARMED, nothing running while idle except the signaling socket; camera/encoder/wake locks only during an operator session.
- **USB link** is Android Open Accessory: RP2040 = USB host, phone = accessory and gets charged (`docs/03-usb-link.md`). Firmware and Android codecs must pass the shared vectors in `protocol/`.
- **Concurrency**: firmware — super-loop + IRQs, no RTOS, no malloc after init. Android — coroutines/Flow only, no custom executors/RxJava. Backend — one tokio runtime, no `std::thread::spawn`.
- **Secrets** never in git: Android signing goes in `~/.gradle/gradle.properties` (`AEROLINK_STORE_FILE`, `AEROLINK_STORE_PASSWORD`, `AEROLINK_KEY_ALIAS`, `AEROLINK_KEY_PASSWORD`); without them release is signed with the debug key.
- UI strings are localized (EN + RU), no hard-coded user-facing text.
