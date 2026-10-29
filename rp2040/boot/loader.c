// aerolink-boot: the loader at the start of flash (docs/03-usb-link.md "Firmware update over the link").
// Finishes an install or a rollback the boot record asks for (LINK-043..045), then starts the application
// in the active slot. It never drives motors and is only ever flashed with a PC (`make flash`).

#include "hardware/structs/nvic.h"
#include "hardware/structs/scb.h"
#include "hardware/structs/systick.h"
#include "hardware/sync.h"
#include "hardware/watchdog.h"
#include "pico/bootrom.h"

#include "flash_rp2040.h"
#include "fwstore.h"

// Long enough for any single flash operation; the application sets its own 100 ms right after start.
#define LOADER_WATCHDOG_MS 2000

static void __attribute__((noreturn)) start_application(void) {
    // Hand over a quiet core: no interrupt enabled or pending, SysTick off (the SDK runtime set them up).
    systick_hw->csr = 0;
    nvic_hw->icer = 0xFFFFFFFFu;
    nvic_hw->icpr = 0xFFFFFFFFu;
    const uint32_t *vectors = (const uint32_t *)(FW_XIP_BASE + FW_ACTIVE);
    scb_hw->vtor = (uintptr_t)vectors;
    __asm volatile(
        "msr msp, %0\n"
        "bx %1\n"
        :
        : "r"(vectors[0]), "r"(vectors[1]));
    __builtin_unreachable();
}

int main(void) {
    watchdog_enable(LOADER_WATCHDOG_MS, true);
    fw_flash_t flash;
    flash_rp2040_init(&flash, LOADER_WATCHDOG_MS);
    switch (fw_boot(&flash)) {
    case FW_RUN:
        watchdog_update();
        start_application();
    case FW_BOOTSEL:
        reset_usb_boot(0, 0);
        break;
    case FW_FLASH_ERROR:
        break;
    }
    watchdog_reboot(0, 0, 0);
    while (true) {
        tight_loop_contents();
    }
}
