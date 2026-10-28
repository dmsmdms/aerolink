// Simulated motor driver: wheels follow the command with a first-order lag. No hardware access,
// so bench tests with the phone work without a hoverboard. Announced via CAP_MOTOR_SIMULATED.

#include <stdbool.h>

#include "messages.h"
#include "motor.h"

#define SIM_TAU_MS 200

static int32_t speed_l;
static int32_t speed_r;
static uint32_t last_ms;
static bool started;

void motor_init(void) {
    speed_l = 0;
    speed_r = 0;
    started = false;
}

uint32_t motor_caps(void) { return CAP_MOTOR_SIMULATED | CAP_MOTOR_FEEDBACK; }

static int32_t follow(int32_t speed, int32_t target, uint32_t dt) {
    if (dt >= SIM_TAU_MS) {
        return target;
    }
    return speed + (target - speed) * (int32_t)dt / SIM_TAU_MS;
}

void motor_set(int16_t left, int16_t right, uint32_t now_ms) {
    uint32_t dt = started ? now_ms - last_ms : 0;
    started = true;
    last_ms = now_ms;
    speed_l = follow(speed_l, left, dt);
    speed_r = follow(speed_r, right, dt);
}

void motor_feedback(motor_feedback_t *fb) {
    fb->battery_mv = 0;
    fb->speed_l = (int16_t)speed_l;
    fb->speed_r = (int16_t)speed_r;
    fb->temp_dc = TEMP_UNKNOWN;
    fb->faults = 0;
}
