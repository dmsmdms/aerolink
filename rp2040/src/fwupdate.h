// Receiving a firmware image from the phone into the staging slot (LINK-040..042, LINK-044 confirm).
// SDK-free: unit-tested on the host with a RAM "flash".

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "fwstore.h"

// FW_ACK status
#define FW_OK         0
#define FW_BAD_STATE  1
#define FW_BAD_OFFSET 2
#define FW_TOO_BIG    3
#define FW_FLASH_FAIL 4
#define FW_CRC_FAIL   5

typedef struct {
    const fw_flash_t *flash;
    fw_image_t running;  // this image, for HELLO_ACK and as the backup source
    uint8_t boot;        // FW_BOOT_* (HELLO_ACK)

    bool active;  // FW_BEGIN .. FW_END / abort: the robot refuses ARM (SAFE-007)
    bool staged;  // FW_END accepted, the loader installs it on the next boot
    uint32_t size;
    uint32_t crc;
    uint32_t received;  // bytes accepted so far
    uint32_t written;   // bytes durably in flash (sector-aligned until FW_END)
    uint8_t sector[FW_SECTOR];
} fwupdate_t;

void fwupdate_init(fwupdate_t *u, const fw_flash_t *flash, fw_image_t running);

// The return values are FW_ACK statuses.
uint8_t fwupdate_begin(fwupdate_t *u, uint32_t size, uint32_t crc);

// `offset` must be `received`, or `written` to resend from the last acknowledged point. Sets *ack when
// FW_ACK must be sent although the status is FW_OK (a sector reached flash).
uint8_t fwupdate_data(fwupdate_t *u, uint32_t offset, const uint8_t *data, uint8_t len, bool *ack);

uint8_t fwupdate_end(fwupdate_t *u);

// The phone has seen this image work: ends the trial (or clears the rolled-back flag).
uint8_t fwupdate_confirm(fwupdate_t *u);

// HELLO or USB detach: the transfer is dropped and ARM is allowed again (a staged image stays staged).
void fwupdate_abort(fwupdate_t *u);
