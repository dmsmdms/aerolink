#include "fwupdate.h"

#include <string.h>

void fwupdate_init(fwupdate_t *u, const fw_flash_t *flash, fw_image_t running) {
    memset(u, 0, sizeof(*u));
    u->flash = flash;
    u->running = running;
    fw_record_t r;
    fw_record_read(flash, &r);
    u->boot = fw_boot_kind(&r);
}

uint8_t fwupdate_begin(fwupdate_t *u, uint32_t size, uint32_t crc) {
    // An unconfirmed image must not become the backup (LINK-044): the phone confirms first.
    if (u->boot == FW_BOOT_TRIAL || u->running.size == 0) {
        return FW_BAD_STATE;
    }
    if (size == 0 || size > FW_SLOT_SIZE) {
        return FW_TOO_BIG;
    }
    u->active = true;
    u->staged = false;
    u->size = size;
    u->crc = crc;
    u->received = 0;
    u->written = 0;
    return FW_OK;
}

// Erases the staging sector at `written` and programs the buffered bytes (padded with 0xFF).
static bool flush(fwupdate_t *u) {
    const fw_flash_t *f = u->flash;
    uint32_t n = u->received - u->written;
    memset(u->sector + n, 0xFF, FW_SECTOR - n);
    if (!f->erase(FW_STAGING + u->written, f->ctx)) {
        return false;
    }
    for (uint32_t p = 0; p < n; p += FW_PAGE) {
        if (!f->program(FW_STAGING + u->written + p, u->sector + p, f->ctx)) {
            return false;
        }
    }
    u->written = u->received;
    return true;
}

uint8_t fwupdate_data(fwupdate_t *u, uint32_t offset, const uint8_t *data, uint8_t len, bool *ack) {
    *ack = false;
    if (!u->active || u->staged) {
        return FW_BAD_STATE;
    }
    if (offset == u->written) {
        u->received = u->written;  // resend from the last acknowledged point
    }
    if (offset != u->received) {
        return FW_BAD_OFFSET;
    }
    if (len > u->size - u->received) {
        return FW_TOO_BIG;
    }
    while (len) {
        uint32_t used = u->received - u->written;
        uint32_t n = FW_SECTOR - used < len ? FW_SECTOR - used : len;
        memcpy(u->sector + used, data, n);
        u->received += n;
        data += n;
        len -= (uint8_t)n;
        if (u->received - u->written == FW_SECTOR) {
            if (!flush(u)) {
                u->active = false;
                return FW_FLASH_FAIL;
            }
            *ack = true;
        }
    }
    return FW_OK;
}

uint8_t fwupdate_end(fwupdate_t *u) {
    if (!u->active || u->staged) {
        return FW_BAD_STATE;
    }
    if (u->received != u->size) {
        return FW_BAD_OFFSET;
    }
    if (u->received > u->written && !flush(u)) {
        u->active = false;
        return FW_FLASH_FAIL;
    }
    if (fw_crc(u->flash, FW_STAGING, u->size) != u->crc) {
        u->active = false;
        return FW_CRC_FAIL;
    }
    fw_record_t r;
    fw_record_read(u->flash, &r);
    r.state = FW_STATE_STAGED;
    r.staging = (fw_image_t){u->size, u->crc};
    r.active = u->running;  // unknown to the record after a PC flash
    r.trial_boots = 0;
    if (!fw_record_write(u->flash, &r)) {
        u->active = false;
        return FW_FLASH_FAIL;
    }
    // Staged: the loader installs it on the next boot. Driving is allowed again meanwhile (an operator who took
    // the robot during the transfer keeps it; the phone reboots the base once the session is over).
    u->staged = true;
    u->active = false;
    return FW_OK;
}

uint8_t fwupdate_confirm(fwupdate_t *u) {
    if (u->boot == FW_BOOT_NORMAL) {
        return FW_OK;
    }
    fw_record_t r;
    fw_record_read(u->flash, &r);
    if (r.state == FW_STATE_TRIAL) {
        r.state = FW_STATE_NONE;
        r.trial_boots = 0;
    }
    r.flags &= ~FW_FLAG_ROLLED_BACK;
    if (!fw_record_write(u->flash, &r)) {
        return FW_FLASH_FAIL;
    }
    u->boot = FW_BOOT_NORMAL;
    return FW_OK;
}

void fwupdate_abort(fwupdate_t *u) {
    // A staged image stays in the record: the loader installs it whenever the board restarts.
    u->active = false;
}
