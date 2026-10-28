// Phone link: decodes frames from the USB transport, dispatches them to the control state
// machine and sends replies, events and telemetry (docs/03-usb-link.md "Behaviour").
// Transport-agnostic and SDK-free: unit-tested on the host.

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "control.h"
#include "motor.h"
#include "proto.h"

// Queues one complete wire frame for sending; returns false if it could not be queued.
typedef bool (*link_tx_fn)(const uint8_t *data, size_t len, void *ctx);

typedef struct {
    control_t *ctl;
    link_tx_fn tx;
    void *tx_ctx;

    proto_decoder_t dec;
    uint8_t tx_seq;
    uint16_t rx_errors;
    bool connected;
    bool hello_done;

    uint32_t caps;
    uint8_t board_id[8];
    uint16_t boot_reason;
    bool boot_reported;

    uint32_t next_telemetry;
    bool bootsel_requested;  // set by REBOOT_BOOTSEL, acted upon by main
} link_t;

void link_init(link_t *l, control_t *ctl, uint32_t caps, const uint8_t board_id[8],
               uint16_t boot_reason, link_tx_fn tx, void *tx_ctx);

// Control-module event sink (pass to control_init with the link as ctx).
void link_on_control_event(uint8_t code, uint16_t arg, void *ctx);

void link_connected(link_t *l);
void link_disconnected(link_t *l);

void link_rx(link_t *l, const uint8_t *data, size_t len, uint32_t now_ms);

// Periodic telemetry while the robot is active (LINK-021); call every loop iteration.
void link_tick(link_t *l, uint32_t now_ms, const motor_feedback_t *fb);
