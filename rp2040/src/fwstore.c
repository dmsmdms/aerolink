#include "fwstore.h"

#include <string.h>

#include "crc32.h"

#define RECORD_MAGIC 0x52424C41u  // "ALBR"
#define RECORD_WORDS 12           // magic, seq, state, trial_boots, flags, 3 x image, crc

static uint32_t get32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static void put32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
    p[2] = (uint8_t)(v >> 16);
    p[3] = (uint8_t)(v >> 24);
}

static bool record_at(const fw_flash_t *f, uint32_t offset, fw_record_t *r) {
    uint8_t b[RECORD_WORDS * 4];
    f->read(offset, b, sizeof(b), f->ctx);
    if (get32(b) != RECORD_MAGIC || get32(b + 44) != crc32_update(0, b, 44)) {
        return false;
    }
    *r = (fw_record_t){
        .seq = get32(b + 4),
        .state = get32(b + 8),
        .trial_boots = get32(b + 12),
        .flags = get32(b + 16),
        .active = {get32(b + 20), get32(b + 24)},
        .staging = {get32(b + 28), get32(b + 32)},
        .backup = {get32(b + 36), get32(b + 40)},
    };
    return true;
}

void fw_record_read(const fw_flash_t *f, fw_record_t *r) {
    fw_record_t a, b;
    bool has_a = record_at(f, FW_RECORD_A, &a);
    bool has_b = record_at(f, FW_RECORD_B, &b);
    if (has_a && (!has_b || a.seq > b.seq)) {
        *r = a;
    } else if (has_b) {
        *r = b;
    } else {
        *r = (fw_record_t){0};
    }
}

bool fw_record_write(const fw_flash_t *f, fw_record_t *r) {
    r->seq++;
    // Odd sequence numbers live in B, even ones in A: the current copy is never the one being erased.
    uint32_t offset = (r->seq & 1) ? FW_RECORD_B : FW_RECORD_A;
    uint8_t page[FW_PAGE];
    memset(page, 0xFF, sizeof(page));
    uint32_t words[RECORD_WORDS - 1] = {
        RECORD_MAGIC, r->seq, r->state, r->trial_boots, r->flags, r->active.size, r->active.crc,
        r->staging.size, r->staging.crc, r->backup.size, r->backup.crc,
    };
    for (size_t i = 0; i < RECORD_WORDS - 1; i++) {
        put32(page + 4 * i, words[i]);
    }
    put32(page + 44, crc32_update(0, page, 44));
    return f->erase(offset, f->ctx) && f->program(offset, page, f->ctx);
}

uint32_t fw_crc(const fw_flash_t *f, uint32_t offset, uint32_t size) {
    uint8_t buf[FW_PAGE];
    uint32_t crc = 0;
    for (uint32_t done = 0; done < size;) {
        uint32_t n = size - done < sizeof(buf) ? size - done : sizeof(buf);
        f->read(offset + done, buf, n, f->ctx);
        crc = crc32_update(crc, buf, n);
        done += n;
    }
    return crc;
}

typedef enum { COPY_OK, COPY_MISMATCH, COPY_FLASH_ERROR } copy_t;

// Copies `size` bytes (whole sectors erased) from slot `src` to slot `dst` and checks the result against `crc`.

static copy_t copy(const fw_flash_t *f, uint32_t dst, uint32_t src, uint32_t size, uint32_t crc) {
    uint8_t page[FW_PAGE];
    for (uint32_t sector = 0; sector < size; sector += FW_SECTOR) {
        if (!f->erase(dst + sector, f->ctx)) {
            return COPY_FLASH_ERROR;
        }
        for (uint32_t p = sector; p < sector + FW_SECTOR && p < size; p += FW_PAGE) {
            f->read(src + p, page, FW_PAGE, f->ctx);
            if (!f->program(dst + p, page, f->ctx)) {
                return COPY_FLASH_ERROR;
            }
        }
    }
    return fw_crc(f, dst, size) == crc ? COPY_OK : COPY_MISMATCH;
}

