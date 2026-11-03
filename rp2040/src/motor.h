// Motor driver interface. Exactly one driver is linked (MOTOR_DRIVER): motor_hoverboard.c drives two Gen2.x
// hoverboard boards (docs/11-hoverboard.md), motor_sim.c simulates wheels for bench tests without them.

#pragma once

#include <stdint.h>

typedef struct {
    uint16_t battery_mv;  // 0 = unknown
    int16_t speed_l;      // ‰ of full scale
    int16_t speed_r;
    int16_t temp_dc;      // 0.1 °C, TEMP_UNKNOWN if not available
    uint16_t faults;      // FAULT_* bits
} motor_feedback_t;

void motor_init(void);

// CAP_* bits describing what this driver provides.
uint32_t motor_caps(void);

// Wheel commands in ‰ of full scale, called every loop iteration.
void motor_set(int16_t left, int16_t right, uint32_t now_ms);

// Latest feedback; non-blocking.
void motor_feedback(motor_feedback_t *fb);
