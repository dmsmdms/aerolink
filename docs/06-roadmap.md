# 06. Roadmap

## Stage 0 — skeleton ✅
Build system (`make all|clean|help`, `tools/*.sh`), firmware/Android/backend skeletons that build,
docs, CLAUDE.md.

## Stage 1 — RP2040 ↔ phone link (current)
Done: protocol v1 spec, reference vectors, C codec + control state machine + link dispatcher with host tests,
Kotlin codec with tests, simulated motor driver. Next: AOA host transport, Android `RobotLinkService`.

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
Backend (operators, robots, tokens, signaling over WebSocket), coturn, WebRTC on the phone (video, audio,
DataChannel for DRIVE), operator console (video, keyboard/gamepad, telemetry, latency indicator).

## Stage 3 — field readiness
Flashlight headlight, two-way audio through the speaker, reconnect on cellular handover, adaptive bitrate,
error reporting, APK update path, firmware update path.

## Stage 4 — autonomy (post-MVP)
Lane keeping, traffic-light detection as additional DRIVE `source`s under phone-side arbitration.
