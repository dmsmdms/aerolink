#include "flash_rp2040.h"

#include <string.h>

#include "hardware/flash.h"
#include "hardware/sync.h"
#include "hardware/watchdog.h"

#define FLASH_OP_WATCHDOG_MS 1000

static uint32_t normal_watchdog_ms;

static bool erase(uint32_t offset, void *ctx) {
    (void)ctx;
    watchdog_enable(FLASH_OP_WATCHDOG_MS, true);
    uint32_t irq = save_and_disable_interrupts();
    flash_range_erase(offset, FW_SECTOR);
    restore_interrupts(irq);
    watchdog_enable(normal_watchdog_ms, true);
    return true;
}

static bool program(uint32_t offset, const uint8_t *data, void *ctx) {
    (void)ctx;
    watchdog_enable(FLASH_OP_WATCHDOG_MS, true);
    uint32_t irq = save_and_disable_interrupts();
    flash_range_program(offset, data, FW_PAGE);
    restore_interrupts(irq);
    watchdog_enable(normal_watchdog_ms, true);
    return true;
}

static void read(uint32_t offset, uint8_t *out, size_t len, void *ctx) {
    (void)ctx;
    // The SDK flushes the XIP cache after every erase/program, so cached reads see the new contents.
    memcpy(out, (const void *)(XIP_BASE + offset), len);
}

void flash_rp2040_init(fw_flash_t *f, uint32_t watchdog_ms) {
    normal_watchdog_ms = watchdog_ms;
    *f = (fw_flash_t){.erase = erase, .program = program, .read = read};
}
