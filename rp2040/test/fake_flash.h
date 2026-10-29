// A RAM "flash" behind fw_flash_t for the host tests: erase sets 0xFF, program can only clear bits (like
// NOR flash), and a power cut can be scheduled after any number of operations.

#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "fwstore.h"

typedef struct {
    uint8_t mem[FW_FLASH_USED];
    long budget;     // operations left before the power cut; < 0 = no cut
    bool dead;       // the cut happened: every further operation fails
    long ops;        // operations done
    int dirty_programs;  // programs over bytes that were not erased (a bug in the code under test)
    fw_flash_t f;
} fake_flash_t;

// Erased flash, no power cut.
void fake_flash_init(fake_flash_t *ff);

// Power back on: operations work again, no cut scheduled.
void fake_flash_power_on(fake_flash_t *ff);

// What `make flash` does: everything erased, `image` in the active slot.
void fake_flash_pc_flash(fake_flash_t *ff, const uint8_t *image, uint32_t size);

// A plausible application image of `size` bytes (vector table into the active slot), contents from `seed`.
void fake_image(uint8_t *out, uint32_t size, uint32_t seed);
