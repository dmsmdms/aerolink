// Gen2.x REMOTE_UARTBUS protocol (docs/11-hoverboard.md "Driving it from the RP2040"), one hoverboard board per
// RP2040 UART. SDK-free and host-tested; the UART glue lives in motor_hoverboard.c.

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define HOVER_BAUD        19200
#define HOVER_CMD_LEN     8   // '/', type 0, slave, i16 speed, u8 wState, u16 crc
#define HOVER_REPLY_LEN   15  // u16 0xABCD, slave, i16 km/h*100, u16 V*100, i16 A*100, i32 hall steps, u16 crc
#define HOVER_SPEED_MAX   1000  // DRIVING_MODE 0: PWM, -1000..1000 = ‰ of full scale

// wState bits of the command (Gen2.x main.c STATE_*).
#define HOVER_STATE_LED_BATTERY 32  // the board shows its battery level on its own LEDs

#define HOVER_TX_PERIOD_MS 20  // one command + reply is ~12 ms on the wire at 19200 baud
#define HOVER_LINK_TIMEOUT_MS 300  // no valid reply for this long = link lost (FAULT_MOTOR_LINK_LOST)
#define HOVER_BOOT_GRACE_MS   2000  // the boards power up together with the RP2040 and need time to boot
// Wheel speed reported as 1000 ‰ in TELEMETRY. Rough (no load, PWM 100 gave 0.58 km/h at 29 V); calibrate on the robot.
#define HOVER_FULL_SPEED_KMH100 1500

typedef struct {
    uint8_t slave;
    int16_t speed;  // km/h * 100, sign of the command
    uint16_t volt;  // V * 100
    int16_t amp;    // A * 100
    int32_t odom;   // hall steps
} hover_reply_t;

typedef struct {
    uint8_t buf[HOVER_REPLY_LEN];
    size_t len;
} hover_parser_t;

// CRC-16/XMODEM (poly 0x1021, init 0), Gen2.x CalcCRC.
uint16_t hover_crc16(const uint8_t *data, size_t len);

void hover_command(uint8_t out[HOVER_CMD_LEN], uint8_t slave, int16_t speed, uint8_t state);

void hover_parser_init(hover_parser_t *p);
// Feeds one received byte; true when it completed a CRC-valid reply (written to *out).
bool hover_parse(hover_parser_t *p, uint8_t byte, hover_reply_t *out);

// One board on its own UART.
typedef struct {
    uint8_t slave;  // SLAVE_ID flashed into the board
    bool invert;    // mirrored mounting: + forward is - on this board
    hover_parser_t parser;
    hover_reply_t last;
    uint32_t last_rx_ms;
    uint32_t next_tx_ms;
    uint32_t grace_until_ms;
    bool seen;  // a valid reply since init
    bool alive;
} hover_side_t;

void hover_side_init(hover_side_t *s, uint8_t slave, bool invert, uint32_t now_ms);
// Command for speed (‰, + forward) if one is due now: fills out and returns true.
bool hover_side_tx(hover_side_t *s, int16_t speed, uint32_t now_ms, uint8_t out[HOVER_CMD_LEN]);
// One received byte. Replies from another slave id (boards swapped) are ignored.
void hover_side_rx(hover_side_t *s, uint8_t byte, uint32_t now_ms);
// Updates and returns alive: a valid reply within HOVER_LINK_TIMEOUT_MS.
bool hover_side_check(hover_side_t *s, uint32_t now_ms);
// Measured wheel speed in ‰ of HOVER_FULL_SPEED_KMH100, + forward.
int16_t hover_side_speed(const hover_side_t *s);
