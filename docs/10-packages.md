# 10. Packages

Debian 13 "trixie" package names. Nothing is fetched by the build scripts except the Gradle/Maven and npm
dependencies of the Android app and the frontend (project-local, pinned in `android/app/build.gradle.kts`
and `frontend/package-lock.json`) and the Rust crates (`backend/Cargo.lock`).

## Build machine

| Component | Debian packages | Not packaged |
|-----------|-----------------|--------------|
| All | `make`, `git` | |
| Firmware (`make rp2040`, `make flash`) | `cmake`, `ninja-build`, `gcc-arm-none-eabi`, `libnewlib-arm-none-eabi`, `libstdc++-arm-none-eabi-newlib`, `picotool` (2.1.1 is enough, see CLAUDE.md), `python3` | Pico SDK 2.3 in `/opt/pico-sdk` with the `lib/tinyusb` submodule |
| Firmware host tests (`make test-rp2040`) | `gcc` (`build-essential`; ASan/UBSan are part of gcc) | |
| Protocol vectors (`make vectors`) | `python3` | |
| Android app (`make android`, `make test-android`) | `openjdk-21-jdk-headless` (or 17), `gradle` (≥ 8.8) | Android SDK in `/opt/android-sdk` (or `ANDROID_HOME`): `platforms;android-34`, `build-tools;34.0.0` |
| Install on a phone (`make install-apk`) | `adb` | |
| Frontend (`make frontend`, `make test-frontend`) | `nodejs` (≥ 20.19), `npm` | |
| Backend + `.deb` (`make backend`, `make test-backend`) | `cargo`, `rustc` (≥ 1.85, edition 2024), `dpkg` (`dpkg-deb`, always installed); optional `rust-clippy` | |
| End-to-end test (`make e2e`) | `chromium`; everything for frontend, backend and Android | Node ≥ 22 (trixie has 20: use a newer Node and put it first in `PATH`) |
| Bench server (`make bench-server`) | as for frontend + backend; `adb`, `coturn` for `BENCH_USB=1` | |
| App icons (one-off, from `docs/logo/`) | `python3-pil` | |

SQLite is compiled into the backend (`rusqlite` bundled), so no `libsqlite3-dev` is needed.

## Server (runtime)

Installed with the package: `sudo apt install ./aerolink.deb` pulls the dependencies.

| Package | | |
|---------|--|--|
| `nginx` (≥ 1.25.1) | required | TLS, static files, rate limits, bot filtering ([09](09-deployment.md)) |
| `libnginx-mod-http-brotli-static` | required | serves the pre-compressed `.br` files |
| `nftables` (≥ 1.0.8) | required | bans by `aerolink guard` (OPS-013) |
| `adduser` | required | creates the `aerolink` system user |
| `coturn` | recommended | TURN relay for robots behind cellular NAT |
| `bash-completion` | suggested | tab completion of `aerolink …` commands (the package ships the completion file) |
| `uacme` (C) or `lego` (Go) | for the certificate | Let's Encrypt without Python; `certbot` works too but pulls in Python |

The `aerolink` binary links only `libc`, `libm`, `libgcc_s` (no OpenSSL: TLS is nginx's job, the Telegram client
uses rustls with built-in root certificates). Nothing of Node, Rust, Python or Java runs on the server.

## Robot phone

Only the APK for its ABI (`build/aerolink-armeabi-v7a.apk` for the SM-J710F, `aerolink-arm64-v8a.apk` for the
Pixel 7; `make install-apk` picks the right one). Root (Magisk) is optional: without it the charge window is off (PWR-023).
