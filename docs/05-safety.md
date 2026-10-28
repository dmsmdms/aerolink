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

## Phone

- `SAFE-010` Operator link silence > 500 ms → phone sends zero DRIVE then DISARM.
- `SAFE-011` Every DRIVE carries `source` (operator, autopilot, local joystick). Arbitration happens on the
  phone; operator input always overrides autopilot.
- `SAFE-012` The phone does not forward stale commands: operator messages carry a timestamp/sequence and
  older-than-latest messages are dropped.