// Vector table of the active slot: initial SP in RAM, reset handler (Thumb) inside the slot.
static bool runnable(const fw_flash_t *f) {
    uint8_t v[8];
    f->read(FW_ACTIVE, v, sizeof(v), f->ctx);
    uint32_t sp = get32(v), pc = get32(v + 4);
    uint32_t base = FW_XIP_BASE + FW_ACTIVE;
    return sp > 0x20000000u && sp <= 0x20042000u && (pc & 1) && pc >= base && pc < base + FW_SLOT_SIZE;
}

static bool valid(const fw_image_t *img) { return img->size > 0 && img->size <= FW_SLOT_SIZE; }

fw_boot_t fw_boot(const fw_flash_t *f) {
    fw_record_t r;
    fw_record_read(f, &r);
    // Each pass either returns or moves the record on; a handful of passes covers install + rollback.
    for (int pass = 0; pass < 8; pass++) {
        switch (r.state) {
        case FW_STATE_STAGED:
            if (!valid(&r.active) || !valid(&r.staging) ||
                fw_crc(f, FW_STAGING, r.staging.size) != r.staging.crc) {
                r.state = FW_STATE_NONE;  // nothing sensible to install
            } else {
                switch (copy(f, FW_BACKUP, FW_ACTIVE, r.active.size, r.active.crc)) {
                case COPY_FLASH_ERROR: return FW_FLASH_ERROR;
                case COPY_MISMATCH: r.state = FW_STATE_NONE; break;  // the active slot is not what it should be
                case COPY_OK:
                    r.backup = r.active;
                    r.state = FW_STATE_COPYING;
                    break;
                }
            }
            break;
        case FW_STATE_COPYING:
            switch (copy(f, FW_ACTIVE, FW_STAGING, r.staging.size, r.staging.crc)) {
            case COPY_FLASH_ERROR: return FW_FLASH_ERROR;
            case COPY_MISMATCH:  // staging was verified, so the flash fails: go back to the backup
                r.trial_boots = FW_TRIAL_BOOTS;
                break;
            case COPY_OK:
                r.active = r.staging;
                r.trial_boots = 0;
                r.flags = 0;
                break;
            }
            r.state = FW_STATE_TRIAL;
            break;
        case FW_STATE_TRIAL:
            if (r.trial_boots < FW_TRIAL_BOOTS && runnable(f)) {
                r.trial_boots++;
                return fw_record_write(f, &r) ? FW_RUN : FW_FLASH_ERROR;
            }
            if (!valid(&r.backup)) {
                r.state = FW_STATE_NONE;  // nothing to go back to: keep what there is
                break;
            }
            switch (copy(f, FW_ACTIVE, FW_BACKUP, r.backup.size, r.backup.crc)) {
            case COPY_FLASH_ERROR: return FW_FLASH_ERROR;
            case COPY_MISMATCH: return FW_BOOTSEL;
            case COPY_OK:
                r.active = r.backup;
                r.state = FW_STATE_NONE;
                r.trial_boots = 0;
                r.flags |= FW_FLAG_ROLLED_BACK;
                break;
            }
            break;
        default:  // FW_STATE_NONE
            if (valid(&r.active) && fw_crc(f, FW_ACTIVE, r.active.size) != r.active.crc) {
                // Damaged active slot: restore the backup if there is a good one.
                if (!valid(&r.backup) || fw_crc(f, FW_BACKUP, r.backup.size) != r.backup.crc) {
                    return FW_BOOTSEL;
                }
                r.state = FW_STATE_TRIAL;
                r.trial_boots = FW_TRIAL_BOOTS;
                break;
            }
            return runnable(f) ? FW_RUN : FW_BOOTSEL;
        }
        if (!fw_record_write(f, &r)) {
            return FW_FLASH_ERROR;
        }
    }
    return FW_BOOTSEL;
}

uint8_t fw_boot_kind(const fw_record_t *r) {
    if (r->state == FW_STATE_TRIAL) {
        return FW_BOOT_TRIAL;
    }
    return (r->flags & FW_FLAG_ROLLED_BACK) ? FW_BOOT_ROLLED_BACK : FW_BOOT_NORMAL;
}
