#include "control.h"

#include "config.h"
#include "messages.h"

#define SCALE 1000  // out_l/out_r fixed point: 1/1000 ‰

static bool reached(uint32_t now, uint32_t deadline) {
    return (int32_t)(now - deadline) >= 0;
}

static int32_t clamp(int32_t v, int32_t lo, int32_t hi) {
    return v < lo ? lo : (v > hi ? hi : v);
}

static void emit(control_t *c, uint8_t code, uint16_t arg) {
    if (c->on_event) {
        c->on_event(code, arg, c->event_ctx);
    }
}

static void set_state(control_t *c, uint8_t state) {
    if (c->state != state) {
        c->state = state;
        emit(c, EVENT_STATE, state);
    }
}

static void clear_target(control_t *c) {
    c->target_l = 0;
    c->target_r = 0;
}

static int32_t ramp(int32_t out, int32_t target, int32_t step) {
    if (out < target) {
        return out + step > target ? target : out + step;
    }
    return out - step < target ? target : out - step;
}

static int32_t step_wheel(int32_t out, int32_t target, int32_t accel_step, int32_t stop_step) {
    bool braking = (out > 0 && target < out) || (out < 0 && target > out);
    if (braking && ((out > 0 && target < 0) || (out < 0 && target > 0))) {
        return ramp(out, 0, stop_step);  // reversing: stop first, never cross zero in one tick
    }
    return ramp(out, target, braking ? stop_step : accel_step);
}

void control_init(control_t *c, uint32_t now_ms, control_event_fn on_event, void *ctx) {
    *c = (control_t){
        .state = STATE_DISARMED,
        .max_speed = CFG_DEFAULT_MAX_SPEED,
        .accel = CFG_DEFAULT_ACCEL,
        .cmd_timeout_ms = CFG_DEFAULT_CMD_TIMEOUT,
        .telemetry_period_ms = CFG_DEFAULT_TELEMETRY_PERIOD,
        .last_tick = now_ms,
        .on_event = on_event,
        .event_ctx = ctx,
    };
}

uint8_t control_arm(control_t *c, uint32_t now_ms) {
    if (c->faults & FAULT_CRITICAL) {
        return NACK_FAULT_ACTIVE;
    }
    if (c->state != STATE_DISARMED) {
        return c->state == STATE_ARMED ? 0 : NACK_BAD_STATE;
    }
    clear_target(c);
    c->link_deadline = now_ms + c->cmd_timeout_ms;
    c->target_deadline = now_ms;
    set_state(c, STATE_ARMED);
    return 0;
}

void control_disarm(control_t *c) {
    clear_target(c);
    if (c->state == STATE_ARMED) {
        set_state(c, STATE_STOPPING);
    } else if (c->state == STATE_FAULT) {
        set_state(c, STATE_DISARMED);  // outputs are already forced to zero in FAULT
    }
}

void control_drive(control_t *c, int16_t speed, int16_t steer, uint16_t ttl_ms, uint32_t now_ms) {
    if (c->state != STATE_ARMED) {
        return;  // LINK-032: silently dropped
    }
    int32_t s = clamp(speed, -1000, 1000);
    int32_t t = clamp(steer, -1000, 1000);
    int32_t l = clamp(s + t, -1000, 1000);
    int32_t r = clamp(s - t, -1000, 1000);
    c->target_l = (int16_t)(l * c->max_speed / 1000);
    c->target_r = (int16_t)(r * c->max_speed / 1000);

    uint16_t validity = (ttl_ms == 0 || ttl_ms > c->cmd_timeout_ms) ? c->cmd_timeout_ms : ttl_ms;
    c->target_deadline = now_ms + validity;
    c->link_deadline = now_ms + c->cmd_timeout_ms;
}

void control_ping(control_t *c, uint32_t now_ms) {
    if (c->state == STATE_ARMED) {
        c->link_deadline = now_ms + c->cmd_timeout_ms;
    }
}

void control_config(control_t *c, uint16_t max_speed, uint16_t accel, uint16_t telemetry_period_ms,
                    uint16_t cmd_timeout_ms) {
    c->max_speed = (uint16_t)clamp(max_speed, 0, CFG_MAX_SPEED);
    c->accel = (uint16_t)clamp(accel, 1, CFG_MAX_ACCEL);
    c->telemetry_period_ms =
        telemetry_period_ms == 0 ? 0 : (uint16_t)clamp(telemetry_period_ms, CFG_MIN_TELEMETRY_PERIOD, 60000);
    c->cmd_timeout_ms = (uint16_t)clamp(cmd_timeout_ms, 1, CFG_MAX_CMD_TIMEOUT);
    // A lower max_speed applies to the running command immediately.
    c->target_l = (int16_t)clamp(c->target_l, -c->max_speed, c->max_speed);
    c->target_r = (int16_t)clamp(c->target_r, -c->max_speed, c->max_speed);
}

void control_link_lost(control_t *c) {
    clear_target(c);
    if (c->state == STATE_ARMED) {
        set_state(c, STATE_STOPPING);
    }
}

void control_set_faults(control_t *c, uint16_t faults) {
    if (faults == c->faults) {
        return;
    }
    c->faults = faults;
    emit(c, EVENT_FAULTS, faults);
    if (faults & FAULT_CRITICAL) {
        clear_target(c);
        c->out_l = 0;
        c->out_r = 0;
        set_state(c, STATE_FAULT);
    }
}

void control_tick(control_t *c, uint32_t now_ms) {
    uint32_t dt = now_ms - c->last_tick;
    c->last_tick = now_ms;

    if (c->state == STATE_ARMED) {
        if (reached(now_ms, c->link_deadline)) {
            clear_target(c);
            emit(c, EVENT_CMD_TIMEOUT, 0);
            set_state(c, STATE_STOPPING);
        } else if (reached(now_ms, c->target_deadline)) {
            clear_target(c);  // stale DRIVE: stop but stay armed
        }
    }

    if (c->state == STATE_FAULT || c->state == STATE_DISARMED) {
        c->out_l = 0;
        c->out_r = 0;
        return;
    }

    // Accelerate with the configured rate; any move towards zero (braking, reversing) uses the
    // stronger stop rate.
    if (dt > 1000) {
        dt = 1000;
    }
    int32_t accel_step = (int32_t)c->accel * (int32_t)dt;
    int32_t stop_step = (int32_t)CFG_STOP_DECEL * (int32_t)dt;
    c->out_l = step_wheel(c->out_l, (int32_t)c->target_l * SCALE, accel_step, stop_step);
    c->out_r = step_wheel(c->out_r, (int32_t)c->target_r * SCALE, accel_step, stop_step);

    if (c->state == STATE_STOPPING && c->out_l == 0 && c->out_r == 0) {
        set_state(c, STATE_DISARMED);
    }
}

int16_t control_out_left(const control_t *c) { return (int16_t)(c->out_l / SCALE); }
int16_t control_out_right(const control_t *c) { return (int16_t)(c->out_r / SCALE); }

bool control_active(const control_t *c) {
    return c->state == STATE_ARMED || c->state == STATE_STOPPING;
}
