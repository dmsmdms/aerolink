// Firmware slots and the boot record (docs/03-usb-link.md "Firmware update over the link", LINK-043..045).
// Shared by the loader (install, trial, rollback) and the application (staging, confirm).
// SDK-free: flash access goes through fw_flash_t, so all of it runs on the host against a RAM "flash"
// with power cuts injected after any operation.

#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Flash layout, offsets from the start of flash (2 MB part, the application is ~40 KB).
#define FW_SECTOR     0x1000u
#define FW_PAGE       0x100u
#define FW_LOADER_MAX 0x0E000u   // boot2 + loader
#define FW_RECORD_A   0x0E000u   // boot record, two sectors written alternately
#define FW_RECORD_B   0x0F000u
#define FW_SLOT_SIZE  0x80000u
#define FW_ACTIVE     0x010000u  // the application runs here (linked at FW_XIP_BASE + FW_ACTIVE)
#define FW_STAGING    0x090000u  // new image as received from the phone
#define FW_BACKUP     0x110000u  // previous image, for rollback
#define FW_FLASH_USED 0x190000u
#define FW_XIP_BASE   0x10000000u

// Unconfirmed boots of a new image before the loader rolls back (LINK-044).
#define FW_TRIAL_BOOTS 3

typedef struct {
    bool (*erase)(uint32_t offset, void *ctx);                       // one FW_SECTOR
    bool (*program)(uint32_t offset, const uint8_t *data, void *ctx);  // one FW_PAGE, erased before
    void (*read)(uint32_t offset, uint8_t *out, size_t len, void *ctx);
    void *ctx;
} fw_flash_t;

typedef struct {
    uint32_t size;  // 0 = unknown / empty
    uint32_t crc;   // CRC-32 of `size` bytes
} fw_image_t;

enum {
    FW_STATE_NONE = 0,  // run the active slot
    FW_STATE_STAGED,    // staging holds a verified image: copy active -> backup, then install
    FW_STATE_COPYING,   // backup done: copy staging -> active
    FW_STATE_TRIAL,     // new image runs until confirmed, at most FW_TRIAL_BOOTS times
};

#define FW_FLAG_ROLLED_BACK 1u  // the active image was restored from the backup

typedef struct {
    uint32_t seq;  // newest valid copy wins
    uint32_t state;
    uint32_t trial_boots;
    uint32_t flags;
    fw_image_t active, staging, backup;
} fw_record_t;

// What the running application tells the phone in HELLO_ACK (`boot`).
enum { FW_BOOT_NORMAL = 0, FW_BOOT_TRIAL = 1, FW_BOOT_ROLLED_BACK = 2 };

// The newest valid record; an all-default one (NONE, nothing known) when there is none.
void fw_record_read(const fw_flash_t *f, fw_record_t *r);

// Writes `r` with the next sequence number into the sector not holding the current copy, so a power cut
// leaves the previous record valid. Updates r->seq.
bool fw_record_write(const fw_flash_t *f, fw_record_t *r);

uint32_t fw_crc(const fw_flash_t *f, uint32_t offset, uint32_t size);

typedef enum {
    FW_RUN,          // start the active slot
    FW_BOOTSEL,      // nothing runnable: USB mass-storage boot (LINK-045)
    FW_FLASH_ERROR,  // a flash operation failed (host tests: the power cut); reboot and try again
} fw_boot_t;

// The loader's decision, finishing any install or rollback first (LINK-043..045). Idempotent: run again
// after a power cut at any point, it completes the same step.
fw_boot_t fw_boot(const fw_flash_t *f);

// FW_BOOT_* of the running application, from the record.
uint8_t fw_boot_kind(const fw_record_t *r);
