# 05. Safety

The firmware is the last line of defence; it must be safe even if the phone app hangs or crashes.

## Firmware

- `SAFE-001` States: `DISARMED` (motors commanded to zero, ignores DRIVE) → `ARMED` (accepts DRIVE) → `FAULT`.
  Boot state is `DISARMED`.
- `SAFE-002` In `ARMED`, if no valid DRIVE/PING arrives within the command timeout (default 250 ms, and never
  longer than the DRIVE `ttl_ms`), the firmware ramps speed to zero with the configured deceleration and
  returns to `DISARMED`.
- `SAFE-003` USB detach or accessory reset → immediate transition to `DISARMED` with controlled stop.
- `SAFE-004` Hardware watchdog (100 ms) is always enabled; a reset boots into `DISARMED`.
- `SAFE-005` Speed and acceleration limits are compile-time maxima; CONFIG can only lower them.
- `SAFE-006` Hoverboard link loss or hoverboard fault → `FAULT`, motors zero, EVENT to the phone.
  `FAULT` is cleared only by an explicit DISARM → ARM.
- `SAFE-007` Firmware updates (`LINK-040..046`) start only in DISARMED; from FW_BEGIN until FW_END, HELLO or USB
  detach the firmware (and the phone) refuse ARM, so the motors stay at zero while the flash is written. REBOOT is accepted only in
  DISARMED. The loader never drives motors, and a new image always boots into DISARMED (`SAFE-004`). The phone
  starts an update only without an operator session (`ROB-009`).

## Phone

- `SAFE-010` Operator link silence > 500 ms → phone sends zero DRIVE then DISARM. Right after ARM the first DRIVE
  may take up to 1.5 s (the console starts streaming when it sees ARMED); the phone keeps the robot at zero
  meanwhile. Session end or peer connection failure stop the robot at once. Losing only the signaling connection
  does not: DRIVE comes peer-to-peer, and the session is resumed after the reconnect (`SIG-007`); if DRIVE stops
  meanwhile, the 500 ms rule applies.
- `SAFE-011` Every DRIVE carries `source` (operator, autopilot, local joystick). Arbitration happens on the
  phone; operator input always overrides autopilot.
- `SAFE-012` The phone does not forward stale commands: operator messages carry a timestamp/sequence and
  older-than-latest messages are dropped.
