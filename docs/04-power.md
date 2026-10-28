# 04. Power

## Topology

```
hoverboard battery (36 V nominal, 10S Li-ion)
   ├── hoverboard mainboard (motors)
   └── DC-DC buck 36 V → 5 V, ≥ 2 A ──┬── USB VBUS to the phone (charging)
                                     └── RP2040 board (VBUS/VSYS)
```

- `PWR-001` The phone is charged from the robot battery through the same USB cable that carries data (AOA).
- `PWR-002` The RP2040 board is a Pico-compatible board with a USB-C receptacle (`PICO_BOARD=pico`). As USB host
  it must source VBUS: the 5 V rail is fed into the board's `VBUS` pin, which must be wired straight to the
  connector (no diode — check the clone's schematic). Fuse the rail; the board traces carry the phone current.
- `PWR-003` Phone side: Samsung SM-J710F has **micro-USB**, so the cable is USB-C (board) → micro-B (phone).
  With data lines in use the phone negotiates USB SDP and draws ≈ 500 mA. That roughly matches its consumption
  while streaming, so the battery is roughly neutral when driving and grows only when parked.
  Future USB-C phones: the cable/receptacle must advertise current via Rp on CC (10 kΩ → 3 A).
- `PWR-004` Robot battery voltage is reported in TELEMETRY when the motor driver provides it; the phone raises a
  low-battery alarm to the operator.

## Phone battery window

- `PWR-020` The phone keeps its battery between **45 % and 60 %**: charging is disabled at ≥ 60 % and re-enabled
  at ≤ 45 %. Rationale: low wear, while leaving ~3 h of standalone operation to keep streaming and call the
  operator if the robot battery dies.
- `PWR-021` Charging is disabled whenever battery temperature is > 40 °C (re-enabled < 37 °C), regardless of level.
  A sealed body in the sun plus streaming plus charging is the main battery killer.
- `PWR-022` Charge control requires **root** (Magisk). The app writes the vendor sysfs charge switch
  (Samsung: `/sys/class/power_supply/battery/batt_slate_mode`, fallback `store_mode`; probed at runtime,
  first writable node wins). The USB link stays up while charging is suspended.
- `PWR-023` Without root (or without a known sysfs node) all root-only features are **disabled**, not emulated:
  the phone simply charges whenever powered. The app reports `charge_control: unavailable` to the server.
- `PWR-024` Battery level/temperature come from the sticky `ACTION_BATTERY_CHANGED` broadcast (event-driven,
  no polling).

## Phone energy rules

The phone is a dedicated device; it runs one app, with the screen off.

- `PWR-010` Work runs in a foreground service; no Activity is required at runtime. Screen stays off.
- `PWR-011` No polling anywhere: USB reads block in a coroutine; network uses push (WebSocket) not polling.
- `PWR-012` Idle robot (no operator session): camera, encoder, microphone, WebRTC and wake locks are released;
  USB link is DISARMED (zero traffic, `LINK-020`); only the signaling WebSocket stays open with a keepalive
  of 2–4 minutes (frequent pings keep the cellular radio in its high-power state).
- `PWR-013` Active session: hardware H.264 encoding (MediaCodec), adaptive resolution/fps, partial wake lock
  held only for the duration of the session. Robots that are not in the operator's focus stream a low-fps
  preview or nothing ([02](02-architecture.md)).
- `PWR-014` Provisioning: the app is **Device Owner** (`adb shell dpm set-device-owner`) for auto-start, no lock
  screen and silent permission grants; battery-optimisation exemption is requested (Doze exists since Android 6).
- `PWR-015` Every stage measures power: `adb shell dumpsys batterystats` + Battery Historian, idle and active,
  compared with the previous baseline.
