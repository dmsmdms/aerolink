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
  it must source VBUS. The bench clone has a **diode between the connector VBUS and the `VBUS` pin**, so 5 V fed
  into the pin never reaches the phone. Therefore the phone is powered **outside the board**: the fused 5 V rail
  goes straight to the VBUS of the phone-side connector/cable, only D+/D−/GND come from the board (USB-C or the
  D+/D− test pads). This also keeps the phone's charge current off the board's thin traces. Bench option: an OTG
  adapter/cable with a separate power input. Bridging the diode works too, but then never plug the board into a
  PC while the external 5 V is on (back-feed into the PC port).
- `PWR-003` Phone side: Samsung SM-J710F has **micro-USB**, so the cable is USB-C (board) → micro-B (phone).
  With data lines in use the phone negotiates USB SDP and draws ≈ 500 mA. That roughly matches its consumption
  while streaming, so the battery is roughly neutral when driving and grows only when parked.
  Future USB-C phones: the cable/receptacle must advertise current via Rp on CC (10 kΩ → 3 A).
- `PWR-005` USB-C phones (bench phone: Google Pixel 7) must **not** be connected with a plain C-to-C cable: the
  Pico clone's receptacle has Rd (sink) pull-downs on CC, so a dual-role phone takes the host role and powers the
  board, and AOA never starts. Bench cabling: board USB-C → OTG adapter (C plug → A receptacle) → standard
  USB-A → USB-C cable (its 56 kΩ Rp makes the phone a sink/device). The same adapter + A → micro-B cable works for
  the SM-J710F. On the robot the board side gets a receptacle/cable with Rp instead.
- `PWR-004` Robot battery voltage is reported in TELEMETRY when the motor driver provides it; the phone raises a
  low-battery alarm to the operator.

## Phone battery window

- `PWR-020` The phone keeps its battery between **45 % and 60 %**: charging is disabled at ≥ 60 % and re-enabled
  at ≤ 45 %. Rationale: low wear, while leaving ~3 h of standalone operation to keep streaming and call the
  operator if the robot battery dies.
- `PWR-021` Charging is disabled whenever battery temperature is > 40 °C (re-enabled < 37 °C), regardless of level.
  A sealed body in the sun plus streaming plus charging is the main battery killer.
- `PWR-022` Charge control requires **root** (Magisk). The app writes a sysfs charge switch under
  `/sys/class/power_supply/battery/`, probed at runtime in one `su` call, first writable node wins:
  `batt_slate_mode` (Samsung, 1 = suspend), `charging_enabled` (0 = suspend), `input_suspend` (1 = suspend).
  Samsung `store_mode` is not used: it only enforces the vendor's own window. The USB link stays up while
  charging is suspended. When the service stops, charging is re-enabled.
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
- `PWR-013` Active session: hardware H.264 encoding (MediaCodec; the console asks for H.264 first, VP8 is the
  fallback), resolution adapts and the frame rate is kept, video capped at 1.5 Mbit/s (≈ 0.7 GB per hour of
  driving) starting at 0.8 Mbit/s, partial wake lock held only for the duration of the session. Robots that are not in the operator's focus stream a low-fps
  preview or nothing ([02](02-architecture.md)).
- `PWR-014` Provisioning: the app is **Device Owner** (`adb shell dpm set-device-owner`, [09](09-deployment.md)
  `OPS-030`) for silent app updates (`ROB-008`), no lock screen and silent permission grants (camera, microphone,
  notifications); battery-optimisation exemption is requested (Doze exists since Android 6).
- `PWR-015` Every stage measures power: `adb shell dumpsys batterystats` + Battery Historian, idle and active,
  compared with the previous baseline.
