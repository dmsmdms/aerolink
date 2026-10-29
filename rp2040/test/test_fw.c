// Host tests for the firmware update path: CRC-32, boot record, loader decisions (install, trial,
// rollback) with power cuts after every flash operation, and the receiver (LINK-040..046).

#include <stdlib.h>
#include <string.h>

#include "check.h"
#include "crc32.h"
#include "fake_flash.h"
#include "fwstore.h"
#include "fwupdate.h"

#define OLD_SIZE 9000u   // 3 sectors, last one partial
#define NEW_SIZE 12345u  // 4 sectors

static fake_flash_t ff;
static fwupdate_t up;
static uint8_t old_img[OLD_SIZE];
static uint8_t new_img[NEW_SIZE];

static fw_image_t image_of(const uint8_t *img, uint32_t size) { return (fw_image_t){size, crc32_update(0, img, size)}; }

static bool active_is(const uint8_t *img, uint32_t size) { return memcmp(ff.mem + FW_ACTIVE, img, size) == 0; }

// The running application receives `img` from the phone the way link.c drives fwupdate.
static uint8_t receive(const uint8_t *img, uint32_t size, fw_image_t running) {
    fwupdate_init(&up, &ff.f, running);
    uint8_t status = fwupdate_begin(&up, size, crc32_update(0, img, size));
    if (status != FW_OK) {
        return status;
    }
    int acks = 0;
    for (uint32_t off = 0; off < size; off += 56) {
        uint8_t n = size - off < 56 ? (uint8_t)(size - off) : 56;
        bool ack;
        status = fwupdate_data(&up, off, img + off, n, &ack);
        if (status != FW_OK) {
            return status;
        }
        acks += ack;
    }
    CHECK_EQ(acks, size / FW_SECTOR);  // one per full sector (LINK-041)
    return fwupdate_end(&up);
}

// Boots like the board does: power cuts make the loader fail, the next power-on runs it again.
static fw_boot_t boot_until_done(void) {
    for (int i = 0; i < 20; i++) {
        fw_boot_t b = fw_boot(&ff.f);
        if (b != FW_FLASH_ERROR) {
            return b;
        }
        fake_flash_power_on(&ff);
    }
    return FW_FLASH_ERROR;
}

static void test_crc32(void) {
    CHECK_EQ(crc32_update(0, (const uint8_t *)"123456789", 9), 0xCBF43926u);
    uint32_t part = crc32_update(0, (const uint8_t *)"1234", 4);
    CHECK_EQ(crc32_update(part, (const uint8_t *)"56789", 5), 0xCBF43926u);
    CHECK_EQ(crc32_update(0, NULL, 0), 0);
}

static void test_record(void) {
    fake_flash_init(&ff);
    fw_record_t r;
    fw_record_read(&ff.f, &r);
    CHECK_EQ(r.seq, 0);
    CHECK_EQ(r.state, FW_STATE_NONE);
    CHECK_EQ(r.active.size, 0);

    r.state = FW_STATE_STAGED;
    r.staging = (fw_image_t){100, 0xABCD};
    CHECK(fw_record_write(&ff.f, &r));
    r.state = FW_STATE_COPYING;
    CHECK(fw_record_write(&ff.f, &r));
    fw_record_t back;
    fw_record_read(&ff.f, &back);
    CHECK_EQ(back.seq, 2);
    CHECK_EQ(back.state, FW_STATE_COPYING);
    CHECK_EQ(back.staging.crc, 0xABCD);

    // A torn write of the newest copy leaves the previous one in force.
    ff.mem[FW_RECORD_A + 9] ^= 1;  // seq 2 lives in A
    fw_record_read(&ff.f, &back);
    CHECK_EQ(back.seq, 1);
    CHECK_EQ(back.state, FW_STATE_STAGED);
    CHECK_EQ(ff.dirty_programs, 0);
}

static void test_boot_fresh(void) {
    fake_flash_init(&ff);
    CHECK_EQ(fw_boot(&ff.f), FW_BOOTSEL);  // loader alone: nothing to run (LINK-045)

    fake_flash_pc_flash(&ff, old_img, OLD_SIZE);
    CHECK_EQ(fw_boot(&ff.f), FW_RUN);
    CHECK_EQ(ff.ops, 0);  // a normal boot writes nothing
    fwupdate_init(&up, &ff.f, image_of(old_img, OLD_SIZE));
    CHECK_EQ(up.boot, FW_BOOT_NORMAL);
}

