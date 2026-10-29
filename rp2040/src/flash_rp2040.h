// fw_flash_t on the RP2040's QSPI flash, shared by the loader and the application.

#pragma once

#include <stdint.h>

#include "fwstore.h"

// Erase/program run with interrupts off (the code executing from flash cannot run meanwhile). A sector
// erase can take up to 400 ms, so the watchdog is widened around each operation and set back to
// `watchdog_ms` afterwards (LINK-046); the caller keeps the motors at zero meanwhile.
void flash_rp2040_init(fw_flash_t *f, uint32_t watchdog_ms);
