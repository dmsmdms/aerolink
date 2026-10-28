// Compile-time safety limits (docs/05-safety.md). Runtime CONFIG can only lower them (SAFE-005).

#pragma once

// Speeds/accelerations are in permille of the motor driver's full scale.
#define CFG_MAX_SPEED       1000  // hard cap for CONFIG.max_speed
#define CFG_MAX_ACCEL       2000  // ‰/s, hard cap for CONFIG.accel
#define CFG_STOP_DECEL      3000  // ‰/s, deceleration for failsafe stops
#define CFG_MAX_CMD_TIMEOUT 250   // ms, hard cap for CONFIG.cmd_timeout_ms (SAFE-002)

// Defaults until the phone sends CONFIG: deliberately slow.
#define CFG_DEFAULT_MAX_SPEED        300
#define CFG_DEFAULT_ACCEL            1000
#define CFG_DEFAULT_CMD_TIMEOUT      250
#define CFG_DEFAULT_TELEMETRY_PERIOD 200
#define CFG_MIN_TELEMETRY_PERIOD     20

#define CFG_WATCHDOG_MS 100  // SAFE-004