static void test_update_and_confirm(void) {
    fake_flash_pc_flash(&ff, old_img, OLD_SIZE);
    CHECK_EQ(receive(new_img, NEW_SIZE, image_of(old_img, OLD_SIZE)), FW_OK);
    CHECK(up.staged);
    CHECK(active_is(old_img, OLD_SIZE));  // nothing installed before the reboot

    CHECK_EQ(fw_boot(&ff.f), FW_RUN);
    CHECK(active_is(new_img, NEW_SIZE));
    CHECK_EQ(memcmp(ff.mem + FW_BACKUP, old_img, OLD_SIZE), 0);
    fw_record_t r;
    fw_record_read(&ff.f, &r);
    CHECK_EQ(r.state, FW_STATE_TRIAL);
    CHECK_EQ(r.trial_boots, 1);
    CHECK_EQ(r.active.crc, image_of(new_img, NEW_SIZE).crc);
    CHECK_EQ(r.backup.crc, image_of(old_img, OLD_SIZE).crc);

    // The new image runs in trial; it must not take another update before the phone confirms it.
    fwupdate_init(&up, &ff.f, image_of(new_img, NEW_SIZE));
    CHECK_EQ(up.boot, FW_BOOT_TRIAL);
    CHECK_EQ(fwupdate_begin(&up, OLD_SIZE, 0), FW_BAD_STATE);
    CHECK_EQ(fwupdate_confirm(&up), FW_OK);
    CHECK_EQ(up.boot, FW_BOOT_NORMAL);
    fw_record_read(&ff.f, &r);
    CHECK_EQ(r.state, FW_STATE_NONE);

    long ops = ff.ops;
    CHECK_EQ(fw_boot(&ff.f), FW_RUN);  // confirmed: plain boots from now on
    CHECK_EQ(ff.ops, ops);
    CHECK(active_is(new_img, NEW_SIZE));
    CHECK_EQ(ff.dirty_programs, 0);
}

// LINK-043: a power cut after any single flash operation of the install still ends with the new image.
static void test_install_power_cuts(void) {
    int cuts = 0;
    for (long cut = 0;; cut++) {
        fake_flash_pc_flash(&ff, old_img, OLD_SIZE);
        CHECK_EQ(receive(new_img, NEW_SIZE, image_of(old_img, OLD_SIZE)), FW_OK);
        ff.budget = cut;
        fw_boot_t b = fw_boot(&ff.f);
        if (b != FW_FLASH_ERROR) {
            CHECK_EQ(b, FW_RUN);
            break;
        }
        cuts++;
        fake_flash_power_on(&ff);
        CHECK_EQ(boot_until_done(), FW_RUN);
        CHECK(active_is(new_img, NEW_SIZE));
        fw_record_t r;
        fw_record_read(&ff.f, &r);
        CHECK_EQ(r.state, FW_STATE_TRIAL);
        CHECK_EQ(r.backup.crc, image_of(old_img, OLD_SIZE).crc);
    }
    CHECK(cuts > 20);  // every erase and program of both copies and the records was hit
}

// A power cut while the image is being received or staged leaves the old image running, unless the cut
// came after the STAGED record was complete in flash: then the new image is installed (in trial).
static void test_receive_power_cuts(void) {
    int installed = 0;
    for (long cut = 0;; cut++) {
        fake_flash_pc_flash(&ff, old_img, OLD_SIZE);
        ff.budget = cut;
        uint8_t status = receive(new_img, NEW_SIZE, image_of(old_img, OLD_SIZE));
        if (status == FW_OK) {
            break;
        }
        CHECK_EQ(status, FW_FLASH_FAIL);
        fake_flash_power_on(&ff);
        CHECK_EQ(boot_until_done(), FW_RUN);
        fw_record_t r;
        fw_record_read(&ff.f, &r);
        if (active_is(new_img, NEW_SIZE)) {
            installed++;
            CHECK_EQ(r.state, FW_STATE_TRIAL);
        } else {
            CHECK(active_is(old_img, OLD_SIZE));
            CHECK_EQ(r.state, FW_STATE_NONE);
        }
    }
    CHECK(installed <= 1);  // only the cut inside the very last program of the record
}

