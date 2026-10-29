// USB transport to the phone: TinyUSB host + Android Open Accessory (docs/03-usb-link.md).
// Switches an attached Android phone into accessory mode (LINK-002), then moves bytes over the
// accessory bulk endpoints. All callbacks run from tuh_task() in the main loop.

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct {
    void (*connected)(void *ctx);
    void (*disconnected)(void *ctx);
    void (*rx)(const uint8_t *data, size_t len, void *ctx);
    void *ctx;
} usb_aoa_callbacks_t;

// serial: RP2040 unique board id as a NUL-terminated hex string (AOA serial string).
void usb_aoa_init(const usb_aoa_callbacks_t *cb, const char *serial);

// Queues one complete wire frame (link_tx_fn compatible). Returns false if the accessory is not
// connected or the queue is full; the frame is then dropped as a whole.
bool usb_aoa_send(const uint8_t *data, size_t len, void *ctx);

bool usb_aoa_connected(void);
