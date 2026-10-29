#include "fake_flash.h"

#include <string.h>

static bool erase(uint32_t offset, void *ctx) {
    fake_flash_t *ff = ctx;
    if (ff->dead || offset % FW_SECTOR || offset + FW_SECTOR > FW_FLASH_USED) {
        return false;
    }
    if (ff->budget == 0) {  // cut half way: part of the sector erased
        memset(ff->mem + offset, 0xFF, FW_SECTOR / 2);
        ff->dead = true;
        return false;
    }
    if (ff->budget > 0) {
        ff->budget--;
    }
    ff->ops++;
    memset(ff->mem + offset, 0xFF, FW_SECTOR);
    return true;
}

static bool program(uint32_t offset, const uint8_t *data, void *ctx) {
    fake_flash_t *ff = ctx;
    if (ff->dead || offset % FW_PAGE || offset + FW_PAGE > FW_FLASH_USED) {
        return false;
    }
    uint32_t n = FW_PAGE;
    if (ff->budget == 0) {  // cut half way: part of the page programmed
        n = FW_PAGE / 2;
        ff->dead = true;
    } else if (ff->budget > 0) {
        ff->budget--;
    }
    for (uint32_t i = 0; i < n; i++) {
        if (ff->mem[offset + i] != 0xFF) {
            ff->dirty_programs++;
        }
        ff->mem[offset + i] &= data[i];
    }
    if (ff->dead) {
        return false;
    }
    ff->ops++;
    return true;
}

static void read(uint32_t offset, uint8_t *out, size_t len, void *ctx) {
    fake_flash_t *ff = ctx;
    memcpy(out, ff->mem + offset, len);
}

void fake_flash_init(fake_flash_t *ff) {
    memset(ff->mem, 0xFF, sizeof(ff->mem));
    ff->f = (fw_flash_t){.erase = erase, .program = program, .read = read, .ctx = ff};
    ff->dirty_programs = 0;
    ff->ops = 0;
    fake_flash_power_on(ff);
}

void fake_flash_power_on(fake_flash_t *ff) {
    ff->budget = -1;
    ff->dead = false;
}

void fake_flash_pc_flash(fake_flash_t *ff, const uint8_t *image, uint32_t size) {
    fake_flash_init(ff);
    memcpy(ff->mem + FW_ACTIVE, image, size);
}

void fake_image(uint8_t *out, uint32_t size, uint32_t seed) {
    uint32_t x = seed * 2654435761u + 1;
    for (uint32_t i = 0; i < size; i++) {
        x = x * 1103515245u + 12345u;
        out[i] = (uint8_t)(x >> 16);
    }
    uint32_t sp = 0x20042000u, pc = FW_XIP_BASE + FW_ACTIVE + 0xF7;
    for (int i = 0; i < 4; i++) {
        out[i] = (uint8_t)(sp >> (8 * i));
        out[4 + i] = (uint8_t)(pc >> (8 * i));
    }
}
