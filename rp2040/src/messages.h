// USB link message set, protocol v1 (docs/03-usb-link.md "Messages").

#pragma once

#include <stdint.h>

// Phone -> firmware
#define MSG_HELLO          0x01
#define MSG_ARM            0x02
#define MSG_DISARM         0x03
#define MSG_DRIVE          0x04
#define MSG_PING           0x05
#define MSG_CONFIG         0x06
#define MSG_REBOOT_BOOTSEL 0x07

// Firmware -> phone
#define MSG_HELLO_ACK 0x81
#define MSG_TELEMETRY 0x82
#define MSG_EVENT     0x83
#define MSG_NACK      0x84
#define MSG_PONG      0x85

// Payload lengths
#define MSG_HELLO_LEN          1
#define MSG_DRIVE_LEN          7
#define MSG_CONFIG_LEN         8
#define MSG_REBOOT_BOOTSEL_LEN 4
#define MSG_HELLO_ACK_LEN      16
#define MSG_TELEMETRY_LEN      13
#define MSG_EVENT_LEN          3
#define MSG_NACK_LEN           2

#define REBOOT_BOOTSEL_MAGIC 0x544F4F42u  // "BOOT"

// state
#define STATE_DISARMED 0
#define STATE_ARMED    1
#define STATE_STOPPING 2
#define STATE_FAULT    3

// DRIVE source
#define SOURCE_LOCAL     0
#define SOURCE_OPERATOR  1
#define SOURCE_AUTOPILOT 2

// caps bits
#define CAP_MOTOR_FEEDBACK  (1u << 0)
#define CAP_BATTERY_VOLTAGE (1u << 1)
#define CAP_MOTOR_SIMULATED (1u << 2)

// faults bits
#define FAULT_MOTOR_LINK_LOST (1u << 0)
#define FAULT_MOTOR_FAULT     (1u << 1)
#define FAULT_LOW_BATTERY     (1u << 2)
#define FAULT_OVERTEMP        (1u << 3)
// Faults that force the FAULT state and block ARM (SAFE-006). LOW_BATTERY is a warning only.
#define FAULT_CRITICAL (FAULT_MOTOR_LINK_LOST | FAULT_MOTOR_FAULT | FAULT_OVERTEMP)

// EVENT codes
#define EVENT_BOOT        1
#define EVENT_STATE       2
#define EVENT_FAULTS      3
#define EVENT_CMD_TIMEOUT 4

// EVENT_BOOT arg
#define BOOT_POWER_ON 0
#define BOOT_WATCHDOG 1
#define BOOT_BOOTSEL  2

// NACK reason
#define NACK_BAD_STATE    1
#define NACK_BAD_ARG      2
#define NACK_FAULT_ACTIVE 3

#define TEMP_UNKNOWN INT16_MIN
