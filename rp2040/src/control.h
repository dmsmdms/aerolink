// Drive state machine and failsafes (docs/05-safety.md). Pure logic: time is passed in,
// outputs are read by the caller, so it is unit-tested on the host.

#pragma once

#include <stdbool.h>
#include <stdint.h>

typedef void (*control_event_fn)(uint8_t code, uint16_t arg, void *ctx);

typedef struct {
    uint8_t state;    // STATE_*
    uint16_t faults;  // FAULT_* bits

    // Runtime limits, always <= the CFG_MAX_* caps.
    uint16_t max_speed;
    uint16_t accel;
    uint16_t cmd_timeout_ms;
    uint16_t telemetry_period_ms;

    int16_t target_l;          // ‰, wheel targets after mixing and max_speed scaling
    int16_t target_r;
    uint32_t target_deadline;  // DRIVE validity (LINK-032)
    uint32_t link_deadline;    // last DRIVE/PING + cmd_timeout (SAFE-002)

    int32_t out_l;  // ramped wheel outputs, in 1/1000 ‰
    int32_t out_r;
    uint32_t last_tick;

    control_event_fn on_event;
    void *event_ctx;
} control_t;

void control_init(control_t *c, uint32_t now_ms, control_event_fn on_event, void *ctx);

// Returns 0 on success or a NACK_* reason.
uint8_t control_arm(control_t *c, uint32_t now_ms);
void control_disarm(control_t *c);

// speed/steer in ‰ (clamped to ±1000); ttl_ms 0 means "cmd_timeout".
void control_drive(control_t *c, int16_t speed, int16_t steer, uint16_t ttl_ms, uint32_t now_ms);
void control_ping(control_t *c, uint32_t now_ms);
void control_config(control_t *c, uint16_t max_speed, uint16_t accel, uint16_t telemetry_period_ms,
                    uint16_t cmd_timeout_ms);

// Phone gone (USB detach / accessory closed): controlled stop, then DISARMED (SAFE-003).
void control_link_lost(control_t *c);

// Faults reported by the motor driver; critical ones force FAULT and zero output (SAFE-006).
void control_set_faults(control_t *c, uint16_t faults);

// Advances timeouts and ramps; call every loop iteration.
void control_tick(control_t *c, uint32_t now_ms);

// Current wheel commands for the motor driver, ‰.
int16_t control_out_left(const control_t *c);
int16_t control_out_right(const control_t *c);

// True while motors may be moving (telemetry is periodic only then, LINK-020/021).
bool control_active(const control_t *c);
