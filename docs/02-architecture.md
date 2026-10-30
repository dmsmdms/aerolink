# 02. Architecture

```
 Operator browser                 Server (Debian)                     Robot
┌──────────────────┐   HTTPS/WS  ┌──────────────────┐  WS (signaling) ┌──────────────────────┐   USB (AOA)  ┌────────┐ UART/GPIO ┌────────────┐
│ frontend (Svelte)│◄───────────►│ nginx + backend  │◄───────────────►│ Android app (Kotlin) │◄────────────►│ RP2040 │◄─────────►│ hoverboard │
│ video, gamepad   │             │ auth, registry,  │                 │ link service, camera,│              │ fw (C) │           │ mainboard  │
└────────▲─────────┘             │ signaling        │                 │ WebRTC, arbitration  │              └────────┘           └────────────┘
         │                       └──────────────────┘                 └──────────▲───────────┘
         └──────────── WebRTC media + DataChannel (via coturn TURN when needed) ──┘
```

## Components

| Dir | Component | Technology |
|-----|-----------|------------|
| `rp2040/` | Firmware | C11, Pico SDK (`/opt/pico-sdk`, override with `PICO_SDK_PATH`), TinyUSB host, CMake + Ninja |
| `android/` | Robot app `com.aerolink.robot` | Kotlin, coroutines/Flow, minSdk 23 (Android 6.0), Compose only for the bench diagnostics screen |
| `backend/` | Server `aerolink` | Rust (edition 2024), tokio current-thread + axum, SQLite, `.deb` + systemd + nginx site ([08](08-backend.md), [09](09-deployment.md)) |
| `frontend/` | Landing + operator console | TypeScript + Svelte 5 + Vite, pre-compressed static files served by nginx |
| `protocol/` | USB protocol test vectors (stage 1) | JSON, consumed by both firmware and Android tests |

## Responsibilities

- **RP2040** is a dumb, safe actuator: it executes drive commands within hard limits, stops on its own when
  commands stop arriving, and reports telemetry. It knows nothing about operators or networks.
- **Phone** owns the robot state: operator session, command arbitration (operator vs future autopilot),
  media capture, network. It enforces its own link-loss stop on top of the firmware failsafe.
- **Backend** authenticates operators and robots, keeps the robot registry and relays signaling only.
  Media **and control** (DRIVE, ARM) go peer-to-peer between console and robot over WebRTC, through TURN
  (coturn) when NAT leaves no direct path — never through the backend ([08](08-backend.md)).
- **nginx** terminates TLS, serves the pre-compressed static files and filters abuse ([09](09-deployment.md)).

## Robot modes and operators

- `ARCH-001` One operator controls many robots; a robot is controlled by at most one operator at a time.
- `ARCH-002` Robot modes (phone-side, reported to the backend): `IDLE` (parked, DISARMED), `TELEOP` (operator
  drives), `AUTO` (autopilot drives — post-MVP), `NEEDS_HELP` (stopped, waiting for an operator).
- `ARCH-003` The backend keeps an attention queue: robots in `NEEDS_HELP` are pushed to their operator's console
  in arrival order; the operator takes one into `TELEOP`, then hands it back to `AUTO` or `IDLE`.
- `ARCH-004` Only the robot in the operator's focus streams full video; others send telemetry and a low-fps
  preview (or nothing), to save phone energy and cellular traffic.

## Motor driver

The hoverboard mainboards are reflashed with open firmware ([11](11-hoverboard.md); the first robot: Gen2.x
`TT-SD2.2`, RoboDurden Gen2.x firmware). The firmware talks to motors through a small driver interface
(`rp2040/src/motor.h`); `motor_sim.c` is a simulator used until the real driver exists (announced with the
`MOTOR_SIMULATED` capability). The real driver will speak the Gen2.x UART protocol (`REMOTE_UARTBUS` or
`REMOTE_UART`, Q1); Gen1 boards would use EFeru `hoverboard-firmware-hack-FOC`.

## Concurrency rules

- Firmware: single-core super-loop + IRQs, no RTOS, no dynamic allocation, hardware watchdog always on.
  Protocol codec and control logic have no SDK dependencies and are unit-tested on the host (`make test`).
- Android: coroutines/Flow only, no custom executors or RxJava. Blocking USB reads live in one coroutine on `Dispatchers.IO`.
- Backend: one process, one thread (tokio current-thread runtime), no `std::thread::spawn`, no timers or
  polling; blocking work stays tiny (SQLite queries, one Argon2 check per login).