// LINK-044: an image the phone never confirms is replaced by the backup after FW_TRIAL_BOOTS boots.
static void test_rollback(void) {
    fake_flash_pc_flash(&ff, old_img, OLD_SIZE);
    CHECK_EQ(receive(new_img, NEW_SIZE, image_of(old_img, OLD_SIZE)), FW_OK);
    for (int i = 0; i < FW_TRIAL_BOOTS; i++) {
        CHECK_EQ(fw_boot(&ff.f), FW_RUN);
        CHECK(active_is(new_img, NEW_SIZE));
    }
    // Power cuts during the rollback copy are survived as well.
    for (long cut = 0;; cut++) {
        static uint8_t save[FW_FLASH_USED];
        memcpy(save, ff.mem, sizeof(save));
        ff.budget = cut;
        fw_boot_t b = fw_boot(&ff.f);
        fake_flash_power_on(&ff);
        if (b != FW_FLASH_ERROR) {
            CHECK_EQ(b, FW_RUN);
            break;
        }
        CHECK_EQ(boot_until_done(), FW_RUN);
        CHECK(active_is(old_img, OLD_SIZE));
        memcpy(ff.mem, save, sizeof(save));
    }
    CHECK(active_is(old_img, OLD_SIZE));
    fwupdate_init(&up, &ff.f, image_of(old_img, OLD_SIZE));
    CHECK_EQ(up.boot, FW_BOOT_ROLLED_BACK);
    CHECK_EQ(fwupdate_confirm(&up), FW_OK);
    fwupdate_init(&up, &ff.f, image_of(old_img, OLD_SIZE));
    CHECK_EQ(up.boot, FW_BOOT_NORMAL);
    // The old image can take the next update.
    CHECK_EQ(receive(new_img, NEW_SIZE, image_of(old_img, OLD_SIZE)), FW_OK);
}

// An image that is not even a Cortex-M0+ program goes back at once.
static void test_unrunnable_image(void) {
    static uint8_t junk[NEW_SIZE];
    memset(junk, 0x5A, sizeof(junk));
    fake_flash_pc_flash(&ff, old_img, OLD_SIZE);
    CHECK_EQ(receive(junk, NEW_SIZE, image_of(old_img, OLD_SIZE)), FW_OK);
    CHECK_EQ(fw_boot(&ff.f), FW_RUN);
    CHECK(active_is(old_img, OLD_SIZE));
    fwupdate_init(&up, &ff.f, image_of(old_img, OLD_SIZE));
    CHECK_EQ(up.boot, FW_BOOT_ROLLED_BACK);
}

// LINK-045: a damaged active slot is restored from the backup, or the board goes to BOOTSEL.
static void test_damaged_active(void) {
    fake_flash_pc_flash(&ff, old_img, OLD_SIZE);
    CHECK_EQ(receive(new_img, NEW_SIZE, image_of(old_img, OLD_SIZE)), FW_OK);
    CHECK_EQ(fw_boot(&ff.f), FW_RUN);
    fwupdate_init(&up, &ff.f, image_of(new_img, NEW_SIZE));
    CHECK_EQ(fwupdate_confirm(&up), FW_OK);

    ff.mem[FW_ACTIVE + 5000] ^= 0x10;
    CHECK_EQ(fw_boot(&ff.f), FW_RUN);
    CHECK(active_is(old_img, OLD_SIZE));

    ff.mem[FW_ACTIVE + 100] ^= 0x10;
    ff.mem[FW_BACKUP + 100] ^= 0x10;
    CHECK_EQ(fw_boot(&ff.f), FW_BOOTSEL);
}

