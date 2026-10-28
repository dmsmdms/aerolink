// Motor driver interface. The hoverboard mainboard is not chosen yet (docs/07 Q1);
// motor_sim.c implements it until a real driver exists. Exactly one driver is linked.

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
