// Hoverboard motor driver: two Gen2.x boards running REMOTE_UARTBUS firmware, each on its own RP2040 UART
// (docs/11-hoverboard.md "Driving it from the RP2040"). Commands go out every HOVER_TX_PERIOD_MS with the
// latest control output; when the RP2040 stops sending (hang, watchdog reset, power loss) each board stops
// on its own after 0.5 s (HOV-001). No IRQs: the 32-byte RX FIFO holds two replies and is drained every loop.

#include <stdio.h>

#include "hardware/gpio.h"
#include "hardware/uart.h"
#include "pico/time.h"

#include "hoverbus.h"
#include "messages.h"
#include "motor.h"

// Left board: SLAVE_ID 0 on UART0, right board: SLAVE_ID 1 on UART1. The right motor is mounted mirrored.
#define LEFT_UART     uart0
#define LEFT_TX_PIN   0
#define LEFT_RX_PIN   1
#define LEFT_SLAVE    0
#define LEFT_INVERT   false
#define RIGHT_UART    uart1
#define RIGHT_TX_PIN  4
#define RIGHT_RX_PIN  5
#define RIGHT_SLAVE   1
#define RIGHT_INVERT  true

#define LOW_BATTERY_MV 32000  // 10S at 3.2 V/cell

#define UART_DR_ERRORS 0xF00u  // overrun, break, parity, framing

typedef struct {
    uart_inst_t *uart;
    const char *name;
    hover_side_t side;
    bool was_alive;
} board_t;

static board_t left = {.uart = LEFT_UART, .name = "left"};
static board_t right = {.uart = RIGHT_UART, .name = "right"};

static void board_init(board_t *b, uint tx, uint rx, uint8_t slave, bool invert, uint32_t now) {
    uart_init(b->uart, HOVER_BAUD);
    gpio_set_function(tx, GPIO_FUNC_UART);
    gpio_set_function(rx, GPIO_FUNC_UART);
    gpio_pull_up(rx);  // idle level while the board is unpowered or unplugged
    hover_side_init(&b->side, slave, invert, now);
    b->was_alive = true;
}

void motor_init(void) {
    uint32_t now = to_ms_since_boot(get_absolute_time());
    board_init(&left, LEFT_TX_PIN, LEFT_RX_PIN, LEFT_SLAVE, LEFT_INVERT, now);
    board_init(&right, RIGHT_TX_PIN, RIGHT_RX_PIN, RIGHT_SLAVE, RIGHT_INVERT, now);
}

uint32_t motor_caps(void) { return CAP_MOTOR_FEEDBACK | CAP_BATTERY_VOLTAGE; }

static void board_tick(board_t *b, int16_t speed, uint32_t now) {
    while (uart_is_readable(b->uart)) {
        uint32_t dr = uart_get_hw(b->uart)->dr;
        if (!(dr & UART_DR_ERRORS)) {
            hover_side_rx(&b->side, (uint8_t)dr, now);
        }
    }
    uint8_t cmd[HOVER_CMD_LEN];
    if (hover_side_tx(&b->side, speed, now, cmd)) {
        uart_write_blocking(b->uart, cmd, sizeof(cmd));  // fits the 32-byte TX FIFO, does not wait
    }
    bool alive = hover_side_check(&b->side, now);
    if (alive != b->was_alive) {
        printf("aerolink: hoverboard %s %s\n", b->name, alive ? "link up" : "link lost");
        b->was_alive = alive;
    }
}

void motor_set(int16_t left_speed, int16_t right_speed, uint32_t now_ms) {
    board_tick(&left, left_speed, now_ms);
    board_tick(&right, right_speed, now_ms);
}

void motor_feedback(motor_feedback_t *fb) {
    bool l = left.side.alive && left.side.seen;
    bool r = right.side.alive && right.side.seen;
    // Both boards share the battery: average whatever is known.
    uint32_t sum = (l ? left.side.last.volt : 0u) + (r ? right.side.last.volt : 0u);
    uint32_t n = (l ? 1u : 0u) + (r ? 1u : 0u);
    fb->battery_mv = n ? (uint16_t)(sum * 10 / n) : 0;
    fb->speed_l = l ? hover_side_speed(&left.side) : 0;
    fb->speed_r = r ? hover_side_speed(&right.side) : 0;
    fb->temp_dc = TEMP_UNKNOWN;
    fb->faults = 0;
    if (!left.side.alive || !right.side.alive) {
        fb->faults |= FAULT_MOTOR_LINK_LOST;
    }
    if (fb->battery_mv && fb->battery_mv < LOW_BATTERY_MV) {
        fb->faults |= FAULT_LOW_BATTERY;
    }
}