static void test_receiver(void) {
    fake_flash_pc_flash(&ff, old_img, OLD_SIZE);
    fw_image_t running = image_of(old_img, OLD_SIZE);
    fwupdate_init(&up, &ff.f, running);
    bool ack;
    CHECK_EQ(fwupdate_data(&up, 0, new_img, 56, &ack), FW_BAD_STATE);  // no FW_BEGIN
    CHECK_EQ(fwupdate_end(&up), FW_BAD_STATE);
    CHECK_EQ(fwupdate_begin(&up, 0, 0), FW_TOO_BIG);
    CHECK_EQ(fwupdate_begin(&up, FW_SLOT_SIZE + 1, 0), FW_TOO_BIG);

    uint32_t crc = image_of(new_img, NEW_SIZE).crc;
    CHECK_EQ(fwupdate_begin(&up, NEW_SIZE, crc), FW_OK);
    CHECK(up.active);
    // A lost frame: the next one does not fit, the phone resends from the last acknowledged offset.
    uint32_t off = 0;
    for (; off < 4000; off += 50) {
        CHECK_EQ(fwupdate_data(&up, off, new_img + off, 50, &ack), FW_OK);
    }
    CHECK_EQ(fwupdate_data(&up, off + 50, new_img + off + 50, 50, &ack), FW_BAD_OFFSET);
    CHECK_EQ(up.written, 0);
    int acks = 0;
    for (off = 0; off < NEW_SIZE; off += 50) {
        uint8_t n = NEW_SIZE - off < 50 ? (uint8_t)(NEW_SIZE - off) : 50;
        CHECK_EQ(fwupdate_data(&up, off, new_img + off, n, &ack), FW_OK);
        acks += ack;
    }
    CHECK_EQ(acks, 3);  // frames crossing a sector boundary are split, each full sector acknowledged
    CHECK_EQ(up.written, 3 * FW_SECTOR);
    // Resend from a sector boundary after an ACK went missing.
    CHECK_EQ(fwupdate_data(&up, 3 * FW_SECTOR, new_img + 3 * FW_SECTOR, 10, &ack), FW_OK);
    CHECK_EQ(up.received, 3 * FW_SECTOR + 10);
    CHECK_EQ(fwupdate_end(&up), FW_BAD_OFFSET);  // incomplete
    for (off = 3 * FW_SECTOR + 10; off < NEW_SIZE; off += 50) {
        uint8_t n = NEW_SIZE - off < 50 ? (uint8_t)(NEW_SIZE - off) : 50;
        CHECK_EQ(fwupdate_data(&up, off, new_img + off, n, &ack), FW_OK);
    }
    CHECK_EQ(fwupdate_data(&up, NEW_SIZE, new_img, 1, &ack), FW_TOO_BIG);
    CHECK_EQ(fwupdate_end(&up), FW_OK);
    CHECK_EQ(fwupdate_data(&up, 0, new_img, 1, &ack), FW_BAD_STATE);  // done

    // A transfer that does not match its CRC is not staged.
    fake_flash_pc_flash(&ff, old_img, OLD_SIZE);
    fwupdate_init(&up, &ff.f, running);
    CHECK_EQ(fwupdate_begin(&up, NEW_SIZE, crc ^ 1), FW_OK);
    for (off = 0; off < NEW_SIZE; off += 56) {
        uint8_t n = NEW_SIZE - off < 56 ? (uint8_t)(NEW_SIZE - off) : 56;
        CHECK_EQ(fwupdate_data(&up, off, new_img + off, n, &ack), FW_OK);
    }
    CHECK_EQ(fwupdate_end(&up), FW_CRC_FAIL);
    CHECK(!up.active);
    fw_record_t r;
    fw_record_read(&ff.f, &r);
    CHECK_EQ(r.state, FW_STATE_NONE);
    CHECK_EQ(fw_boot(&ff.f), FW_RUN);
    CHECK(active_is(old_img, OLD_SIZE));

    // HELLO / detach drop the transfer.
    CHECK_EQ(fwupdate_begin(&up, NEW_SIZE, crc), FW_OK);
    fwupdate_abort(&up);
    CHECK(!up.active);
    CHECK_EQ(fwupdate_data(&up, 0, new_img, 1, &ack), FW_BAD_STATE);
    CHECK_EQ(ff.dirty_programs, 0);
}

void test_fw(void) {
    fake_image(old_img, OLD_SIZE, 1);
    fake_image(new_img, NEW_SIZE, 2);
    test_crc32();
    test_record();
    test_boot_fresh();
    test_update_and_confirm();
    test_install_power_cuts();
    test_receive_power_cuts();
    test_rollback();
    test_unrunnable_image();
    test_damaged_active();
    test_receiver();
}
